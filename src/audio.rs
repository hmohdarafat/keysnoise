use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, OutputStreamHandle};

use crate::engine::{KeyEvent, Settings, CLICK_CODE};
use crate::keymap::find_key_loc;

const MUTE_KEYCODE: u16 = 0x46; // Scroll Lock
const FALLBACK_CODE: u16 = 0x31; // sound used for unknown keys

pub struct Clip {
    frames: Vec<[f32; 2]>,
    sample_rate: u32,
}

pub struct SoundBank {
    clips: HashMap<u16, Clip>,
}

impl SoundBank {
    /// Loads every `<hexcode>-<0|1>.wav` file from `dir`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let rd = fs::read_dir(dir)
            .map_err(|e| format!("Cannot read sound folder \"{}\": {e}", dir.display()))?;
        let mut clips = HashMap::new();

        for entry in rd.flatten() {
            let path = entry.path();
            let is_wav = path
                .extension()
                .and_then(|e| e.to_str())
                .map_or(false, |e| e.eq_ignore_ascii_case("wav"));
            if !is_wav {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            let Some((code, press)) = stem.split_once('-') else { continue };
            let Ok(code) = u8::from_str_radix(code, 16) else { continue };
            let press: u16 = match press {
                "0" => 0,
                "1" => 1,
                _ => continue,
            };
            if let Ok(clip) = load_wav(&path) {
                clips.insert(code as u16 + press * 256, clip);
            }
        }

        if clips.is_empty() {
            return Err(format!("No key sounds (e.g. 1e-1.wav) found in \"{}\".", dir.display()));
        }
        Ok(Self { clips })
    }

    fn get(&self, code: u16, press: bool) -> Option<&Clip> {
        self.clips.get(&(code + if press { 256 } else { 0 }))
    }
}

fn load_wav(path: &Path) -> Result<Clip, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<f32>, hound::Error>>(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<Vec<f32>, hound::Error>>()
        }
    }
    .map_err(|e| e.to_string())?;

    let channels = (spec.channels as usize).max(1);
    let frames = samples
        .chunks_exact(channels)
        .map(|c| if c.len() > 1 { [c[0], c[1]] } else { [c[0], c[0]] })
        .collect();

    Ok(Clip { frames, sample_rate: spec.sample_rate })
}

/// Press the mute key twice within 2 seconds (without other keys) to toggle mute.
#[derive(Default)]
struct MuteTracker {
    last: Option<Instant>,
    count: u8,
}

impl MuteTracker {
    fn pressed(&mut self, is_mute_key: bool) -> bool {
        if !is_mute_key {
            self.count = 0;
            return false;
        }
        let now = Instant::now();
        let recent = self
            .last
            .map_or(false, |t| now.duration_since(t) < Duration::from_secs(2));
        self.last = Some(now);
        if recent {
            self.count += 1;
            if self.count == 2 {
                self.count = 0;
                return true;
            }
        } else {
            self.count = 1;
        }
        false
    }
}

fn play(handle: &OutputStreamHandle, clip: &Clip, code: u16, s: &Settings) {
    let gain = s.gain.load(Ordering::Relaxed) as f32 / 100.0;
    let width = s.stereo_width.load(Ordering::Relaxed) as f32 / 100.0;
    let pan = (find_key_loc(code) * width).clamp(-1.0, 1.0);
    let left = (1.0 - pan).min(1.0) * gain;
    let right = (1.0 + pan).min(1.0) * gain;

    let mut data = Vec::with_capacity(clip.frames.len() * 2);
    for f in &clip.frames {
        data.push(f[0] * left);
        data.push(f[1] * right);
    }
    let _ = handle.play_raw(SamplesBuffer::new(2, clip.sample_rate, data));
}

/// Audio thread: owns the output stream, ends when all senders are dropped.
pub fn run(
    rx: Receiver<KeyEvent>,
    bank: SoundBank,
    settings: Arc<Settings>,
    ready: Sender<Result<(), String>>,
) {
    let (_stream, handle) = match OutputStream::try_default() {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(format!("Audio output error: {e}")));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    let mut mute = MuteTracker::default();

    while let Ok(ev) = rx.recv() {
        if ev.press && mute.pressed(ev.code == MUTE_KEYCODE) {
            settings.muted.fetch_xor(true, Ordering::Relaxed);
        }
        if settings.muted.load(Ordering::Relaxed) {
            continue;
        }
        if ev.code == CLICK_CODE && !settings.mouse_click.load(Ordering::Relaxed) {
            continue;
        }
        let clip = bank.get(ev.code, ev.press).or_else(|| {
            if settings.fallback.load(Ordering::Relaxed) {
                bank.get(FALLBACK_CODE, ev.press)
            } else {
                None
            }
        });
        if let Some(clip) = clip {
            play(&handle, clip, ev.code, &settings);
        }
    }
}
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::{audio, input};

/// Pseudo key code used for mouse clicks (same as the original project).
pub const CLICK_CODE: u16 = 0xff;

#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub code: u16,
    pub press: bool,
}

pub struct Settings {
    pub gain: AtomicU8,         // 0..=100
    pub stereo_width: AtomicU8, // 0..=100
    pub fallback: AtomicBool,
    pub mouse_click: AtomicBool,
    pub muted: AtomicBool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            gain: AtomicU8::new(100),
            stereo_width: AtomicU8::new(50),
            fallback: AtomicBool::new(true),
            mouse_click: AtomicBool::new(true),
            muted: AtomicBool::new(false),
        }
    }
}

pub struct Engine {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Engine {
    pub fn start(settings: Arc<Settings>, wav_dir: &Path) -> Result<Engine, String> {
        let bank = audio::SoundBank::load(wav_dir)?;
        let devices = input::open_devices()?;

        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();

        let audio_thread = thread::spawn(move || audio::run(rx, bank, settings, ready_tx));
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = audio_thread.join();
                return Err(e);
            }
            Err(_) => return Err("audio thread failed to start".into()),
        }

        let mut threads = vec![audio_thread];
        for dev in devices {
            let (tx, stop) = (tx.clone(), stop.clone());
            threads.push(thread::spawn(move || input::read_loop(dev, tx, stop)));
        }
        Ok(Engine { stop, threads })
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}
//! Global key / mouse events read straight from /dev/input
//! (works on X11, Wayland and the text console).

use std::fs::{self, File, OpenOptions};
use std::io::{Error, ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::engine::{KeyEvent, CLICK_CODE};

const EV_KEY: u16 = 1;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;

const NO_ACCESS: &str = "No permission to read /dev/input/event*.\n\
Run once:  sudo usermod -aG input $USER\n\
then log out and back in.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Keyboard,
    Mouse,
}

pub struct Device {
    file: File,
    kind: Kind,
}

fn list_devices() -> Vec<(PathBuf, Kind)> {
    let text = fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(handlers) = line.strip_prefix("H: Handlers=") else { continue };
        let tokens: Vec<&str> = handlers.split_whitespace().collect();
        let Some(event) = tokens.iter().find(|t| t.starts_with("event")) else { continue };
        let kind = if tokens.contains(&"kbd") {
            Kind::Keyboard
        } else if tokens.iter().any(|t| t.starts_with("mouse")) {
            Kind::Mouse
        } else {
            continue;
        };
        out.push((PathBuf::from("/dev/input").join(*event), kind));
    }
    out
}

pub fn open_devices() -> Result<Vec<Device>, String> {
    let mut devices = Vec::new();
    let mut denied = false;

    for (path, kind) in list_devices() {
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(file) => devices.push(Device { file, kind }),
            Err(e) if e.kind() == ErrorKind::PermissionDenied => denied = true,
            Err(_) => {}
        }
    }

    if !devices.iter().any(|d| d.kind == Kind::Keyboard) {
        return Err(if denied {
            NO_ACCESS.to_string()
        } else {
            "No keyboard input device found.".to_string()
        });
    }
    Ok(devices)
}

pub fn read_loop(dev: Device, tx: Sender<KeyEvent>, stop: Arc<AtomicBool>) {
    let Device { mut file, kind } = dev;
    let fd = file.as_raw_fd();

    // struct input_event { struct timeval time; u16 type; u16 code; i32 value; }
    let long = std::mem::size_of::<libc::c_long>();
    let ev_size = 2 * long + 8;
    let mut buf = vec![0u8; ev_size * 64];

    while !stop.load(Ordering::Relaxed) {
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut pfd, 1, 100) };
        if r < 0 {
            if Error::last_os_error().kind() == ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if r == 0 {
            continue;
        }
        if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            break; // device unplugged
        }

        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for ev in buf[..n].chunks_exact(ev_size) {
                    let o = 2 * long;
                    let etype = u16::from_ne_bytes([ev[o], ev[o + 1]]);
                    let code = u16::from_ne_bytes([ev[o + 2], ev[o + 3]]);
                    let value = i32::from_ne_bytes([ev[o + 4], ev[o + 5], ev[o + 6], ev[o + 7]]);

                    if etype != EV_KEY || value == 2 {
                        continue; // not a key, or auto-repeat
                    }
                    let out = match kind {
                        Kind::Keyboard if code < 0x100 => code,
                        Kind::Mouse if code == BTN_LEFT || code == BTN_RIGHT => CLICK_CODE,
                        _ => continue,
                    };
                    if tx.send(KeyEvent { code: out, press: value == 1 }).is_err() {
                        return;
                    }
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
            Err(_) => break,
        }
    }
}
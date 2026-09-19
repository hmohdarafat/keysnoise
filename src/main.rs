use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use gtk::{glib, prelude::*};

mod audio;
mod engine;
mod input;
mod keymap;

use engine::{Engine, Settings};

const APP_ID: &str = "org.example.Keysnoise";

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

#[derive(Clone)]
struct Ui {
    start: gtk::Button,
    stop: gtk::Button,
    browse: gtk::Button,
    dir: gtk::Entry,
    status: gtk::Label,
}

impl Ui {
    fn set_running(&self, running: bool) {
        self.start.set_sensitive(!running);
        self.stop.set_sensitive(running);
        self.browse.set_sensitive(!running);
        self.dir.set_sensitive(!running);
    }
}

fn slider(label: &str, value: f64) -> (gtk::Box, gtk::Scale) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_width_chars(13);
    lbl.set_xalign(0.0);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_value(value);
    scale.set_hexpand(true);
    scale.set_draw_value(true);
    row.append(&lbl);
    row.append(&scale);
    (row, scale)
}

fn check(label: &str, active: bool) -> gtk::CheckButton {
    let c = gtk::CheckButton::with_label(label);
    c.set_active(active);
    c
}

fn default_wav_dir() -> String {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("KEYSNOISE_WAV_DIR") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("assets/wav"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("assets/wav"));
            if let Some(root) = dir.parent().and_then(|p| p.parent()) {
                candidates.push(root.join("assets/wav")); // target/release -> project root
            }
        }
    }
    candidates.push(PathBuf::from("/usr/share/keysnoise/wav"));
    candidates
        .into_iter()
        .find(|p| p.is_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "assets/wav".to_string())
}

fn build_ui(app: &gtk::Application) {
    let settings = Arc::new(Settings::default());
    let engine: Rc<RefCell<Option<Engine>>> = Rc::new(RefCell::new(None));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Keysnoise")
        .default_width(440)
        .resizable(false)
        .build();

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();

    // Start / Stop
    let start = gtk::Button::with_label("Start");
    start.add_css_class("suggested-action");
    let stop = gtk::Button::with_label("Stop");
    stop.set_sensitive(false);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_homogeneous(true);
    buttons.append(&start);
    buttons.append(&stop);
    root.append(&buttons);

    // Sliders
    let (vol_row, vol) = slider("Volume", 100.0);
    vol.connect_value_changed({
        let s = settings.clone();
        move |sc| s.gain.store(sc.value() as u8, Ordering::Relaxed)
    });
    let (width_row, width) = slider("Stereo width", 50.0);
    width.connect_value_changed({
        let s = settings.clone();
        move |sc| s.stereo_width.store(sc.value() as u8, Ordering::Relaxed)
    });
    root.append(&vol_row);
    root.append(&width_row);

    // Checkboxes
    let mute = check("Mute (or press Scroll Lock twice)", false);
    mute.connect_toggled({
        let s = settings.clone();
        move |c| s.muted.store(c.is_active(), Ordering::Relaxed)
    });
    let fallback = check("Fallback sound for unknown keys", true);
    fallback.connect_toggled({
        let s = settings.clone();
        move |c| s.fallback.store(c.is_active(), Ordering::Relaxed)
    });
    let click = check("Mouse click sound", true);
    click.connect_toggled({
        let s = settings.clone();
        move |c| s.mouse_click.store(c.is_active(), Ordering::Relaxed)
    });
    root.append(&mute);
    root.append(&fallback);
    root.append(&click);

    // Wav folder
    let dir = gtk::Entry::new();
    dir.set_text(&default_wav_dir());
    dir.set_hexpand(true);
    let browse = gtk::Button::with_label("Browse…");
    let dir_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    dir_row.append(&gtk::Label::new(Some("Sounds")));
    dir_row.append(&dir);
    dir_row.append(&browse);
    root.append(&dir_row);

    // Status
    let status = gtk::Label::new(Some("Stopped."));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_max_width_chars(52);
    status.set_selectable(true);
    status.add_css_class("dim-label");
    root.append(&status);

    let ui = Ui { start, stop, browse, dir, status };

    ui.browse.connect_clicked({
        let window = window.clone();
        let dir = ui.dir.clone();
        move |_| {
            let dialog = gtk::FileChooserDialog::new(
                Some("Select the folder with the .wav files"),
                Some(&window),
                gtk::FileChooserAction::SelectFolder,
                &[
                    ("Cancel", gtk::ResponseType::Cancel),
                    ("Select", gtk::ResponseType::Accept),
                ],
            );
            dialog.set_modal(true);
            let dir = dir.clone();
            dialog.connect_response(move |d, resp| {
                if resp == gtk::ResponseType::Accept {
                    if let Some(path) = d.file().and_then(|f| f.path()) {
                        dir.set_text(&path.to_string_lossy());
                    }
                }
                d.close();
            });
            dialog.present();
        }
    });

    ui.start.connect_clicked({
        let ui = ui.clone();
        let engine = engine.clone();
        let settings = settings.clone();
        move |_| {
            if engine.borrow().is_some() {
                return;
            }
            let path = PathBuf::from(ui.dir.text().as_str());
            match Engine::start(settings.clone(), &path) {
                Ok(e) => {
                    *engine.borrow_mut() = Some(e);
                    ui.set_running(true);
                    ui.status.set_text("Running – every keystroke now clicks.");
                }
                Err(msg) => ui.status.set_text(&format!("Error: {msg}")),
            }
        }
    });

    ui.stop.connect_clicked({
        let ui = ui.clone();
        let engine = engine.clone();
        move |_| {
            let old = engine.borrow_mut().take();
            drop(old); // joins the worker threads
            ui.set_running(false);
            ui.status.set_text("Stopped.");
        }
    });

    // Keep the Mute checkbox in sync with the Scroll Lock hotkey.
    glib::timeout_add_local(Duration::from_millis(250), {
        let s = settings.clone();
        let mute = mute.clone();
        move || {
            let m = s.muted.load(Ordering::Relaxed);
            if mute.is_active() != m {
                mute.set_active(m);
            }
            glib::ControlFlow::Continue
        }
    });

    window.set_child(Some(&root));
    window.present();
}
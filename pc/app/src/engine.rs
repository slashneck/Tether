//! The engine: one background thread that gets frames from the source (phone or webcam), applies mirror/rotate/
//! pause/cover, and hands them to Tether Camera and the window's preview. The window only reads `Shared` and sends
//! `Cmd`s, so it can never stall the video.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tether_shm::Shm;
use windows::Win32::Media::MediaFoundation::{MFStartup, MFSTARTUP_FULL, MF_VERSION};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::adb::{Adb, Device};
use crate::cover::Cover;
use crate::decoder::Decoder;
use crate::link::{self, Link};
use crate::settings::{Settings, Source};
use crate::webcam::{self, Webcam, WebcamInfo};
use crate::{frames, vcam};

pub enum Cmd {
    Settings(Settings),
    ToggleCover,
    TogglePause,
    InstallOnPhone,
    RefreshWebcams,
    /// Turn everything off: tell the phone to stop, remove Tether Camera, end the process.
    Quit,
    /// Ask GitHub for a newer Tether. `quiet`: don't show "up to date" or network errors (automatic checks).
    CheckUpdate { quiet: bool },
    /// Download the found update, verify it, run its setup, then quit so it can replace us.
    InstallUpdate,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Tether Camera couldn't be turned on (not installed?).
    NoCamera(String),
    Searching(String),
    Live,
}

#[derive(Clone, Debug)]
pub struct PhoneCamera {
    pub id: String,
    pub facing: String,
    pub torch: bool,
    pub max_fps: u32,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub phase: Phase,
    pub device: String,
    pub link: &'static str,
    pub cameras: Vec<PhoneCamera>,
    pub actual: Option<(u32, u32, u32)>,
    pub battery: Option<u32>,
    pub fps: f32,
    pub mbps: f32,
    pub decode_ms: f32,
    pub in_use: bool,
    pub covered: bool,
    pub paused: bool,
    pub webcams: Vec<WebcamInfo>,
    pub install: Option<(bool, String)>, // (busy, message)
    pub hotkey_error: Option<String>,
    pub cover_error: Option<String>,
    pub update: crate::updater::UpdateState,
    /// The Tether version on the connected phone (to offer updating it).
    pub phone_app: Option<String>,
    /// "Phone (browser)": the link to show as a QR code while waiting for a phone.
    pub pair_url: Option<String>,
}

pub struct Preview {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub serial: u64,
}

pub struct Shared {
    pub status: Mutex<Status>,
    pub settings: Mutex<Settings>,
    pub preview: Mutex<Preview>,
    /// The window sets this while it's minimized, so no preview work is done for nobody.
    pub minimized: std::sync::atomic::AtomicBool,
    pub repaint: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl Shared {
    fn status(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap());
        self.wake();
    }

    fn wake(&self) {
        if let Some(r) = self.repaint.lock().unwrap().as_ref() {
            r();
        }
    }
}

pub fn start() -> (Arc<Shared>, Sender<Cmd>) {
    let saved = Settings::load_saved();
    let first_run = saved.is_none();
    let settings = saved.unwrap_or_default();
    let shared = Arc::new(Shared {
        status: Mutex::new(Status {
            phase: Phase::Searching("Starting…".into()),
            device: String::new(),
            link: "USB",
            cameras: Vec::new(),
            actual: None,
            battery: None,
            fps: 0.0,
            mbps: 0.0,
            decode_ms: 0.0,
            in_use: false,
            covered: false,
            paused: false,
            webcams: Vec::new(),
            install: None,
            hotkey_error: None,
            cover_error: None,
            update: crate::updater::UpdateState::Idle,
            phone_app: None,
            pair_url: None,
        }),
        settings: Mutex::new(settings.clone()),
        preview: Mutex::new(Preview { rgba: Vec::new(), width: 0, height: 0, serial: 0 }),
        minimized: Default::default(),
        repaint: Mutex::new(None),
    });
    let (tx, rx) = std::sync::mpsc::channel();

    // The hotkey thread just turns presses into ToggleCover commands.
    let (press_tx, press_rx) = std::sync::mpsc::channel::<()>();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let hotkeys = crate::hotkey::HotkeyThread::start(press_tx, result_tx);
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        while press_rx.recv().is_ok() {
            if tx2.send(Cmd::ToggleCover).is_err() {
                break;
            }
        }
    });
    let s2 = shared.clone();
    std::thread::spawn(move || {
        while let Ok(r) = result_rx.recv() {
            s2.status(|st| st.hotkey_error = r.err());
        }
    });
    hotkeys.set(settings.hotkey.clone());

    // Automatic update checks: shortly after start, then every 6 hours (if switched on at that moment).
    let (tx3, s4) = (tx.clone(), shared.clone());
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(8));
        loop {
            if s4.settings.lock().unwrap().auto_update && tx3.send(Cmd::CheckUpdate { quiet: true }).is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
        }
    });

    let (s3, tx4) = (shared.clone(), tx.clone());
    std::thread::Builder::new()
        .name("tether-engine".into())
        .spawn(move || Engine::new(s3, rx, tx4, settings, hotkeys).run(first_run))
        .expect("engine thread");
    (shared, tx)
}

struct Engine {
    shared: Arc<Shared>,
    cmds: Receiver<Cmd>,
    /// For background jobs that need to send a command back (e.g. quit after starting an update).
    self_tx: Sender<Cmd>,
    settings: Settings,
    hotkeys: crate::hotkey::HotkeyThread,
    adb: Option<Adb>,
    shm: Option<Shm>,
    shm_tried: Instant,
    cover: Cover,
    covered: bool,
    paused: bool,
    /// Set when the source changed and the current loop should hand back to `run`.
    restart: bool,
    quit: bool,
    last_cover_write: Instant,
    transformed: Vec<u8>,
    looked: Vec<u8>,
    preview_buf: Vec<u8>,
    last_preview: Instant,
    launched_for: Option<String>,
    failed_connects: u32,
}

impl Engine {
    fn new(
        shared: Arc<Shared>,
        cmds: Receiver<Cmd>,
        self_tx: Sender<Cmd>,
        settings: Settings,
        hotkeys: crate::hotkey::HotkeyThread,
    ) -> Engine {
        let mut e = Engine {
            shared,
            cmds,
            self_tx,
            settings,
            hotkeys,
            adb: Adb::find(),
            shm: None,
            shm_tried: Instant::now() - Duration::from_secs(10),
            cover: Cover::builtin(),
            covered: false,
            paused: false,
            restart: false,
            quit: false,
            last_cover_write: Instant::now(),
            transformed: Vec::new(),
            looked: Vec::new(),
            preview_buf: Vec::new(),
            last_preview: Instant::now(),
            launched_for: None,
            failed_connects: 0,
        };
        e.load_cover();
        e
    }

    fn load_cover(&mut self) {
        let (cover, err) = match &self.settings.cover_file {
            Some(p) => match Cover::load(p) {
                Ok(c) => (c, None),
                Err(e) => (Cover::builtin(), Some(e)),
            },
            None => (Cover::builtin(), None),
        };
        self.cover = cover;
        self.shared.status(|s| s.cover_error = err);
    }

    fn run(mut self, first_run: bool) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
        }
        let camera = match vcam::VirtualCamera::start() {
            Ok(c) => Some(c),
            Err(e) => {
                self.shared.status(|s| {
                    s.phase = Phase::NoCamera(format!("Tether Camera isn't installed on this PC ({})", e.code()))
                });
                None
            }
        };
        let webcams = webcam::list();
        // First start: use the PC's webcam if it has one; the phone is one click away under Source.
        if first_run {
            let mut s = self.settings.clone();
            if let Some(cam) = webcams.first() {
                s.source = Source::Webcam { link: cam.link.clone(), name: cam.name.clone() };
            }
            self.apply_settings(s);
        }
        self.shared.status(|s| s.webcams = webcams);
        while !self.quit {
            self.restart = false;
            self.shared.status(|s| s.pair_url = None);
            match self.settings.source.clone() {
                Source::Phone => self.phone_loop(),
                Source::Browser => self.browser_loop(),
                Source::Webcam { link, name } => self.webcam_loop(&link, &name),
            }
        }
        // Quit: nothing of Tether stays on.
        self.settings.save();
        drop(camera); // removes Tether Camera from Windows
        if let Some(adb) = self.adb.take().or_else(Adb::find) {
            adb.remove_forward(link::PORT);
        }
        crate::hard_exit();
    }

    // ---------- commands ----------

    /// Handles queued commands. Returns quickly; call it often.
    fn pump(&mut self) {
        while let Ok(cmd) = self.cmds.try_recv() {
            match cmd {
                Cmd::Settings(new) => self.apply_settings(new),
                Cmd::ToggleCover => {
                    self.covered = !self.covered;
                    let c = self.covered;
                    self.shared.status(|s| s.covered = c);
                }
                Cmd::TogglePause => {
                    self.paused = !self.paused;
                    let p = self.paused;
                    self.shared.status(|s| s.paused = p);
                }
                Cmd::InstallOnPhone => self.install_on_phone(),
                Cmd::RefreshWebcams => {
                    let list = webcam::list();
                    self.shared.status(|s| s.webcams = list);
                }
                Cmd::Quit => {
                    self.quit = true;
                    self.restart = true;
                }
                Cmd::CheckUpdate { quiet } => self.check_update(quiet),
                Cmd::InstallUpdate => self.install_update(),
            }
        }
        if self.shm.is_none() && self.shm_tried.elapsed() > Duration::from_secs(1) {
            self.shm_tried = Instant::now();
            self.shm = Shm::open().ok();
        }
        if let Some(shm) = &self.shm {
            let in_use = shm.in_use();
            let changed = self.shared.status.lock().unwrap().in_use != in_use;
            if changed {
                self.shared.status(|s| s.in_use = in_use);
            }
        }
    }

    fn apply_settings(&mut self, new: Settings) {
        let old = std::mem::replace(&mut self.settings, new.clone());
        new.save();
        *self.shared.settings.lock().unwrap() = new.clone();
        if old.hotkey != new.hotkey {
            self.hotkeys.set(new.hotkey.clone());
        }
        if old.cover_file != new.cover_file {
            self.load_cover();
        }
        let source_changed = old.source != new.source;
        let webcam_format_changed = matches!(new.source, Source::Webcam { .. })
            && (old.width, old.height, old.fps) != (new.width, new.height, new.fps);
        if source_changed || webcam_format_changed {
            self.restart = true;
        }
    }

    fn check_update(&mut self, quiet: bool) {
        use crate::updater::{self, UpdateState};
        let busy = matches!(self.shared.status.lock().unwrap().update, UpdateState::Checking | UpdateState::Downloading(_));
        if busy {
            return;
        }
        if !quiet {
            self.shared.status(|s| s.update = UpdateState::Checking);
        }
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let state = match updater::check() {
                Ok(Some(rel)) => UpdateState::Available(rel),
                Ok(None) if quiet => UpdateState::Idle,
                Ok(None) => UpdateState::UpToDate,
                Err(_) if quiet => UpdateState::Idle,
                Err(e) => UpdateState::Error(e),
            };
            // A quiet check never hides a result the user is looking at.
            shared.status(|s| {
                if !(quiet && matches!(state, UpdateState::Idle)) {
                    s.update = state;
                }
            });
        });
    }

    fn install_update(&mut self) {
        use crate::updater::{self, UpdateState};
        let UpdateState::Available(rel) = self.shared.status.lock().unwrap().update.clone() else { return };
        let (shared, tx) = (self.shared.clone(), self.self_tx.clone());
        shared.status(|s| s.update = UpdateState::Downloading(0.0));
        std::thread::spawn(move || {
            let s2 = shared.clone();
            let result = updater::download(&rel, &move |p| s2.status(|s| s.update = UpdateState::Downloading(p)))
                .and_then(|path| updater::run_setup(&path));
            match result {
                // The setup closes and replaces us, then starts the new Tether.
                Ok(()) => {
                    let _ = tx.send(Cmd::Quit);
                }
                Err(e) => shared.status(|s| s.update = UpdateState::Error(e)),
            }
        });
    }

    fn install_on_phone(&mut self) {
        let Some(adb) = Adb::find() else {
            self.shared.status(|s| s.install = Some((false, "adb is missing from the Tether folder.".into())));
            return;
        };
        let Some(apk) = crate::default_apk() else {
            self.shared.status(|s| s.install = Some((false, "tether.apk is missing from the Tether folder.".into())));
            return;
        };
        let shared = self.shared.clone();
        shared.status(|s| s.install = Some((true, "Installing on your phone…".into())));
        std::thread::spawn(move || {
            let device = match adb.devices().into_iter().find(|d| d.state == "device") {
                Some(d) => d,
                None => {
                    shared.status(|s| {
                        s.install = Some((false, "No phone found. Plug it in with USB debugging on (see the steps below).".into()))
                    });
                    return;
                }
            };
            // A test build signed with another key can't be updated in place: replace it.
            let mut result = adb.install(&device.serial, &apk);
            if result.as_ref().is_err_and(|e| e.contains("different build")) && adb.uninstall(&device.serial) {
                result = adb.install(&device.serial, &apk);
            }
            let msg = match result {
                Ok(()) => {
                    adb.launch(&device.serial);
                    format!("Installed on {}. Tether is opening on the phone.", device.model)
                }
                Err(e) if e.contains("blocked") => {
                    // Some phones block installs from a PC. Installing on the phone itself still works.
                    match adb.push_and_open(&device.serial, &apk) {
                        Ok(()) => "Your phone blocked installing from the PC. Tether.apk is now in the phone's \
                                   Download folder, which is open on the phone: tap Tether.apk, then Install. \
                                   (If it says the app conflicts, uninstall Tether on the phone first.)"
                            .to_string(),
                        Err(e2) => format!("{e} ({e2})"),
                    }
                }
                Err(e) => e,
            };
            shared.status(|s| s.install = Some((false, msg)));
        });
    }

    // ---------- output ----------

    /// One picture from the source: mirror/rotate, then to Tether Camera and the preview.
    fn emit(&mut self, nv12: &[u8], w: usize, h: usize) {
        if self.covered || self.paused {
            // Paused: Tether Camera keeps showing the last frame. Covered: the cover ticker writes.
            if let Some(shm) = &self.shm {
                shm.keep_alive();
            }
            return;
        }
        let (frame, fw, fh): (&[u8], usize, usize) = if self.settings.rotation % 360 != 0 || self.settings.mirror {
            let (tw, th) = frames::transform(nv12, w, h, self.settings.rotation, self.settings.mirror, &mut self.transformed);
            (&self.transformed, tw, th)
        } else {
            (nv12, w, h)
        };
        // Zoom/fit/filters, sized to what the app using Tether Camera asked for (1080p until one does).
        let (ow, oh) = self.shm.as_ref().and_then(|s| s.wanted_format()).map_or((1920, 1080), |f| (f.0 as usize, f.1 as usize));
        let (frame, fw, fh): (&[u8], usize, usize) =
            if crate::look::process(frame, fw, fh, ow, oh, &self.settings.look, &mut self.looked) {
                (&self.looked, ow & !1, oh & !1)
            } else {
                (frame, fw, fh)
            };
        if let Some(shm) = &self.shm {
            shm.write_frame(frame, fw as u32, fh as u32);
        }
        Self::preview(&self.shared, &mut self.preview_buf, &mut self.last_preview, frame, fw, fh);
    }

    fn preview(shared: &Shared, buf: &mut Vec<u8>, last: &mut Instant, frame: &[u8], w: usize, h: usize) {
        if shared.minimized.load(std::sync::atomic::Ordering::Relaxed) || last.elapsed() < Duration::from_millis(30) {
            return;
        }
        *last = Instant::now();
        let (pw, ph) = frames::preview_rgba(frame, w, h, 960, buf);
        {
            let mut p = shared.preview.lock().unwrap();
            std::mem::swap(&mut p.rgba, buf);
            p.width = pw;
            p.height = ph;
            p.serial += 1;
        }
        shared.wake();
    }

    /// While covered (or disconnected with "cover on disconnect"), keeps Tether Camera showing the cover.
    fn tick_cover(&mut self, disconnected: bool) {
        let show = self.covered || (disconnected && self.settings.cover_on_disconnect);
        if !show || self.last_cover_write.elapsed() < Duration::from_millis(33) {
            return;
        }
        self.last_cover_write = Instant::now();
        let (w, h) = (self.cover.width as usize, self.cover.height as usize);
        let frame = self.cover.current().to_vec();
        if let Some(shm) = &self.shm {
            shm.write_frame(&frame, w as u32, h as u32);
        }
        Self::preview(&self.shared, &mut self.preview_buf, &mut self.last_preview, &frame, w, h);
    }

    /// Sleeps while staying responsive: commands, cover animation.
    fn idle(&mut self, total: Duration, disconnected: bool) {
        let end = Instant::now() + total;
        while Instant::now() < end && !self.restart {
            self.pump();
            self.tick_cover(disconnected);
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn set_phase(&self, phase: Phase) {
        let changed = self.shared.status.lock().unwrap().phase != phase;
        if changed {
            self.shared.status(|s| s.phase = phase);
        }
    }

    // ---------- webcam source ----------

    fn webcam_loop(&mut self, link: &str, name: &str) {
        let want = (self.settings.width, self.settings.height, self.settings.fps);
        let mut cam = match Webcam::open(link, want) {
            Ok(c) => c,
            Err(e) => {
                self.set_phase(Phase::Searching(format!("{name}: {e}")));
                self.idle(Duration::from_secs(2), true);
                return;
            }
        };
        let (w, h, fps) = (cam.width, cam.height, cam.fps);
        self.shared.status(|s| {
            s.device = name.to_string();
            s.link = "Webcam";
            s.actual = Some((w, h, fps));
            s.battery = None;
            s.cameras.clear();
        });
        self.set_phase(Phase::Live);
        let mut stats = (Instant::now(), 0u32);
        let mut frame = Vec::new();
        while !self.restart {
            self.pump();
            self.tick_cover(false);
            match cam.read() {
                Ok(Some(f)) => {
                    frame.clear();
                    frame.extend_from_slice(f);
                    self.emit(&frame, w as usize, h as usize);
                    stats.1 += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    self.set_phase(Phase::Searching(format!("{name}: {e}")));
                    self.idle(Duration::from_secs(1), true);
                    return;
                }
            }
            let since = stats.0.elapsed();
            if since >= Duration::from_secs(1) {
                let f = stats.1 as f32 / since.as_secs_f32();
                self.shared.status(|s| {
                    s.fps = f;
                    s.mbps = 0.0;
                    s.decode_ms = 0.0;
                });
                stats = (Instant::now(), 0);
            }
        }
    }

    // ---------- phone through the browser (Wi‑Fi, any phone) ----------

    fn browser_loop(&mut self) {
        let link = crate::browser::BrowserLink::start();
        let mut decoder = match Decoder::new() {
            Ok(d) => d,
            Err(e) => {
                self.set_phase(Phase::Searching(format!("decoder: {e}")));
                self.idle(Duration::from_secs(2), true);
                return;
            }
        };
        self.shared.status(|s| {
            s.link = "Wi‑Fi";
            s.device = "Phone (browser)".into();
            s.cameras.clear();
            s.battery = None;
            s.phone_app = None;
        });
        self.set_phase(Phase::Searching("Starting…".into()));
        let mut live = false;
        let mut frame = Vec::new();
        let mut stats = (Instant::now(), 0u32, 0usize, Duration::ZERO);
        let mut waiting_for_keyframe = true;
        let mut last_frame = Instant::now();
        while !self.restart {
            self.pump();
            self.tick_cover(!live);
            let evt = match link.events.recv_timeout(Duration::from_millis(15)) {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) => {
                    if live {
                        if let Some(shm) = &self.shm {
                            shm.keep_alive();
                        }
                        // A phone that stops sending (screen locked, page closed) shows the cover after 2 s.
                        if last_frame.elapsed() > Duration::from_secs(2) {
                            self.tick_cover(true);
                        }
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            match evt {
                crate::browser::Event::Waiting { url } => {
                    live = false;
                    self.set_phase(Phase::Searching("Scan the code with your phone's camera".into()));
                    self.shared.status(|s| s.pair_url = Some(url));
                }
                crate::browser::Event::Connected => {
                    self.shared.status(|s| s.pair_url = None);
                }
                crate::browser::Event::Lost(reason) => {
                    live = false;
                    self.set_phase(Phase::Searching(reason));
                }
                crate::browser::Event::Frame(au) => {
                    if !live {
                        live = true;
                        self.set_phase(Phase::Live);
                        self.shared.status(|s| s.pair_url = None);
                    }
                    last_frame = Instant::now();
                    let started = Instant::now();
                    let mut got: Option<(u32, u32)> = None;
                    let result = decoder.decode(&au, 0, &mut |nv12, w, h| {
                        frame.clear();
                        frame.extend_from_slice(nv12);
                        got = Some((w, h));
                    });
                    if let Some((w, h)) = got {
                        waiting_for_keyframe = false;
                        self.emit(&frame, w as usize, h as usize);
                        stats.1 += 1;
                        self.shared.status(|s| s.actual = Some((w, h, s.actual.map_or(30, |a| a.2))));
                    }
                    if result.is_err() && !waiting_for_keyframe {
                        link.request_keyframe();
                        waiting_for_keyframe = true;
                    }
                    stats.2 += au.len();
                    stats.3 += started.elapsed();
                }
            }
            let since = stats.0.elapsed();
            if since >= Duration::from_secs(1) {
                let secs = since.as_secs_f32();
                let (f, b, d) = (
                    stats.1 as f32 / secs,
                    stats.2 as f32 * 8.0 / secs / 1e6,
                    if stats.1 > 0 { stats.3.as_secs_f32() * 1000.0 / stats.1 as f32 } else { 0.0 },
                );
                self.shared.status(|s| {
                    s.fps = f;
                    s.mbps = b;
                    s.decode_ms = d;
                    if let Some(a) = s.actual.as_mut() {
                        a.2 = f.round() as u32;
                    }
                });
                stats = (Instant::now(), 0, 0, Duration::ZERO);
            }
        }
        self.shared.status(|s| s.pair_url = None);
    }

    // ---------- phone source ----------

    fn phone_loop(&mut self) {
        self.pump();
        let Some(adb) = self.adb.take().or_else(Adb::find) else {
            self.set_phase(Phase::Searching("adb is missing from the Tether folder".into()));
            self.idle(Duration::from_secs(2), true);
            return;
        };
        let result = self.phone_once(&adb);
        self.adb = Some(adb);
        if let Err((msg, wait)) = result {
            self.set_phase(Phase::Searching(msg));
            self.idle(wait, true);
        }
    }

    fn phone_once(&mut self, adb: &Adb) -> Result<(), (String, Duration)> {
        let devices = adb.devices();
        let Some(device) = devices.iter().find(|d| d.state == "device").cloned() else {
            self.launched_for = None;
            return Err((
                match devices.first() {
                    Some(d) if d.state == "unauthorized" => "Tap \"Allow\" on the USB debugging prompt on your phone".into(),
                    Some(d) => format!("Your phone is {} on USB. Unplug and plug it back in.", d.state),
                    None => "Plug in your phone with a USB cable".into(),
                },
                Duration::from_millis(800),
            ));
        };
        if !adb.is_installed(&device.serial) {
            return Err((
                format!("Tether isn't on your {} yet. Install it from the Setup tab.", device.model),
                Duration::from_secs(2),
            ));
        }
        adb.forward(&device.serial, link::PORT).map_err(|e| (format!("USB forward failed: {e}"), Duration::from_secs(1)))?;
        // Open the app once per plug-in, and again if it keeps not answering (its service may have been stopped).
        if self.launched_for.as_deref() != Some(&device.serial) || (self.failed_connects > 0 && self.failed_connects % 8 == 0) {
            adb.launch(&device.serial);
            self.launched_for = Some(device.serial.clone());
        }
        // No "Connecting…" phase here: on every retry it would alternate with "Waiting for Tether…" and flicker.
        self.shared.status(|s| {
            s.device = device.model.clone();
            s.link = "USB";
        });
        let addr: SocketAddr = ([127, 0, 0, 1], link::PORT).into();
        match self.session(addr, &device) {
            Ok(()) => {
                self.failed_connects = 0;
                Err(("Connection lost, reconnecting…".into(), Duration::from_millis(300)))
            }
            Err(e) => {
                self.failed_connects += 1;
                Err((format!("Waiting for Tether on {} ({e})", device.model), Duration::from_millis(700)))
            }
        }
    }

    fn phone_settings_json(&self) -> serde_json::Value {
        let s = &self.settings;
        json!({
            "pc": std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into()),
            "camera": s.camera,
            "width": s.width,
            "height": s.height,
            "fps": s.fps,
            "bitrate": s.bitrate(),
            "torch": s.torch,
            "preview": s.phone_preview,
            "dim": s.phone_dim,
        })
    }

    /// One connection, from HELLO until it drops. Ok(()) = it was live, then dropped.
    fn session(&mut self, addr: SocketAddr, device: &Device) -> Result<(), String> {
        let link = Link::connect(addr).map_err(|e| e.to_string())?;
        let hello = match link.messages.recv_timeout(Duration::from_secs(2)) {
            Ok(m) if m.kind == link::HELLO => serde_json::from_slice::<serde_json::Value>(&m.payload).unwrap_or_default(),
            _ => return Err("the app isn't open on the phone".into()),
        };
        if hello["proto"].as_u64().unwrap_or(0) != 1 {
            return Err("the phone app is a different version; update it from the Setup tab".into());
        }
        let cameras: Vec<PhoneCamera> = hello["cameras"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| PhoneCamera {
                        id: c["id"].as_str().unwrap_or("0").into(),
                        facing: c["facing"].as_str().unwrap_or("back").into(),
                        torch: c["torch"].as_bool().unwrap_or(false),
                        max_fps: c["maxFps"].as_u64().unwrap_or(30) as u32,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let phone_app = hello["app"].as_str().map(String::from);
        self.shared.status(|s| {
            s.cameras = cameras;
            s.device = device.model.clone();
            s.phone_app = phone_app;
        });

        let mut sent = self.phone_settings_json();
        link.send_json(link::SETTINGS, &sent).map_err(|e| e.to_string())?;
        self.set_phase(Phase::Live);

        let mut decoder = Decoder::new().map_err(|e| format!("decoder: {e}"))?;
        let mut last_rx = Instant::now();
        let mut last_ping = Instant::now();
        let mut stats = (Instant::now(), 0u32, 0usize, Duration::ZERO);
        let mut waiting_for_keyframe = true;
        let mut frame = Vec::new();
        let mut got_frame = false;

        loop {
            self.pump();
            if self.restart {
                if self.quit {
                    // Tell the phone to stop filming and close, and give the message a moment to get there.
                    let _ = link.send(link::BYE, &[]);
                    std::thread::sleep(Duration::from_millis(200));
                }
                return Ok(());
            }
            let now_json = self.phone_settings_json();
            if now_json != sent {
                sent = now_json;
                let _ = link.send_json(link::SETTINGS, &sent);
            }
            // Before the first picture arrives, keep showing the cover/card rather than nothing.
            self.tick_cover(!got_frame);
            if last_ping.elapsed() >= Duration::from_millis(500) {
                last_ping = Instant::now();
                if link.send(link::PING, &[0; 8]).is_err() {
                    return Ok(());
                }
            }
            if last_rx.elapsed() > Duration::from_secs(3) {
                return Ok(());
            }
            let msg = match link.messages.recv_timeout(Duration::from_millis(15)) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            };
            last_rx = Instant::now();
            match msg.kind {
                link::VIDEO if msg.payload.len() > 9 => {
                    let flags = msg.payload[0];
                    let pts = i64::from_be_bytes(msg.payload[1..9].try_into().unwrap());
                    let data = &msg.payload[9..];
                    if flags & link::FLAG_KEYFRAME != 0 {
                        waiting_for_keyframe = false;
                    }
                    let started = Instant::now();
                    let mut got: Option<(u32, u32)> = None;
                    let result = decoder.decode(data, pts, &mut |nv12, w, h| {
                        frame.clear();
                        frame.extend_from_slice(nv12);
                        got = Some((w, h));
                    });
                    let decode_time = started.elapsed();
                    if let Some((w, h)) = got {
                        got_frame = true;
                        self.emit(&frame, w as usize, h as usize);
                        stats.1 += 1;
                    }
                    if result.is_err() && !waiting_for_keyframe {
                        let _ = link.send(link::KEYFRAME, &[]);
                        waiting_for_keyframe = true;
                    }
                    stats.2 += data.len();
                    stats.3 += decode_time;
                }
                link::STATUS => {
                    let st: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
                    // Buttons pressed on the phone (light, preview, dim) show up here; keep the PC side in sync.
                    let mut s = self.settings.clone();
                    if let Some(v) = st["torch"].as_bool() {
                        s.torch = v;
                    }
                    if let Some(v) = st["preview"].as_bool() {
                        s.phone_preview = v;
                    }
                    if let Some(v) = st["dim"].as_bool() {
                        s.phone_dim = v;
                    }
                    if s != self.settings {
                        self.apply_settings(s);
                        sent = self.phone_settings_json();
                    }
                    let actual = match (st["width"].as_u64(), st["height"].as_u64(), st["fps"].as_u64()) {
                        (Some(w), Some(h), Some(f)) if st["streaming"].as_bool() == Some(true) => {
                            Some((w as u32, h as u32, f as u32))
                        }
                        _ => None,
                    };
                    let battery = st["battery"].as_u64().map(|b| b as u32);
                    self.shared.status(|s| {
                        s.actual = actual.or(s.actual);
                        s.battery = battery;
                    });
                }
                _ => {}
            }

            let since = stats.0.elapsed();
            if since >= Duration::from_secs(1) {
                let secs = since.as_secs_f32();
                let (f, b, d) = (
                    stats.1 as f32 / secs,
                    stats.2 as f32 * 8.0 / secs / 1e6,
                    if stats.1 > 0 { stats.3.as_secs_f32() * 1000.0 / stats.1 as f32 } else { 0.0 },
                );
                self.shared.status(|s| {
                    s.fps = f;
                    s.mbps = b;
                    s.decode_ms = d;
                });
                stats = (Instant::now(), 0, 0, Duration::ZERO);
            }
        }
    }
}

/// Where a cover file chosen in the window gets copied, so it keeps working if the original moves.
pub fn cover_store_path(original: &std::path::Path) -> Option<PathBuf> {
    let ext = original.extension()?.to_string_lossy().to_lowercase();
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Tether").join(format!("cover.{ext}")))
}

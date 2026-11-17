//! The global "cover" hotkey. Works while Tether is in the background (Windows RegisterHotKey).

use std::sync::mpsc::Sender;

use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, PostThreadMessageW, MSG, WM_APP, WM_HOTKEY};
use windows::Win32::Foundation::{LPARAM, WPARAM};

use crate::settings::Hotkey;

const WM_SET: u32 = WM_APP + 1;

pub struct HotkeyThread {
    thread_id: u32,
    pending: std::sync::Arc<std::sync::Mutex<Option<Hotkey>>>,
}

impl HotkeyThread {
    /// Starts the listener. `on_press` runs on every press; `on_result` reports whether registering worked.
    pub fn start(on_press: Sender<()>, on_result: Sender<Result<(), String>>) -> HotkeyThread {
        let pending = std::sync::Arc::new(std::sync::Mutex::new(None::<Hotkey>));
        let pending2 = pending.clone();
        let (tid_tx, tid_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("tether-hotkey".into())
            .spawn(move || unsafe {
                let _ = tid_tx.send(GetCurrentThreadId());
                let mut registered = false;
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    if msg.message == WM_HOTKEY {
                        let _ = on_press.send(());
                    } else if msg.message == WM_SET {
                        if registered {
                            let _ = UnregisterHotKey(None, 1);
                            registered = false;
                        }
                        if let Some(hk) = pending2.lock().unwrap().take() {
                            let mut mods = MOD_NOREPEAT;
                            if hk.ctrl {
                                mods |= MOD_CONTROL;
                            }
                            if hk.shift {
                                mods |= MOD_SHIFT;
                            }
                            if hk.alt {
                                mods |= MOD_ALT;
                            }
                            if hk.win {
                                mods |= MOD_WIN;
                            }
                            match RegisterHotKey(None, 1, HOT_KEY_MODIFIERS(mods.0), hk.vk) {
                                Ok(()) => {
                                    registered = true;
                                    let _ = on_result.send(Ok(()));
                                }
                                Err(_) => {
                                    let _ = on_result.send(Err(format!(
                                        "{} is already used by another app. Pick another one.",
                                        hk.label()
                                    )));
                                }
                            }
                        } else {
                            let _ = on_result.send(Ok(()));
                        }
                    }
                }
            })
            .expect("hotkey thread");
        let thread_id = tid_rx.recv().unwrap_or(0);
        HotkeyThread { thread_id, pending }
    }

    /// Replaces the hotkey (None: no hotkey).
    pub fn set(&self, hotkey: Option<Hotkey>) {
        *self.pending.lock().unwrap() = hotkey;
        // The thread may not have its message queue yet right after start; retry briefly.
        for _ in 0..50 {
            if unsafe { PostThreadMessageW(self.thread_id, WM_SET, WPARAM(0), LPARAM(0)) }.is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// Maps an egui key name ("H", "7", "F5") to a Windows virtual-key code.
pub fn vk_from_name(name: &str) -> Option<u32> {
    let up = name.to_ascii_uppercase();
    if up.len() == 1 {
        let c = up.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x6F + n);
        }
    }
    match up.as_str() {
        "PAUSE" => Some(0x13),
        "INSERT" => Some(0x2D),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" => Some(0x21),
        "PAGEDOWN" => Some(0x22),
        _ => None,
    }
}

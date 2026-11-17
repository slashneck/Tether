//! Talking to the phone over USB through adb: find it, forward the Tether port, install and launch the app.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const PACKAGE: &str = "io.github.slashneck.tether";

pub struct Adb {
    exe: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub serial: String,
    /// "device", "unauthorized" (USB debugging prompt not accepted yet), "offline", ...
    pub state: String,
    pub model: String,
}

impl Adb {
    /// Bundled `adb\adb.exe` next to tether.exe first, then the Android SDK, then PATH.
    pub fn find() -> Option<Adb> {
        let mut candidates = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("adb").join("adb.exe"));
            }
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(local).join("Android\\Sdk\\platform-tools\\adb.exe"));
        }
        for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
            if let Some(root) = std::env::var_os(var) {
                candidates.push(PathBuf::from(root).join("platform-tools\\adb.exe"));
            }
        }
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).map(|p| p.join("adb.exe")));
        }
        candidates.into_iter().find(|p| p.is_file()).map(|exe| Adb { exe })
    }

    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        Command::new(&self.exe).args(args).creation_flags(CREATE_NO_WINDOW).output()
    }

    pub fn devices(&self) -> Vec<Device> {
        let Ok(out) = self.run(&["devices", "-l"]) else { return Vec::new() };
        let mut list: Vec<Device> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .skip(1)
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let serial = parts.next()?.to_string();
                let state = parts.next()?.to_string();
                let model = parts
                    .find_map(|p| p.strip_prefix("model:"))
                    .unwrap_or("Android phone")
                    .replace('_', " ");
                // Wi‑Fi adb devices look like ip:port; Tether's Wi‑Fi mode doesn't go through adb.
                (!serial.contains(':')).then_some(Device { serial, state, model })
            })
            .collect();
        for d in list.iter_mut().filter(|d| d.state == "device") {
            d.model = self.display_name(d);
        }
        list
    }

    /// Makes `127.0.0.1:port` on this PC reach `port` on the phone.
    pub fn forward(&self, serial: &str, port: u16) -> Result<(), String> {
        let spec = format!("tcp:{port}");
        let out = self.run(&["-s", serial, "forward", &spec, &spec]).map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    /// The phone's marketing name instead of its model code, where the phone tells us.
    pub fn display_name(&self, device: &Device) -> String {
        for prop in ["ro.product.marketname", "ro.product.vendor.marketname"] {
            if let Ok(out) = self.run(&["-s", &device.serial, "shell", "getprop", prop]) {
                let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !name.is_empty() {
                    return name;
                }
            }
        }
        device.model.clone()
    }

    pub fn uninstall(&self, serial: &str) -> bool {
        self.run(&["-s", serial, "uninstall", PACKAGE])
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("Success"))
            .unwrap_or(false)
    }

    /// Undoes `forward` for every phone (on quit, so nothing of Tether stays set up).
    pub fn remove_forward(&self, port: u16) {
        let _ = self.run(&["forward", "--remove", &format!("tcp:{port}")]);
    }

    pub fn is_installed(&self, serial: &str) -> bool {
        self.run(&["-s", serial, "shell", "pm", "path", PACKAGE])
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("package:"))
            .unwrap_or(false)
    }

    /// Opens Tether on the phone (which turns its camera service on).
    pub fn launch(&self, serial: &str) {
        let component = format!("{PACKAGE}/.MainActivity");
        let _ = self.run(&["-s", serial, "shell", "am", "start", "-n", &component]);
    }

    /// Fallback for phones that block installs from a PC: copy the APK to the phone's
    /// Download folder and open that folder on the phone, so the user taps it once.
    pub fn push_and_open(&self, serial: &str, apk: &std::path::Path) -> Result<(), String> {
        let apk = apk.to_string_lossy();
        let out = self.run(&["-s", serial, "push", &apk, "/sdcard/Download/Tether.apk"]).map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("Couldn't copy the app to the phone: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        let _ = self.run(&[
            "-s",
            serial,
            "shell",
            "am start -a android.intent.action.VIEW -t vnd.android.document/directory \
             -d content://com.android.externalstorage.documents/document/primary%3ADownload",
        ]);
        Ok(())
    }

    /// Installs or updates the phone app. Errors are explained in plain words.
    pub fn install(&self, serial: &str, apk: &std::path::Path) -> Result<(), String> {
        let apk = apk.to_string_lossy();
        let out = self.run(&["-s", serial, "install", "-r", &apk]).map_err(|e| e.to_string())?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if out.status.success() && text.contains("Success") {
            return Ok(());
        }
        Err(explain_install_error(&text))
    }
}

fn explain_install_error(text: &str) -> String {
    if text.contains("INSTALL_FAILED_USER_RESTRICTED") {
        "The phone blocked installing from the PC.".into()
    } else if text.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE") {
        "A different build of Tether is on the phone (signed with another key).".into()
    } else if text.contains("INSTALL_FAILED_INSUFFICIENT_STORAGE") {
        "The phone is out of storage.".into()
    } else if text.contains("INSTALL_FAILED_VERSION_DOWNGRADE") {
        "The phone already has a newer Tether.".into()
    } else {
        let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
        format!("Install failed: {}", last.trim())
    }
}

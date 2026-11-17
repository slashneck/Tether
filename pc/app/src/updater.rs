//! Updates from GitHub releases. Asks GitHub for the latest release; if it's newer, downloads its setup, checks it
//! against the published SHA-256 and runs it in update mode (it replaces Tether and starts it again). Nothing about
//! the user or their PC is sent: it's a plain HTTPS GET to GitHub.

use std::io::{Read, Write};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

pub const REPO: &str = "slashneck/Tether";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
    setup_url: String,
    sha_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(f32),
    Error(String),
}

pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.split(|c: char| c == '.' || c == '-').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into()
}

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    agent()
        .get(url)
        .header("User-Agent", &format!("Tether/{VERSION}"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "No release published yet.".to_string(),
            other => format!("Couldn't reach GitHub ({other})"),
        })
}

/// None when this is the newest version.
pub fn check() -> Result<Option<Release>, String> {
    let mut resp = get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let rel: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let tag = rel["tag_name"].as_str().unwrap_or_default();
    if !is_newer(tag, VERSION) {
        return Ok(None);
    }
    let version = tag.trim_start_matches('v').to_string();
    let assets = rel["assets"].as_array().cloned().unwrap_or_default();
    let url_of = |name: &str| {
        assets
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .and_then(|a| a["browser_download_url"].as_str())
            .map(String::from)
    };
    let setup_name = format!("TetherSetup-{version}.exe");
    let setup_url = url_of(&setup_name).ok_or("The new release has no setup file.")?;
    Ok(Some(Release {
        sha_url: url_of(&format!("{setup_name}.sha256")),
        page: rel["html_url"].as_str().unwrap_or("https://github.com/slashneck/Tether/releases").to_string(),
        version,
        setup_url,
    }))
}

/// Downloads and verifies the setup. Returns its path in %TEMP%.
pub fn download(rel: &Release, progress: &dyn Fn(f32)) -> Result<PathBuf, String> {
    let expected = match &rel.sha_url {
        Some(url) => {
            let mut r = get(url)?;
            let text = r.body_mut().read_to_string().map_err(|e| e.to_string())?;
            Some(text.split_whitespace().next().unwrap_or_default().to_lowercase())
        }
        None => None,
    };
    let mut resp = get(&rel.setup_url)?;
    let total: u64 = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("TetherSetup-{}.exe", rel.version));
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Download stopped ({e})"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if total > 0 {
            progress(done as f32 / total as f32);
        }
    }
    drop(file);
    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    match expected {
        Some(e) if e == actual => Ok(path),
        Some(_) => {
            let _ = std::fs::remove_file(&path);
            Err("The download doesn't match its checksum, so it wasn't run. Try again later.".into())
        }
        None => {
            let _ = std::fs::remove_file(&path);
            Err("The release has no checksum, so the download wasn't run.".into())
        }
    }
}

/// Starts the setup in update mode (Windows asks for permission, since Tether Camera lives in Program Files).
pub fn run_setup(path: &std::path::Path) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows_core::{w, HSTRING};
    let result = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), w!("/update"), None, SW_SHOWNORMAL) };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err("The update was cancelled.".into())
    }
}

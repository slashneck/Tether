//! Tether for Windows: use your Android phone (or any webcam) as "Tether Camera", with cover, zoom and filters.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adb;
mod cover;
mod decoder;
mod engine;
mod frames;
mod hotkey;
mod link;
mod look;
mod settings;
mod ui;
mod updater;
mod vcam;
mod webcam;

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;
use windows_core::w;

/// The APK shipped next to tether.exe, or the one from a local Android build.
pub fn default_apk() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let mut candidates = vec![exe_dir.join("tether.apk")];
    for up in exe_dir.ancestors().take(6) {
        candidates.push(up.join("android\\app\\build\\outputs\\apk\\release\\app-release.apk"));
        candidates.push(up.join("android\\app\\build\\outputs\\apk\\debug\\app-debug.apk"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Ends the process at once. `std::process::exit` from a background thread can hang on Windows while the window
/// thread and camera libraries are still loaded; everything that matters is cleaned up explicitly before this.
pub fn hard_exit() -> ! {
    unsafe {
        let _ = windows::Win32::System::Threading::TerminateProcess(
            windows::Win32::System::Threading::GetCurrentProcess(),
            0,
        );
    }
    std::process::exit(0)
}

fn icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../../../assets/icon-256.png")).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

fn main() -> eframe::Result {
    // One Tether at a time: two would fight over the phone and the camera.
    let _single = unsafe { CreateMutexW(None, true, w!("Local\\Tether.App")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Ok(());
    }

    // Leftovers from swapping in a new build while the old one ran (best effort; they're in use until it quits).
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.starts_with("tether.old") && name.ends_with(".exe") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    let (shared, tx) = engine::start();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Tether")
        .with_inner_size([1200.0, 780.0])
        .with_min_inner_size([960.0, 620.0]);
    if let Some(icon) = icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Tether",
        options,
        Box::new(move |cc| {
            ui::setup_style(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            *shared.repaint.lock().unwrap() = Some(Box::new(move || ctx.request_repaint()));
            Ok(Box::new(ui::App::new(shared, tx, &cc.egui_ctx)))
        }),
    )
}

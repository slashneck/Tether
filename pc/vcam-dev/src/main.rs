//! Developer tool for Tether Camera. The DLL must be registered first (scripts\dev-install-vcam.ps1).
//!
//!   tether-vcam-dev           turn "Tether Camera" on until Enter is pressed (the real app will do this itself)
//!   tether-vcam-dev list      list the cameras apps can see
//!   tether-vcam-dev grab FILE read frames from Tether Camera the way apps do and save the last one as a PNG

use std::io::Read;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED};
use windows_core::{GUID, HSTRING, PWSTR};

/// Must match `CLSID_TETHER_CAMERA` in the vcam crate.
const CLSID_TETHER_CAMERA: GUID = GUID::from_u128(0x7a3c6e1b_4f2d_4b8a_9c61_2e5d8f0a1b37);

fn main() -> windows_core::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
    }
    let result = match args.first().map(String::as_str) {
        None => run_camera(),
        Some("list") => list().map(|_| ()),
        Some("grab") => grab(args.get(1).map(String::as_str).unwrap_or("tether-grab.png")),
        Some(other) => {
            eprintln!("Unknown command {other}");
            Ok(())
        }
    };
    unsafe {
        let _ = MFShutdown();
        CoUninitialize();
    }
    result
}

fn run_camera() -> windows_core::Result<()> {
    unsafe {
        let clsid = format!("{{{:?}}}", CLSID_TETHER_CAMERA);
        println!("Creating Tether Camera ({clsid})");
        let camera = MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_Session,
            MFVirtualCameraAccess_CurrentUser,
            &HSTRING::from("Tether Camera"),
            &HSTRING::from(clsid),
            None,
        )?;
        camera.Start(None)?;
        println!("Tether Camera is on. Open the Camera app, Discord or OBS and pick it.");
        println!("Press Enter to turn it off.");
        let _ = std::io::stdin().read(&mut [0u8]);
        let _ = camera.Remove();
        let _ = camera.Shutdown();
    }
    println!("Tether Camera is off.");
    Ok(())
}

fn devices() -> windows_core::Result<Vec<(String, IMFActivate)>> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
        let mut list = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attrs, &mut list, &mut count)?;
        let mut out = Vec::new();
        for i in 0..count as usize {
            if let Some(activate) = (*list.add(i)).take() {
                let mut name = PWSTR::null();
                let mut len = 0u32;
                activate.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut name, &mut len)?;
                out.push((name.to_string().unwrap_or_default(), activate));
                CoTaskMemFree(Some(name.0 as _));
            }
        }
        CoTaskMemFree(Some(list as _));
        Ok(out)
    }
}

fn list() -> windows_core::Result<Vec<(String, IMFActivate)>> {
    let devices = devices()?;
    for (name, _) in &devices {
        println!("  {name}");
    }
    Ok(devices)
}

fn grab(path: &str) -> windows_core::Result<()> {
    let Some((_, activate)) = devices()?.into_iter().find(|(n, _)| n.starts_with("Tether Camera")) else {
        eprintln!("Tether Camera not found. Is `tether-vcam-dev` (no arguments) running?");
        std::process::exit(2);
    };
    unsafe {
        let source: IMFMediaSource = activate.ActivateObject()?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, None)?;
        let native = reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)?;
        let size = native.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (w, h) = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
        println!("Format: {w}x{h}");

        let started = std::time::Instant::now();
        let mut frames = 0;
        let mut last = Vec::new();
        while frames < 60 {
            let mut flags = 0u32;
            let mut sample = None;
            reader.ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags),
                None,
                Some(&mut sample),
            )?;
            let Some(sample) = sample else { continue };
            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut data = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut data, None, Some(&mut len))?;
            last = std::slice::from_raw_parts(data, len as usize).to_vec();
            buffer.Unlock()?;
            frames += 1;
        }
        let secs = started.elapsed().as_secs_f64();
        println!("Read {frames} frames in {secs:.2}s ({:.1} fps)", frames as f64 / secs);
        let _ = source.Shutdown();

        // Luma plane only: enough to see the picture.
        let file = std::fs::File::create(path).expect("create png");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
        enc.set_color(png::ColorType::Grayscale);
        enc.write_header().unwrap().write_image_data(&last[..w * h]).unwrap();
        println!("Saved {path}");
    }
    Ok(())
}

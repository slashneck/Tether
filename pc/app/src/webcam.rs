//! A real webcam on this PC as Tether's source (instead of the phone).

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows_core::{Interface, PWSTR};

#[derive(Clone, Debug, PartialEq)]
pub struct WebcamInfo {
    pub name: String,
    pub link: String,
}

unsafe fn string_attr(a: &IMFActivate, key: &windows_core::GUID) -> String {
    let mut p = PWSTR::null();
    let mut len = 0u32;
    if a.GetAllocatedString(key, &mut p, &mut len).is_ok() {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        s
    } else {
        String::new()
    }
}

unsafe fn activates() -> windows_core::Result<Vec<IMFActivate>> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 1)?;
    let attrs = attrs.unwrap();
    attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
    let mut list = std::ptr::null_mut();
    let mut count = 0u32;
    MFEnumDeviceSources(&attrs, &mut list, &mut count)?;
    let mut out = Vec::new();
    for i in 0..count as usize {
        if let Some(a) = (*list.add(i)).take() {
            out.push(a);
        }
    }
    CoTaskMemFree(Some(list as _));
    Ok(out)
}

/// Webcams Tether can use. Tether Camera itself is left out (it can't film itself).
pub fn list() -> Vec<WebcamInfo> {
    unsafe {
        activates()
            .unwrap_or_default()
            .iter()
            .map(|a| WebcamInfo {
                name: string_attr(a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME),
                link: string_attr(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK),
            })
            .filter(|w| !w.name.starts_with("Tether Camera"))
            .collect()
    }
}

pub struct Webcam {
    source: IMFMediaSource,
    reader: IMFSourceReader,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    frame: Vec<u8>,
}

impl Webcam {
    /// Opens the webcam with the format closest to `want` (width, height, fps), delivering NV12.
    pub fn open(link: &str, want: (u32, u32, u32)) -> Result<Webcam, String> {
        unsafe {
            let activate = activates()
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|a| string_attr(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK) == link)
                .ok_or("That webcam isn't plugged in")?;
            let source: IMFMediaSource = activate.ActivateObject().map_err(|e| format!("Can't open the webcam ({e})"))?;
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 2).map_err(|e| e.to_string())?;
            let attrs = attrs.unwrap();
            // Lets the reader convert MJPG/YUY2 to NV12 for us.
            let _ = attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1);
            let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs).map_err(|e| e.to_string())?;
            let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

            // Pick the native format closest to what's wanted: right size first, then frame rate.
            let mut best: Option<(i64, IMFMediaType, u32, u32, u32)> = None;
            let mut i = 0;
            while let Ok(t) = reader.GetNativeMediaType(stream, i) {
                i += 1;
                let size = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                let rate = t.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
                let (w, h) = ((size >> 32) as u32, size as u32);
                let fps = (rate >> 32) as u32 / (rate as u32).max(1);
                if w == 0 || h == 0 {
                    continue;
                }
                let size_cost = ((w as i64 - want.0 as i64).abs() + (h as i64 - want.1 as i64).abs()) * 10;
                let fps_cost = (fps as i64 - want.2 as i64).abs() * if fps > want.2 { 1 } else { 40 };
                let cost = size_cost + fps_cost;
                if best.as_ref().map_or(true, |b| cost < b.0) {
                    best = Some((cost, t, w, h, fps));
                }
            }
            let (_, native, w, h, fps) = best.ok_or("The webcam reports no video formats")?;
            reader.SetCurrentMediaType(stream, None, &native).map_err(|e| e.to_string())?;
            let nv12 = MFCreateMediaType().map_err(|e| e.to_string())?;
            nv12.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.to_string())?;
            nv12.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| e.to_string())?;
            nv12.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64).map_err(|e| e.to_string())?;
            reader
                .SetCurrentMediaType(stream, None, &nv12)
                .map_err(|e| format!("The webcam can't give NV12 ({e})"))?;
            Ok(Webcam { source, reader, width: w & !1, height: h & !1, fps, frame: Vec::new() })
        }
    }

    /// Blocks for the next frame (about one frame time). Returns the tightly packed NV12 picture.
    pub fn read(&mut self) -> Result<Option<&[u8]>, String> {
        unsafe {
            let mut flags = 0u32;
            let mut sample = None;
            self.reader
                .ReadSample(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0, None, Some(&mut flags), None, Some(&mut sample))
                .map_err(|e| format!("The webcam stopped ({e})"))?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err("The webcam was unplugged".into());
            }
            let Some(sample) = sample else { return Ok(None) };
            let buffer = sample.ConvertToContiguousBuffer().map_err(|e| e.to_string())?;
            let (w, h) = (self.width as usize, self.height as usize);
            self.frame.resize(w * h * 3 / 2, 0);
            let (base, pitch, b2) = match buffer.cast::<IMF2DBuffer>() {
                Ok(b2) => {
                    let mut p = std::ptr::null_mut();
                    let mut pitch = 0i32;
                    b2.Lock2D(&mut p, &mut pitch).map_err(|e| e.to_string())?;
                    (p as *const u8, pitch as usize, Some(b2))
                }
                Err(_) => {
                    let mut p = std::ptr::null_mut();
                    buffer.Lock(&mut p, None, None).map_err(|e| e.to_string())?;
                    (p as *const u8, w, None)
                }
            };
            // The UV plane follows the Y plane's rows, which may be padded (e.g. 1088 for 1080): the buffer holds
            // 1.5x as many rows as the Y plane has, so the padded height is two thirds of that.
            let total = buffer.GetCurrentLength().unwrap_or(0) as usize;
            let uv_rows = if pitch > 0 && total >= pitch * h * 3 / 2 { (total / pitch) * 2 / 3 } else { h };
            for row in 0..h {
                std::ptr::copy_nonoverlapping(base.add(row * pitch), self.frame.as_mut_ptr().add(row * w), w);
            }
            let uv = base.add(pitch * uv_rows);
            for row in 0..h / 2 {
                std::ptr::copy_nonoverlapping(uv.add(row * pitch), self.frame.as_mut_ptr().add(w * h + row * w), w);
            }
            match b2 {
                Some(b2) => {
                    let _ = b2.Unlock2D();
                }
                None => {
                    let _ = buffer.Unlock();
                }
            }
            Ok(Some(&self.frame))
        }
    }
}

impl Drop for Webcam {
    fn drop(&mut self) {
        unsafe {
            let _ = self.source.Shutdown();
        }
    }
}

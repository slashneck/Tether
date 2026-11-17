//! H.264 → NV12 with Windows' built-in decoder (Media Foundation), in low-latency mode.

use std::mem::ManuallyDrop;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows_core::Interface;

pub struct Decoder {
    mft: IMFTransform,
    provides_samples: bool,
    /// Size of the decoder's buffers (often padded, e.g. 1088 rows for 1080p).
    coded: (usize, usize),
    /// The part of the picture that's real.
    visible: (usize, usize, usize, usize), // x, y, w, h
    frame: Vec<u8>,
}

impl Decoder {
    pub fn new() -> windows_core::Result<Decoder> {
        unsafe {
            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)?;
            if let Ok(attrs) = mft.GetAttributes() {
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            mft.SetInputType(0, &input, 0)?;
            let mut d = Decoder { mft, provides_samples: false, coded: (0, 0), visible: (0, 0, 0, 0), frame: Vec::new() };
            d.pick_output()?;
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(d)
        }
    }

    fn pick_output(&mut self) -> windows_core::Result<()> {
        unsafe {
            let mut i = 0;
            loop {
                let t = self.mft.GetOutputAvailableType(0, i)?;
                if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    self.mft.SetOutputType(0, &t, 0)?;
                    let size = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                    self.coded = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
                    self.visible = (0, 0, self.coded.0, self.coded.1);
                    let mut area = MFVideoArea::default();
                    let bytes = std::slice::from_raw_parts_mut(
                        (&mut area as *mut MFVideoArea).cast::<u8>(),
                        std::mem::size_of::<MFVideoArea>(),
                    );
                    if t.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, bytes, None).is_ok()
                        && area.Area.cx > 0
                        && area.Area.cy > 0
                    {
                        self.visible = (
                            area.OffsetX.value as usize,
                            area.OffsetY.value as usize,
                            area.Area.cx as usize,
                            area.Area.cy as usize,
                        );
                    }
                    let info = self.mft.GetOutputStreamInfo(0)?;
                    self.provides_samples = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
                    return Ok(());
                }
                i += 1;
            }
        }
    }

    /// Feeds one H.264 access unit; calls `on_frame(nv12, width, height)` for every picture that comes out.
    pub fn decode(&mut self, data: &[u8], pts_us: i64, on_frame: &mut dyn FnMut(&[u8], u32, u32)) -> windows_core::Result<()> {
        unsafe {
            let buffer = MFCreateMemoryBuffer(data.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(data.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts_us * 10)?;

            if let Err(e) = self.mft.ProcessInput(0, &sample, 0) {
                if e.code() != MF_E_NOTACCEPTING {
                    return Err(e);
                }
                self.drain(on_frame)?;
                self.mft.ProcessInput(0, &sample, 0)?;
            }
            self.drain(on_frame)
        }
    }

    fn drain(&mut self, on_frame: &mut dyn FnMut(&[u8], u32, u32)) -> windows_core::Result<()> {
        unsafe {
            loop {
                let ours = if self.provides_samples {
                    None
                } else {
                    let info = self.mft.GetOutputStreamInfo(0)?;
                    let s = MFCreateSample()?;
                    s.AddBuffer(&MFCreateMemoryBuffer(info.cbSize)?)?;
                    Some(s)
                };
                let mut out = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(ours),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let result = self.mft.ProcessOutput(0, &mut out, &mut status);
                let sample = ManuallyDrop::take(&mut out[0].pSample);
                drop(ManuallyDrop::take(&mut out[0].pEvents));
                match result {
                    Ok(()) => {
                        if let Some(sample) = sample {
                            self.copy_out(&sample)?;
                            let (_, _, w, h) = self.visible;
                            on_frame(&self.frame, w as u32, h as u32);
                        }
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.pick_output()?,
                    Err(e) => return Err(e),
                }
            }
        }
    }

    /// Copies the visible part of a decoded (possibly padded) NV12 picture into a tightly packed frame.
    unsafe fn copy_out(&mut self, sample: &IMFSample) -> windows_core::Result<()> {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let (cw, ch) = self.coded;
        let (vx, vy, vw, vh) = self.visible;
        let (vw, vh) = (vw & !1, vh & !1);
        self.visible = (vx, vy, vw, vh);
        self.frame.resize(vw * vh * 3 / 2, 0);

        let (base, pitch, locked2d) = match buffer.cast::<IMF2DBuffer>() {
            Ok(b2) => {
                let mut p = std::ptr::null_mut();
                let mut pitch = 0i32;
                b2.Lock2D(&mut p, &mut pitch)?;
                (p as *const u8, pitch as usize, Some(b2))
            }
            Err(_) => {
                let mut p = std::ptr::null_mut();
                buffer.Lock(&mut p, None, None)?;
                (p as *const u8, cw, None)
            }
        };
        let y_plane = base;
        let uv_plane = base.add(pitch * ch);
        for row in 0..vh {
            let src = y_plane.add((vy + row) * pitch + vx);
            std::ptr::copy_nonoverlapping(src, self.frame.as_mut_ptr().add(row * vw), vw);
        }
        let uv_dst = self.frame.as_mut_ptr().add(vw * vh);
        for row in 0..vh / 2 {
            let src = uv_plane.add((vy / 2 + row) * pitch + (vx & !1));
            std::ptr::copy_nonoverlapping(src, uv_dst.add(row * vw), vw);
        }
        match locked2d {
            Some(b2) => b2.Unlock2D()?,
            None => buffer.Unlock()?,
        }
        Ok(())
    }
}

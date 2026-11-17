//! The video stream. Frame Server asks for frames with `RequestSample`; a worker thread answers each request at the
//! stream's frame rate, so a consumer that asks too eagerly never makes us spin.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows_core::{implement, IUnknown, Ref, GUID, HRESULT};

use tether_shm::{scale_nv12, Shm};

use crate::pattern::Pattern;
use crate::source::KS_NOT_SUPPORTED;
use crate::SendCom;

/// The formats Tether Camera offers, best first. Apps pick one; the first is what most apps take by default.
pub const FORMATS: &[(u32, u32, u32)] = &[(1920, 1080, 30), (1280, 720, 30), (1920, 1080, 60), (1280, 720, 60)];

struct Inner {
    running: bool,
    shutdown: bool,
    /// One entry per outstanding RequestSample, holding its token (if any).
    requests: VecDeque<Option<SendCom<IUnknown>>>,
    width: u32,
    height: u32,
    fps: u32,
}

struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
}

#[implement(IMFMediaStream2, IKsControl)]
pub struct Stream {
    queue: IMFMediaEventQueue,
    attributes: IMFAttributes,
    descriptor: IMFStreamDescriptor,
    parent: Mutex<Option<IMFMediaSource>>,
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

fn video_type(width: u32, height: u32, fps: u32) -> windows_core::Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64)?;
        t.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        t.SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)?;
        t.SetUINT32(&MF_MT_DEFAULT_STRIDE, width)?;
        t.SetUINT32(&MF_MT_SAMPLE_SIZE, width * height * 3 / 2)?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, width * height * 12 * fps)?;
        Ok(t)
    }
}

impl Stream {
    pub fn create(index: u32) -> windows_core::Result<windows_core::ComObject<Stream>> {
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 8)?;
            let attributes = attrs.unwrap();
            attributes.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
            attributes.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, index)?;
            attributes.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
            attributes.SetUINT32(&MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color.0 as u32)?;

            let types: Vec<Option<IMFMediaType>> =
                FORMATS.iter().map(|&(w, h, f)| video_type(w, h, f).map(Some)).collect::<Result<_, _>>()?;
            let descriptor = MFCreateStreamDescriptor(index, &types)?;
            descriptor.GetMediaTypeHandler()?.SetCurrentMediaType(types[0].as_ref())?;

            let (width, height, fps) = FORMATS[0];
            Ok(windows_core::ComObject::new(Stream {
                queue: MFCreateEventQueue()?,
                attributes,
                descriptor,
                parent: Mutex::new(None),
                shared: Arc::new(Shared {
                    inner: Mutex::new(Inner {
                        running: false,
                        shutdown: false,
                        requests: VecDeque::new(),
                        width,
                        height,
                        fps,
                    }),
                    wake: Condvar::new(),
                }),
                worker: Mutex::new(None),
            }))
        }
    }

    pub fn descriptor(&self) -> &IMFStreamDescriptor {
        &self.descriptor
    }

    pub fn attributes(&self) -> &IMFAttributes {
        &self.attributes
    }

    pub fn set_parent(&self, parent: Option<IMFMediaSource>) {
        *self.parent.lock().unwrap() = parent;
    }

    pub fn is_running(&self) -> bool {
        self.shared.inner.lock().unwrap().running
    }

    pub fn start(&self, media_type: &IMFMediaType) -> windows_core::Result<()> {
        let (size, rate) = unsafe { (media_type.GetUINT64(&MF_MT_FRAME_SIZE)?, media_type.GetUINT64(&MF_MT_FRAME_RATE)?) };
        let (width, height) = ((size >> 32) as u32, size as u32);
        let (num, den) = ((rate >> 32) as u32, (rate as u32).max(1));
        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.width = width;
            inner.height = height;
            inner.fps = (num / den).clamp(1, 120);
            inner.running = true;
        }
        self.shared.wake.notify_all();

        let mut worker = self.worker.lock().unwrap();
        if worker.is_none() {
            let shared = self.shared.clone();
            let queue = SendCom(self.queue.clone());
            *worker = Some(std::thread::Builder::new().name("tether-vcam".into()).spawn(move || run(shared, queue)).unwrap());
        }
        unsafe { self.queue.QueueEventParamVar(MEStreamStarted.0 as u32, &GUID::zeroed(), HRESULT(0), std::ptr::null()) }
    }

    pub fn stop(&self) -> windows_core::Result<()> {
        let was_running = {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.requests.clear();
            std::mem::replace(&mut inner.running, false)
        };
        if was_running {
            unsafe { self.queue.QueueEventParamVar(MEStreamStopped.0 as u32, &GUID::zeroed(), HRESULT(0), std::ptr::null())? };
        }
        Ok(())
    }

    pub fn shutdown(&self) {
        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.shutdown = true;
            inner.running = false;
            inner.requests.clear();
        }
        self.shared.wake.notify_all();
        if let Some(handle) = self.worker.lock().unwrap().take() {
            let _ = handle.join();
        }
        unsafe {
            let _ = self.queue.Shutdown();
        }
    }
}

/// The worker: waits for a request, then for a fresh phone frame (or the next tick of the test card), delivers it.
fn run(shared: Arc<Shared>, queue: SendCom<IMFMediaEventQueue>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut pattern = Pattern::new();
    let mut next_due = Instant::now();
    let mut shm: Option<Shm> = None;
    let mut shm_tried: Option<Instant> = None;
    let mut phone_frame = Vec::new();
    let mut last_count = 0u64;
    loop {
        let (token, width, height, fps) = {
            let mut inner = shared.inner.lock().unwrap();
            while !inner.shutdown && !(inner.running && !inner.requests.is_empty()) {
                inner = shared.wake.wait(inner).unwrap();
            }
            if inner.shutdown {
                break;
            }
            (inner.requests.pop_front().unwrap(), inner.width, inner.height, inner.fps)
        };

        if shm.is_none() && shm_tried.map_or(true, |t| t.elapsed() > Duration::from_secs(1)) {
            shm_tried = Some(Instant::now());
            shm = Shm::create().ok();
        }
        if let Some(s) = &shm {
            s.set_wanted_format(width, height, fps);
            s.mark_reading();
        }

        let period = Duration::from_nanos(1_000_000_000 / fps as u64);
        let live = shm.as_ref().filter(|s| s.writer_alive());
        if let Some(s) = live {
            // Hand over each phone frame as soon as it lands; if one is late, repeat the last one after 1.5 periods.
            let deadline = Instant::now() + period * 3 / 2;
            while s.frame_count() == last_count && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            next_due = Instant::now() + period;
        } else {
            let now = Instant::now();
            if next_due > now {
                std::thread::sleep(next_due - now);
                next_due += period;
            } else {
                // We fell behind (or this is the first frame): restart the clock instead of bursting to catch up.
                next_due = now + period;
            }
        }

        if !shared.inner.lock().unwrap().running {
            continue;
        }
        let mut fill = |out: &mut [u8]| {
            if let Some(s) = live {
                if let Some((pw, ph, count)) = s.read_latest(&mut phone_frame) {
                    last_count = count;
                    scale_nv12(&phone_frame, pw as usize, ph as usize, out, width as usize, height as usize);
                    return;
                }
            }
            pattern.render(out, width as usize, height as usize);
        };
        if let Err(e) = deliver(&queue.0, &mut fill, token.map(|t| t.0), width, height, fps) {
            unsafe {
                let _ = queue.0.QueueEventParamVar(MEError.0 as u32, &GUID::zeroed(), e.code(), std::ptr::null());
            }
        }
    }
    unsafe { CoUninitialize() };
}

fn deliver(
    queue: &IMFMediaEventQueue,
    fill: &mut dyn FnMut(&mut [u8]),
    token: Option<IUnknown>,
    width: u32,
    height: u32,
    fps: u32,
) -> windows_core::Result<()> {
    let size = width * height * 3 / 2;
    unsafe {
        let buffer = MFCreateMemoryBuffer(size)?;
        let mut data = std::ptr::null_mut();
        buffer.Lock(&mut data, None, None)?;
        fill(std::slice::from_raw_parts_mut(data, size as usize));
        buffer.Unlock()?;
        buffer.SetCurrentLength(size)?;

        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(MFGetSystemTime())?;
        sample.SetSampleDuration(10_000_000 / fps as i64)?;
        if let Some(token) = &token {
            sample.SetUnknown(&MFSampleExtension_Token, token)?;
        }
        queue.QueueEventParamUnk(MEMediaSample.0 as u32, &GUID::zeroed(), HRESULT(0), &sample)
    }
}

impl IMFMediaEventGenerator_Impl for Stream_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> windows_core::Result<IMFMediaEvent> {
        unsafe { self.queue.GetEvent(flags.0) }
    }
    fn BeginGetEvent(&self, cb: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> windows_core::Result<()> {
        unsafe { self.queue.BeginGetEvent(cb.as_ref(), state.as_ref()) }
    }
    fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> windows_core::Result<IMFMediaEvent> {
        unsafe { self.queue.EndGetEvent(result.as_ref()) }
    }
    fn QueueEvent(&self, met: u32, ext: *const GUID, hr: HRESULT, v: *const PROPVARIANT) -> windows_core::Result<()> {
        unsafe { self.queue.QueueEventParamVar(met, ext, hr, v) }
    }
}

impl IMFMediaStream_Impl for Stream_Impl {
    fn GetMediaSource(&self) -> windows_core::Result<IMFMediaSource> {
        self.parent.lock().unwrap().clone().ok_or_else(|| MF_E_SHUTDOWN.into())
    }

    fn GetStreamDescriptor(&self) -> windows_core::Result<IMFStreamDescriptor> {
        Ok(self.descriptor.clone())
    }

    fn RequestSample(&self, token: Ref<IUnknown>) -> windows_core::Result<()> {
        {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.shutdown {
                return Err(MF_E_SHUTDOWN.into());
            }
            if !inner.running {
                return Err(MF_E_MEDIA_SOURCE_WRONGSTATE.into());
            }
            inner.requests.push_back(token.as_ref().cloned().map(SendCom));
        }
        self.shared.wake.notify_all();
        Ok(())
    }
}

impl IMFMediaStream2_Impl for Stream_Impl {
    fn SetStreamState(&self, state: MF_STREAM_STATE) -> windows_core::Result<()> {
        if state == MF_STREAM_STATE_STOPPED {
            self.stop()
        } else if state == MF_STREAM_STATE_RUNNING {
            let media_type = unsafe { self.descriptor.GetMediaTypeHandler()?.GetCurrentMediaType()? };
            self.start(&media_type)
        } else {
            Err(MF_E_INVALID_STATE_TRANSITION.into())
        }
    }

    fn GetStreamState(&self) -> windows_core::Result<MF_STREAM_STATE> {
        Ok(if self.is_running() { MF_STREAM_STATE_RUNNING } else { MF_STREAM_STATE_STOPPED })
    }
}

impl IKsControl_Impl for Stream_Impl {
    fn KsProperty(&self, _p: *const KSIDENTIFIER, _pl: u32, _d: *mut core::ffi::c_void, _dl: u32, _r: *mut u32) -> windows_core::Result<()> {
        Err(KS_NOT_SUPPORTED.into())
    }
    fn KsMethod(&self, _p: *const KSIDENTIFIER, _pl: u32, _d: *mut core::ffi::c_void, _dl: u32, _r: *mut u32) -> windows_core::Result<()> {
        Err(KS_NOT_SUPPORTED.into())
    }
    fn KsEvent(&self, _p: *const KSIDENTIFIER, _pl: u32, _d: *mut core::ffi::c_void, _dl: u32, _r: *mut u32) -> windows_core::Result<()> {
        Err(KS_NOT_SUPPORTED.into())
    }
}

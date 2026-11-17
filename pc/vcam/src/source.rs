//! The live media source: one video stream, always "live", started and stopped by Frame Server.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::VT_I8;
use windows_core::{implement, ComObject, IUnknown, Interface, Ref, BOOL, GUID, HRESULT};

use crate::stream::Stream;

/// HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND): "this property set isn't supported", what KS callers expect.
pub(crate) const KS_NOT_SUPPORTED: HRESULT = HRESULT(0x8007_0492_u32 as i32);

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl)]
pub struct Source {
    queue: IMFMediaEventQueue,
    attributes: IMFAttributes,
    descriptor: IMFPresentationDescriptor,
    stream: ComObject<Stream>,
    shut: AtomicBool,
}

impl Source {
    pub fn create(activator_attrs: &IMFAttributes) -> windows_core::Result<IMFMediaSource> {
        let mut attrs = None;
        unsafe { MFCreateAttributes(&mut attrs, 8)? };
        let attributes = attrs.unwrap();
        unsafe { activator_attrs.CopyAllItems(&attributes)? };

        let stream = Stream::create(0)?;
        let descriptor = unsafe { MFCreatePresentationDescriptor(Some(&[Some(stream.descriptor().clone())]))? };
        let queue = unsafe { MFCreateEventQueue()? };

        let source = ComObject::new(Source { queue, attributes, descriptor, stream, shut: AtomicBool::new(false) });
        let ex: IMFMediaSourceEx = source.to_interface();
        let iface: IMFMediaSource = ex.cast()?;
        source.stream.set_parent(Some(iface.clone()));
        Ok(iface)
    }

    fn check(&self) -> windows_core::Result<()> {
        if self.shut.load(Ordering::SeqCst) {
            Err(MF_E_SHUTDOWN.into())
        } else {
            Ok(())
        }
    }
}

fn time_variant(t: i64) -> PROPVARIANT {
    let mut pv = PROPVARIANT::default();
    unsafe {
        let inner = &mut *pv.Anonymous.Anonymous;
        inner.vt = VT_I8;
        inner.Anonymous.hVal = t;
    }
    pv
}

impl IMFMediaEventGenerator_Impl for Source_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> windows_core::Result<IMFMediaEvent> {
        self.check()?;
        unsafe { self.queue.GetEvent(flags.0) }
    }
    fn BeginGetEvent(&self, cb: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> windows_core::Result<()> {
        self.check()?;
        unsafe { self.queue.BeginGetEvent(cb.as_ref(), state.as_ref()) }
    }
    fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> windows_core::Result<IMFMediaEvent> {
        self.check()?;
        unsafe { self.queue.EndGetEvent(result.as_ref()) }
    }
    fn QueueEvent(&self, met: u32, ext: *const GUID, hr: HRESULT, v: *const PROPVARIANT) -> windows_core::Result<()> {
        self.check()?;
        unsafe { self.queue.QueueEventParamVar(met, ext, hr, v) }
    }
}

impl IMFMediaSource_Impl for Source_Impl {
    fn GetCharacteristics(&self) -> windows_core::Result<u32> {
        self.check()?;
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }

    fn CreatePresentationDescriptor(&self) -> windows_core::Result<IMFPresentationDescriptor> {
        self.check()?;
        unsafe { self.descriptor.Clone() }
    }

    fn Start(&self, pd: Ref<IMFPresentationDescriptor>, _fmt: *const GUID, _pos: *const PROPVARIANT) -> windows_core::Result<()> {
        self.check()?;
        let pd = pd.ok().map_err(|_| windows_core::Error::from(E_INVALIDARG))?;
        unsafe {
            let count = pd.GetStreamDescriptorCount()?;
            for i in 0..count {
                let mut selected = BOOL(0);
                let mut sd = None;
                pd.GetStreamDescriptorByIndex(i, &mut selected, &mut sd)?;
                let Some(sd) = sd else { continue };
                if sd.GetStreamIdentifier()? != 0 {
                    return Err(E_INVALIDARG.into());
                }
                if selected.as_bool() {
                    self.descriptor.SelectStream(0)?;
                    let media_type = sd.GetMediaTypeHandler()?.GetCurrentMediaType()?;
                    let event = if self.stream.is_running() { MEUpdatedStream } else { MENewStream };
                    let unk: IUnknown = self.stream.to_interface();
                    self.queue.QueueEventParamUnk(event.0 as u32, &GUID::zeroed(), HRESULT(0), &unk)?;
                    self.stream.start(&media_type)?;
                } else {
                    self.descriptor.DeselectStream(0)?;
                    self.stream.stop()?;
                }
            }
            let time = time_variant(MFGetSystemTime());
            self.queue.QueueEventParamVar(MESourceStarted.0 as u32, &GUID::zeroed(), HRESULT(0), &time)?;
        }
        Ok(())
    }

    fn Stop(&self) -> windows_core::Result<()> {
        self.check()?;
        self.stream.stop()?;
        unsafe {
            let time = time_variant(MFGetSystemTime());
            self.queue.QueueEventParamVar(MESourceStopped.0 as u32, &GUID::zeroed(), HRESULT(0), &time)
        }
    }

    fn Pause(&self) -> windows_core::Result<()> {
        self.check()?;
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }

    fn Shutdown(&self) -> windows_core::Result<()> {
        if self.shut.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.stream.shutdown();
        self.stream.set_parent(None);
        unsafe { self.queue.Shutdown() }
    }
}

impl IMFMediaSourceEx_Impl for Source_Impl {
    fn GetSourceAttributes(&self) -> windows_core::Result<IMFAttributes> {
        self.check()?;
        Ok(self.attributes.clone())
    }

    fn GetStreamAttributes(&self, id: u32) -> windows_core::Result<IMFAttributes> {
        self.check()?;
        if id != 0 {
            return Err(E_INVALIDARG.into());
        }
        Ok(self.stream.attributes().clone())
    }

    fn SetD3DManager(&self, _manager: Ref<IUnknown>) -> windows_core::Result<()> {
        // Frames are delivered in system memory, so there is nothing to do with a GPU device here.
        self.check()
    }
}

impl IMFGetService_Impl for Source_Impl {
    fn GetService(&self, _service: *const GUID, _riid: *const GUID, _ppv: *mut *mut core::ffi::c_void) -> windows_core::Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

impl IKsControl_Impl for Source_Impl {
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

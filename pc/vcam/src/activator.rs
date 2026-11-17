//! The object COM hands to Frame Server. It carries attributes (Frame Server sets some) and creates the media source.

use std::ffi::c_void;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows_core::{implement, IUnknown, Interface, Ref, BOOL, GUID, PCWSTR, PWSTR};

use crate::source::Source;

#[implement(IMFActivate)]
pub struct Activator {
    attrs: IMFAttributes,
}

impl Activator {
    pub fn create() -> windows_core::Result<IMFActivate> {
        let mut attrs = None;
        unsafe { MFCreateAttributes(&mut attrs, 4)? };
        Ok(Activator { attrs: attrs.unwrap() }.into())
    }
}

impl IMFActivate_Impl for Activator_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut c_void) -> windows_core::Result<()> {
        let source: IMFMediaSource = Source::create(&self.attrs)?;
        unsafe { source.query(riid, ppv).ok() }
    }

    fn ShutdownObject(&self) -> windows_core::Result<()> {
        Ok(())
    }

    fn DetachObject(&self) -> windows_core::Result<()> {
        Ok(())
    }
}

impl IMFAttributes_Impl for Activator_Impl {
    fn GetItem(&self, k: *const GUID, v: *mut PROPVARIANT) -> windows_core::Result<()> {
        unsafe { self.attrs.GetItem(k, if v.is_null() { None } else { Some(v) }) }
    }
    fn GetItemType(&self, k: *const GUID) -> windows_core::Result<MF_ATTRIBUTE_TYPE> {
        unsafe { self.attrs.GetItemType(k) }
    }
    fn CompareItem(&self, k: *const GUID, v: *const PROPVARIANT) -> windows_core::Result<BOOL> {
        unsafe { self.attrs.CompareItem(k, v) }
    }
    fn Compare(&self, theirs: Ref<IMFAttributes>, m: MF_ATTRIBUTES_MATCH_TYPE) -> windows_core::Result<BOOL> {
        unsafe { self.attrs.Compare(theirs.as_ref(), m) }
    }
    fn GetUINT32(&self, k: *const GUID) -> windows_core::Result<u32> {
        unsafe { self.attrs.GetUINT32(k) }
    }
    fn GetUINT64(&self, k: *const GUID) -> windows_core::Result<u64> {
        unsafe { self.attrs.GetUINT64(k) }
    }
    fn GetDouble(&self, k: *const GUID) -> windows_core::Result<f64> {
        unsafe { self.attrs.GetDouble(k) }
    }
    fn GetGUID(&self, k: *const GUID) -> windows_core::Result<GUID> {
        unsafe { self.attrs.GetGUID(k) }
    }
    fn GetStringLength(&self, k: *const GUID) -> windows_core::Result<u32> {
        unsafe { self.attrs.GetStringLength(k) }
    }
    fn GetString(&self, k: *const GUID, buf: PWSTR, cch: u32, len: *mut u32) -> windows_core::Result<()> {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(buf.0, cch as usize);
            self.attrs.GetString(k, slice, if len.is_null() { None } else { Some(len) })
        }
    }
    fn GetAllocatedString(&self, k: *const GUID, v: *mut PWSTR, len: *mut u32) -> windows_core::Result<()> {
        unsafe { self.attrs.GetAllocatedString(k, v, len) }
    }
    fn GetBlobSize(&self, k: *const GUID) -> windows_core::Result<u32> {
        unsafe { self.attrs.GetBlobSize(k) }
    }
    fn GetBlob(&self, k: *const GUID, buf: *mut u8, size: u32, len: *mut u32) -> windows_core::Result<()> {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(buf, size as usize);
            self.attrs.GetBlob(k, slice, if len.is_null() { None } else { Some(len) })
        }
    }
    fn GetAllocatedBlob(&self, k: *const GUID, buf: *mut *mut u8, size: *mut u32) -> windows_core::Result<()> {
        unsafe { self.attrs.GetAllocatedBlob(k, buf, size) }
    }
    fn GetUnknown(&self, k: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> windows_core::Result<()> {
        let unk: IUnknown = unsafe { self.attrs.GetUnknown(k)? };
        unsafe { unk.query(riid, ppv).ok() }
    }
    fn SetItem(&self, k: *const GUID, v: *const PROPVARIANT) -> windows_core::Result<()> {
        unsafe { self.attrs.SetItem(k, v) }
    }
    fn DeleteItem(&self, k: *const GUID) -> windows_core::Result<()> {
        unsafe { self.attrs.DeleteItem(k) }
    }
    fn DeleteAllItems(&self) -> windows_core::Result<()> {
        unsafe { self.attrs.DeleteAllItems() }
    }
    fn SetUINT32(&self, k: *const GUID, v: u32) -> windows_core::Result<()> {
        unsafe { self.attrs.SetUINT32(k, v) }
    }
    fn SetUINT64(&self, k: *const GUID, v: u64) -> windows_core::Result<()> {
        unsafe { self.attrs.SetUINT64(k, v) }
    }
    fn SetDouble(&self, k: *const GUID, v: f64) -> windows_core::Result<()> {
        unsafe { self.attrs.SetDouble(k, v) }
    }
    fn SetGUID(&self, k: *const GUID, v: *const GUID) -> windows_core::Result<()> {
        unsafe { self.attrs.SetGUID(k, v) }
    }
    fn SetString(&self, k: *const GUID, v: &PCWSTR) -> windows_core::Result<()> {
        unsafe { self.attrs.SetString(k, *v) }
    }
    fn SetBlob(&self, k: *const GUID, buf: *const u8, size: u32) -> windows_core::Result<()> {
        unsafe { self.attrs.SetBlob(k, std::slice::from_raw_parts(buf, size as usize)) }
    }
    fn SetUnknown(&self, k: *const GUID, v: Ref<IUnknown>) -> windows_core::Result<()> {
        unsafe { self.attrs.SetUnknown(k, v.as_ref()) }
    }
    fn LockStore(&self) -> windows_core::Result<()> {
        unsafe { self.attrs.LockStore() }
    }
    fn UnlockStore(&self) -> windows_core::Result<()> {
        unsafe { self.attrs.UnlockStore() }
    }
    fn GetCount(&self) -> windows_core::Result<u32> {
        unsafe { self.attrs.GetCount() }
    }
    fn GetItemByIndex(&self, i: u32, k: *mut GUID, v: *mut PROPVARIANT) -> windows_core::Result<()> {
        unsafe { self.attrs.GetItemByIndex(i, k, if v.is_null() { None } else { Some(v) }) }
    }
    fn CopyAllItems(&self, dest: Ref<IMFAttributes>) -> windows_core::Result<()> {
        unsafe { self.attrs.CopyAllItems(dest.as_ref()) }
    }
}

//! Tether Camera: a Windows 11 virtual camera.
//!
//! Windows' camera service (Frame Server) loads this DLL through COM when an app opens "Tether Camera". The DLL hands
//! out an `IMFActivate` that creates a live media source with one video stream. No kernel driver is involved.

#![allow(non_snake_case)]

mod activator;
mod pattern;
mod source;
mod stream;

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, ERROR_SUCCESS, E_POINTER, HINSTANCE, HMODULE, MAX_PATH, S_FALSE,
    S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, REG_SZ,
};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_core::{implement, IUnknown, Interface, Ref, BOOL, GUID, HRESULT, HSTRING, PCWSTR};

/// The COM class id of Tether Camera. Never change it: installed cameras are registered under it.
pub const CLSID_TETHER_CAMERA: GUID = GUID::from_u128(0x7a3c6e1b_4f2d_4b8a_9c61_2e5d8f0a1b37);

static MODULE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

#[no_mangle]
extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(hinst.0, Ordering::SeqCst);
    }
    BOOL(1)
}

#[implement(IClassFactory)]
struct ClassFactory;

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, riid: *const GUID, ppv: *mut *mut c_void) -> windows_core::Result<()> {
        if ppv.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppv = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let activate = activator::Activator::create()?;
        unsafe { activate.query(riid, ppv).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> windows_core::Result<()> {
        Ok(())
    }
}

#[no_mangle]
unsafe extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    if ppv.is_null() || rclsid.is_null() {
        return E_POINTER;
    }
    *ppv = std::ptr::null_mut();
    if *rclsid != CLSID_TETHER_CAMERA {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = ClassFactory.into();
    factory.query(riid, ppv)
}

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    // Frame Server keeps the DLL for the life of its process anyway; staying loaded is always safe.
    S_FALSE
}

fn clsid_key() -> String {
    format!("Software\\Classes\\CLSID\\{{{:?}}}", CLSID_TETHER_CAMERA)
}

fn module_path() -> windows_core::Result<String> {
    let mut buf = [0u16; MAX_PATH as usize * 4];
    let len = unsafe { GetModuleFileNameW(Some(HMODULE(MODULE.load(Ordering::SeqCst))), &mut buf) } as usize;
    if len == 0 || len >= buf.len() {
        return Err(windows_core::Error::from_thread());
    }
    Ok(String::from_utf16_lossy(&buf[..len]))
}

unsafe fn set_string(key: HKEY, name: Option<&str>, value: &str) -> windows_core::Result<()> {
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
    let name_h = name.map(HSTRING::from);
    let name_p = name_h.as_ref().map_or(PCWSTR::null(), |h| PCWSTR(h.as_ptr()));
    RegSetValueExW(key, name_p, None, REG_SZ, Some(bytes)).ok()
}

/// Registers the COM class machine-wide (Frame Server runs as a service and only sees HKLM). Needs admin.
#[no_mangle]
unsafe extern "system" fn DllRegisterServer() -> HRESULT {
    let result = (|| -> windows_core::Result<()> {
        let path = module_path()?;
        let mut key = HKEY::default();
        RegCreateKeyW(HKEY_LOCAL_MACHINE, &HSTRING::from(clsid_key()), &mut key).ok()?;
        let r = set_string(key, None, "Tether Camera");
        let _ = RegCloseKey(key);
        r?;
        let mut key = HKEY::default();
        RegCreateKeyW(HKEY_LOCAL_MACHINE, &HSTRING::from(clsid_key() + "\\InprocServer32"), &mut key).ok()?;
        let r = set_string(key, None, &path).and_then(|_| set_string(key, Some("ThreadingModel"), "Both"));
        let _ = RegCloseKey(key);
        r
    })();
    match result {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

#[no_mangle]
unsafe extern "system" fn DllUnregisterServer() -> HRESULT {
    let r = RegDeleteTreeW(HKEY_LOCAL_MACHINE, &HSTRING::from(clsid_key()));
    if r == ERROR_SUCCESS || r.0 == 2 {
        S_OK
    } else {
        r.to_hresult()
    }
}

/// Lets objects cross into the worker thread. Media Foundation objects used here are free-threaded.
pub(crate) struct SendCom<T>(pub T);
unsafe impl<T> Send for SendCom<T> {}
unsafe impl<T> Sync for SendCom<T> {}

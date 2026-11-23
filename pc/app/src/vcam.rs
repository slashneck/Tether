//! Turns "Tether Camera" on for as long as the app runs, and repairs its Windows registration.

use std::path::PathBuf;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
use windows::Win32::Media::MediaFoundation::{
    IMFVirtualCamera, MFCreateVirtualCamera, MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime_Session,
    MFVirtualCameraType_SoftwareCameraSource,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows_core::GUID;

/// Must match `CLSID_TETHER_CAMERA` in the vcam crate.
const CLSID_TETHER_CAMERA: GUID = GUID::from_u128(0x7a3c6e1b_4f2d_4b8a_9c61_2e5d8f0a1b37);
const DLL_NAME: &str = "tether_vcam.dll";

pub struct VirtualCamera(IMFVirtualCamera);

impl VirtualCamera {
    pub fn start() -> windows_core::Result<VirtualCamera> {
        unsafe {
            let camera = MFCreateVirtualCamera(
                MFVirtualCameraType_SoftwareCameraSource,
                MFVirtualCameraLifetime_Session,
                MFVirtualCameraAccess_CurrentUser,
                &HSTRING::from("Tether Camera"),
                &HSTRING::from(format!("{{{:?}}}", CLSID_TETHER_CAMERA)),
                None,
            )?;
            camera.Start(None)?;
            Ok(VirtualCamera(camera))
        }
    }
}

impl Drop for VirtualCamera {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Remove();
            let _ = self.0.Shutdown();
        }
    }
}

/// The camera DLL that ships next to tether.exe.
fn our_dll() -> Option<PathBuf> {
    let dll = std::env::current_exe().ok()?.parent()?.join(DLL_NAME);
    dll.is_file().then_some(dll)
}

/// The DLL Windows loads for Tether Camera, if it's registered at all.
fn registered_dll() -> Option<PathBuf> {
    let key = HSTRING::from(format!("SOFTWARE\\Classes\\CLSID\\{{{:?}}}\\InProcServer32", CLSID_TETHER_CAMERA));
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let err = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &key,
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if err.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

/// True when Windows can't load Tether Camera: not registered, or registered to a file that's gone.
/// (Something on the PC can remove the registration; Tether then looks fine but no app sees the camera.)
pub fn needs_repair() -> bool {
    our_dll().is_some() && !registered_dll().is_some_and(|p| p.is_file())
}

/// Registers our camera DLL again. Shows one Windows admin prompt.
pub fn repair() -> Result<(), String> {
    let dll = our_dll().ok_or("Tether Camera's file is missing. Reinstall Tether.")?;
    let params = HSTRING::from(format!("/s \"{}\"", dll.display()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: w!("regsvr32.exe"),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            "Repair needs admin rights. Click Repair again and choose Yes.".into()
        } else {
            format!("Windows couldn't start the repair ({}).", e.code())
        });
    }
    let mut code = 1u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    match code {
        0 if !needs_repair() => Ok(()),
        0 => Err("Windows still doesn't list Tether Camera. Reinstall Tether.".into()),
        c => Err(format!("Windows didn't accept Tether Camera (error {c}). Reinstall Tether.")),
    }
}

//! Turns "Tether Camera" on for as long as the app runs.

use windows::Win32::Media::MediaFoundation::{
    IMFVirtualCamera, MFCreateVirtualCamera, MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime_Session,
    MFVirtualCameraType_SoftwareCameraSource,
};
use windows_core::{GUID, HSTRING};

/// Must match `CLSID_TETHER_CAMERA` in the vcam crate.
const CLSID_TETHER_CAMERA: GUID = GUID::from_u128(0x7a3c6e1b_4f2d_4b8a_9c61_2e5d8f0a1b37);

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

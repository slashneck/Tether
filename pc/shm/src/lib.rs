//! The shared frame buffer between the Tether app (writer) and the Tether Camera DLL (reader).
//!
//! The DLL runs inside Windows' camera service, in session 0, so the buffer lives in the `Global\` namespace. Only
//! services may create objects there, so the **DLL creates** it (with a DACL that lets the signed-in user write) and
//! the app opens it. The app keeps trying to open it, so start order doesn't matter.
//!
//! Layout: a 4 KiB header, then two frame slots. The writer fills the slot the reader isn't looking at, then publishes
//! it. A reader copies the published slot and checks the writer didn't lap it meanwhile.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL, INVALID_HANDLE_VALUE};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows_core::{w, BOOL};

pub const NAME: windows_core::PCWSTR = w!("Global\\TetherCamera.Frames.v1");
const MAGIC: u32 = u32::from_le_bytes(*b"TTHR");
const VERSION: u32 = 1;

pub const MAX_WIDTH: u32 = 3840;
pub const MAX_HEIGHT: u32 = 2160;
pub const SLOT_BYTES: usize = (MAX_WIDTH * MAX_HEIGHT * 3 / 2) as usize;
const HEADER_BYTES: usize = 4096;
pub const TOTAL_BYTES: usize = HEADER_BYTES + 2 * SLOT_BYTES;

/// Frames older than this mean the app stopped writing (closed or crashed); the DLL then shows its own card.
pub const WRITER_TIMEOUT_MS: u64 = 1500;

#[repr(C)]
struct SlotMeta {
    width: AtomicU32,
    height: AtomicU32,
}

#[repr(C)]
struct Header {
    magic: AtomicU32,
    version: AtomicU32,
    /// Bumped after each frame is complete; also tells which slot is newest (count % 2).
    frame_count: AtomicU64,
    slots: [SlotMeta; 2],
    /// GetTickCount64 of the writer's last frame or keep-alive.
    writer_alive_ms: AtomicU64,
    /// GetTickCount64 of the last time an app pulled frames from Tether Camera ("in use").
    reader_alive_ms: AtomicU64,
    /// The format the app using the camera asked for. The writer may use it to pick the phone's resolution.
    want_width: AtomicU32,
    want_height: AtomicU32,
    want_fps: AtomicU32,
}

pub struct Shm {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

unsafe impl Send for Shm {}
unsafe impl Sync for Shm {}

pub fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

impl Shm {
    /// DLL side: create the buffer (or open it if another camera instance already did).
    pub fn create() -> windows_core::Result<Shm> {
        unsafe {
            // Everyone and SYSTEM get full access; low integrity label so the user's app can write too.
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:(A;;GA;;;WD)(A;;GA;;;SY)S:(ML;;NW;;;LW)"),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )?;
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: BOOL(0),
            };
            let size = TOTAL_BYTES as u64;
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                Some(&sa),
                PAGE_READWRITE,
                (size >> 32) as u32,
                size as u32,
                NAME,
            );
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            let shm = Self::map(handle?)?;
            let h = shm.header();
            if h.magic.load(Ordering::Acquire) != MAGIC {
                h.version.store(VERSION, Ordering::Relaxed);
                h.magic.store(MAGIC, Ordering::Release);
            }
            Ok(shm)
        }
    }

    /// App side: open the buffer the DLL created. Fails until an app has opened Tether Camera at least once since boot.
    pub fn open() -> windows_core::Result<Shm> {
        unsafe {
            let handle = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, NAME)?;
            let shm = Self::map(handle)?;
            if shm.header().magic.load(Ordering::Acquire) != MAGIC
                || shm.header().version.load(Ordering::Relaxed) != VERSION
            {
                return Err(windows::Win32::Foundation::E_UNEXPECTED.into());
            }
            Ok(shm)
        }
    }

    unsafe fn map(handle: HANDLE) -> windows_core::Result<Shm> {
        let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, TOTAL_BYTES);
        if view.Value.is_null() {
            let e = windows_core::Error::from_thread();
            let _ = CloseHandle(handle);
            return Err(e);
        }
        Ok(Shm { handle, view })
    }

    fn header(&self) -> &Header {
        unsafe { &*(self.view.Value as *const Header) }
    }

    fn slot_ptr(&self, slot: usize) -> *mut u8 {
        unsafe { (self.view.Value as *mut u8).add(HEADER_BYTES + slot * SLOT_BYTES) }
    }

    // ---------- writer (app) ----------

    /// Publishes one NV12 frame (`width * height * 3 / 2` bytes, tightly packed).
    pub fn write_frame(&self, nv12: &[u8], width: u32, height: u32) {
        let len = (width * height * 3 / 2) as usize;
        if width > MAX_WIDTH || height > MAX_HEIGHT || nv12.len() < len {
            return;
        }
        let h = self.header();
        let count = h.frame_count.load(Ordering::Acquire);
        let slot = ((count + 1) % 2) as usize;
        unsafe { std::ptr::copy_nonoverlapping(nv12.as_ptr(), self.slot_ptr(slot), len) };
        h.slots[slot].width.store(width, Ordering::Relaxed);
        h.slots[slot].height.store(height, Ordering::Relaxed);
        h.writer_alive_ms.store(now_ms(), Ordering::Relaxed);
        h.frame_count.store(count + 1, Ordering::Release);
    }

    /// Lets the DLL know the app is alive even when no frame was written lately.
    pub fn keep_alive(&self) {
        self.header().writer_alive_ms.store(now_ms(), Ordering::Relaxed);
    }

    /// True while some app is pulling frames from Tether Camera.
    pub fn in_use(&self) -> bool {
        now_ms().saturating_sub(self.header().reader_alive_ms.load(Ordering::Relaxed)) < 1000
    }

    /// The format the app using Tether Camera negotiated: (width, height, fps), or None if nobody did yet.
    pub fn wanted_format(&self) -> Option<(u32, u32, u32)> {
        let h = self.header();
        let (w, ht, f) = (
            h.want_width.load(Ordering::Relaxed),
            h.want_height.load(Ordering::Relaxed),
            h.want_fps.load(Ordering::Relaxed),
        );
        (w > 0 && ht > 0).then_some((w, ht, f))
    }

    // ---------- reader (DLL) ----------

    pub fn set_wanted_format(&self, width: u32, height: u32, fps: u32) {
        let h = self.header();
        h.want_width.store(width, Ordering::Relaxed);
        h.want_height.store(height, Ordering::Relaxed);
        h.want_fps.store(fps, Ordering::Relaxed);
    }

    pub fn mark_reading(&self) {
        self.header().reader_alive_ms.store(now_ms(), Ordering::Relaxed);
    }

    pub fn writer_alive(&self) -> bool {
        now_ms().saturating_sub(self.header().writer_alive_ms.load(Ordering::Relaxed)) < WRITER_TIMEOUT_MS
    }

    pub fn frame_count(&self) -> u64 {
        self.header().frame_count.load(Ordering::Acquire)
    }

    /// Copies the newest frame into `out` (resized as needed). Returns (width, height, frame_count).
    pub fn read_latest(&self, out: &mut Vec<u8>) -> Option<(u32, u32, u64)> {
        let h = self.header();
        for _ in 0..4 {
            let count = h.frame_count.load(Ordering::Acquire);
            if count == 0 {
                return None;
            }
            let slot = (count % 2) as usize;
            let (w, ht) = (h.slots[slot].width.load(Ordering::Relaxed), h.slots[slot].height.load(Ordering::Relaxed));
            if w == 0 || ht == 0 || w > MAX_WIDTH || ht > MAX_HEIGHT {
                return None;
            }
            let len = (w * ht * 3 / 2) as usize;
            out.resize(len, 0);
            unsafe { std::ptr::copy_nonoverlapping(self.slot_ptr(slot), out.as_mut_ptr(), len) };
            // The writer only reuses this slot two frames later; if that happened while copying, try again.
            if h.frame_count.load(Ordering::Acquire) <= count + 1 {
                return Some((w, ht, count));
            }
        }
        None
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Nearest-neighbour NV12 resize, used when the phone's frame size differs from what the camera app asked for.
/// Keeps the aspect ratio: letterboxes/pillarboxes with black.
pub fn scale_nv12(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize) {
    if sw == dw && sh == dh {
        dst[..dw * dh * 3 / 2].copy_from_slice(&src[..sw * sh * 3 / 2]);
        return;
    }
    dst[..dw * dh].fill(16);
    dst[dw * dh..dw * dh * 3 / 2].fill(128);
    // Fit inside dst, even dimensions so chroma stays aligned.
    let (fw, fh) = if sw * dh > sh * dw { (dw, (sh * dw / sw) & !1) } else { ((sw * dh / sh) & !1, dh) };
    let (ox, oy) = (((dw - fw) / 2) & !1, ((dh - fh) / 2) & !1);
    let xmap: Vec<usize> = (0..fw).map(|x| x * sw / fw).collect();
    for y in 0..fh {
        let sy = y * sh / fh;
        let srow = &src[sy * sw..sy * sw + sw];
        let drow = &mut dst[(oy + y) * dw + ox..(oy + y) * dw + ox + fw];
        for (d, &sx) in drow.iter_mut().zip(&xmap) {
            *d = srow[sx];
        }
    }
    let (suv, duv) = (&src[sw * sh..], &mut dst[dw * dh..]);
    for y in 0..fh / 2 {
        let sy = y * sh / fh;
        for x in 0..fw / 2 {
            let sx = (x * sw / fw) & !0;
            let si = sy * sw + sx * 2;
            let di = (oy / 2 + y) * dw + ox + x * 2;
            duv[di] = suv[si];
            duv[di + 1] = suv[si + 1];
        }
    }
}

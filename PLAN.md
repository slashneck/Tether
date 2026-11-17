# Tether — plan

Use your Android phone as a webcam on Windows 11. USB first, Wi‑Fi second. Light on CPU, never drops.

## Scope (v1)

- Android phone → Windows 11 PC
- Video only (no mic yet)
- USB cable (main path) and Wi‑Fi (same network)
- Shows up in every app as a normal webcam called **Tether Camera**
- GitHub releases: installer, portable zip, checksums, in-app updater (same setup as MangaCut / Better Comfy)

## Getting the app onto the phone (two ways)

1. **Download the APK** from the GitHub release page on the phone and install it.
2. **Install from the PC**: plug the phone in (USB debugging on), click "Install on phone" in the desktop app. The
   bundled adb runs `adb install -r` with the APK that ships inside the PC app. The same button updates the phone app
   when the PC app has a newer version.

## Little extras (character, still minimal)

- **Flashlight** toggle (phone torch) from PC and phone
- Resolution / fps / front–back / lens switch live, without reconnecting
- **"Connection lost" screen**: when the phone drops, Discord & co. see a custom GIF/image/video loop instead of a
  frozen frame or black. Ships with a few built-ins; drop your own GIF in Settings. The PC app decodes it and feeds the
  virtual camera, so the camera DLL stays tiny.
- Mirror, pause ("freeze" on the last frame or show the pause GIF), and a privacy "cover" button that swaps in the GIF instantly
- Keep each one a single toggle; nothing buried in menus

## Do we need a driver?

Not a kernel driver. Windows 11 has a built-in **virtual camera API** (`MFCreateVirtualCamera`). We ship a small
user-mode DLL (a "media source") that Windows' own camera service loads. To apps it looks exactly like a real webcam:
Discord, OBS, Zoom, Teams, browsers. No driver signing, no test mode, no reboot. The installer registers the DLL, the
uninstaller removes it.

(Iriun uses an older DirectShow-style approach. That's one reason it's clunkier: some apps don't see it, and it
copies frames around more.)

## Architecture

```
 PHONE (Kotlin)                          PC (Rust)
 ┌────────────────────┐   USB (adb       ┌──────────────────────┐   shared   ┌───────────────────────┐
 │ CameraX / Camera2  │   forward) or    │ tether.exe           │   memory   │ tether_vcam.dll       │
 │   → MediaCodec     │ ─ Wi‑Fi TCP ───▶ │  receive → D3D11/MF  │ ─ NV12 ──▶ │ virtual camera source │──▶ Discord, OBS, …
 │     H.264 (HW)     │                  │  HW decode → NV12    │  ring buf  │ (loaded by Windows)   │
 │ foreground service │ ◀── control ──── │  UI (egui/wgpu)      │            └───────────────────────┘
 └────────────────────┘                  └──────────────────────┘
```

### Phone app — `android/`
- Kotlin + Jetpack Compose, minSdk 26
- Camera2 → `MediaCodec` hardware H.264 encoder via an input Surface (no CPU copies)
- Runs as a **foreground service** with Wi‑Fi lock + partial wake lock, so Android doesn't kill it
- "Screen off while streaming" to save battery and heat
- Listens on a TCP port; announces itself on Wi‑Fi via mDNS (`_tether._tcp`)
- Release: signed APK attached to the GitHub release (Play Store later, maybe)

### PC app — `pc/` (Rust workspace)
- `tether` (the app): egui on wgpu, small native window, tray icon
  - **USB**: bundled `adb.exe` runs `adb forward tcp:… tcp:…`, then a plain TCP socket. Survives unplug/replug.
  - **Wi‑Fi**: mDNS discovery, one-click connect
  - Hardware H.264 decode via Media Foundation (D3D11) → NV12
  - Heartbeat every 500 ms; auto-reconnect with backoff; the last frame stays on screen during a hiccup instead of black
- `tether-vcam` (the DLL): COM media source implemented with `windows-rs`; reads NV12 frames from a named
  shared-memory ring buffer the app writes. When no phone is connected it shows a clean "Tether — no phone" frame.
- `tether-proto`: the wire format, shared by both sides (documented so the Kotlin side matches)

### Wire protocol (v1)
Length-prefixed messages over one TCP connection:
`HELLO` (version, device name) · `CONFIG` (camera, resolution, fps, bitrate) · `VIDEO` (H.264 Annex‑B access unit + timestamp)
· `PING`/`PONG` · `KEYFRAME_REQUEST`. TCP_NODELAY; the receiver drops stale frames instead of building up latency.

### Release — same as your other apps
- `build-release.ps1` → `TetherSetup-<v>.exe` (+ `.sha256`), `Tether-<v>-win-x64.zip` (+ `.sha256`), `Tether-<v>.apk`, source zip
- Setup: the C# .NET 8 setup from MangaCut, adapted. It also registers/unregisters `tether_vcam.dll`
- Updater: checks `slashneck/Tether` latest release, verifies SHA‑256, runs setup in update mode
- `LICENSE`, `THIRD-PARTY-NOTICES.md` (adb is Apache‑2.0)

## Milestones

1. **Virtual camera spike**: the DLL shows a test pattern as "Tether Camera" in the Windows Camera app and Discord. (Riskiest part, so it goes first.)
2. **Phone streams over USB**: Android app encodes H.264 and the PC decodes and feeds it to the virtual camera.
3. **Reliability**: reconnect, heartbeat, unplug/replug, phone sleep, PC sleep.
4. **Wi‑Fi**: mDNS discovery + connect.
5. **UI**: both apps to the mockup.
6. **Release**: icon, installer, updater, README, first GitHub release.

## Targets
- End-to-end latency over USB: < 60 ms at 1080p30
- PC CPU while streaming 1080p30: < 3% on a mid-range machine
- Zero manual reconnects after cable wiggles or Wi‑Fi blips

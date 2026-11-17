<p align="center">
  <img src="docs/banner.png" alt="Tether" width="100%">
</p>

<p align="center">
  <a href="https://github.com/slashneck/Tether/releases/latest"><img src="https://img.shields.io/github/v/release/slashneck/Tether?style=flat-square&label=release&color=f4f4f4&labelColor=161616" alt="Latest release"></a>
  <a href="https://github.com/slashneck/Tether/releases"><img src="https://img.shields.io/github/downloads/slashneck/Tether/total?style=flat-square&color=f4f4f4&labelColor=161616" alt="Downloads"></a>
  <img src="https://img.shields.io/badge/Windows-11-f4f4f4?style=flat-square&labelColor=161616" alt="Windows 11">
  <img src="https://img.shields.io/badge/Android-8%2B-f4f4f4?style=flat-square&labelColor=161616" alt="Android 8 and newer">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-f4f4f4?style=flat-square&labelColor=161616" alt="GPL 3.0"></a>
</p>

<p align="center">
  <a href="https://github.com/slashneck/Tether/releases/latest"><b>Download for Windows</b></a>
  &nbsp;&nbsp;|&nbsp;&nbsp;
  <a href="#getting-started">Getting started</a>
  &nbsp;&nbsp;|&nbsp;&nbsp;
  <a href="#features">Features</a>
  &nbsp;&nbsp;|&nbsp;&nbsp;
  <a href="#building">Build it yourself</a>
</p>

<br>

Tether is a webcam tool for Windows. It adds one camera, **Tether Camera**, that Discord, OBS, Zoom, Teams and your
browser see like any other, and puts everything you wish your webcam could do in front of it: a hotkey that covers you
instantly, zoom and framing, filters, rotate and mirror. Feed it your normal webcam, or your Android phone over a
USB cable with the free Tether phone app. Pick Tether Camera once in each app; switching sources, covering yourself or
changing the look happens in Tether, and your apps never notice.

## Getting started

1. **Install Tether.** Download `TetherSetup-x.y.z.exe` from the
   [latest release](https://github.com/slashneck/Tether/releases/latest) and run it. Windows asks for admin rights once,
   because Tether Camera is added for the whole PC.
2. **Pick a source** in Tether: any webcam on your PC, or your phone.
3. **Pick "Tether Camera"** in your app. Done.

**Using your phone?** Turn on USB debugging (Settings → About phone → tap *Build number* seven times, then Developer
options → *USB debugging*), plug it in and tap *Allow*. Then *Setup* → *Install on phone* in Tether puts the app on it,
and keeps it updated. Prefer doing it yourself? Download `Tether-x.y.z.apk` from the release page on your phone.

## Features

**One camera, any source.** Your webcam or your phone, switched in Tether while your apps keep seeing the same Tether
Camera. No re-picking cameras in Discord mid-call.

**Cover hotkey.** One global shortcut (Ctrl+Shift+H by default) swaps your camera for a picture or GIF of your choice,
instantly, in every app at once. The same cover shows if the source drops, instead of a frozen face.

**Zoom, framing and filters.** Zoom in and pan, fill/fit/stretch the frame, rotate and mirror, plus B&W, Noir, Warm,
Cool, Vivid and Fade filters and brightness, contrast and saturation sliders. The preview shows exactly what your apps
get.

**No driver.** Tether Camera uses Windows 11's built-in virtual camera support. Nothing runs in the kernel, there's no
driver to sign, and uninstalling leaves nothing behind.

**Your phone as a webcam, done right.** Over USB: fast, private (nothing on your Wi‑Fi can connect) and immune to Wi‑Fi
drops. The phone encodes on its own hardware, the PC decodes with Windows' built-in decoder, and a heartbeat reconnects
by itself within seconds, so a wiggled cable is a short freeze, not a restart.

**The phone stays out of the way.** Back or front camera, resolution, frame rate, quality and the flashlight are set from
the PC. On the phone: a live preview you can switch off to save battery, and a screen that dims to minimum while
streaming. Quitting Tether on the PC closes the app on the phone too.

**Updates itself.** Tether checks GitHub for a new version (you can switch that off) and installs it after checking its
SHA-256. The phone app comes with the PC app, and Tether offers to update the phone when it's behind.

## Requirements

- Windows 11 (virtual cameras arrived with Windows 11)
- A webcam, or an Android phone with Android 8 or newer and a USB cable that carries data

## Privacy

Tether sends nothing anywhere. The video stays on your PC (and on the cable from your phone). The only
network request Tether makes on its own is asking GitHub for the newest release, which you can switch off under
*Setup → About*.

## Building

You need Rust (MSVC toolchain), the Android SDK with a JDK 17 or newer, and the .NET 8 SDK.

```
pc\        Windows app (tether.exe) and Tether Camera (tether_vcam.dll), Rust
android\   phone app, Kotlin, no third-party libraries
setup\     the installer, C# / .NET Framework 4.8
docs\      wire protocol and plan
```

- `cd pc && cargo build --release` builds the Windows app and the camera DLL.
- `scripts\dev-install-vcam.ps1` registers the freshly built camera DLL for testing (asks for admin).
- `cd android && .\gradlew.bat assembleDebug` builds the phone app.
- `build-release.ps1` builds everything into `release\`: the setup, the APK, their checksums and a source zip. Release
  APKs are signed with a key kept outside the repo; without it, the APK is signed with the debug key.

The phone and the PC talk over a small protocol documented in [docs/PROTOCOL.md](docs/PROTOCOL.md).

## License

Tether is free software under the [GNU General Public License v3.0](LICENSE). It includes adb from the Android Open Source
Project; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

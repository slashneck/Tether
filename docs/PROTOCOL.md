# Tether wire protocol (v1)

One TCP connection. **The phone listens** on port `27183`; the PC connects.

- USB: the PC runs `adb forward tcp:27183 tcp:27183` and connects to `127.0.0.1:27183`.
- Wi‑Fi: the PC connects to `<phone ip>:27183`.

Both ends set `TCP_NODELAY`.

## Framing

Every message: `type: u8`, `length: u32` (big-endian, payload bytes), `payload`.

## Phone → PC

| type | name | payload |
|---|---|---|
| `0x01` | HELLO | UTF‑8 JSON: `{"proto":1,"app":"0.1.0","device":"Pixel 8","cameras":[{"id":"0","facing":"back"},…],"torch":true}` |
| `0x02` | VIDEO | `flags: u8`, `pts_us: i64` BE, H.264 Annex‑B data. flags bit 0 = keyframe, bit 1 = codec config (SPS/PPS only) |
| `0x03` | STATUS | UTF‑8 JSON: `{"camera":"0","width":1920,"height":1080,"fps":30,"torch":false,"battery":81}` |
| `0x11` | PONG | the 8 bytes of the PING it answers |

## PC → phone

| type | name | payload |
|---|---|---|
| `0x81` | SETTINGS | UTF‑8 JSON, any subset: `{"camera":"0","width":1920,"height":1080,"fps":30,"bitrate":8000000,"torch":false}` |
| `0x82` | KEYFRAME | empty: send a keyframe as soon as possible |
| `0x83` | BYE | empty: the PC app is quitting. The phone stops the camera at once, ends its service and closes the app |
| `0x90` | PING | 8 bytes (PC clock, opaque to the phone) |

## Liveness

The PC pings every 500 ms. Either side treats 3 s without any message as a dead connection, closes it, and the PC
reconnects (every 500 ms, forever). The phone keeps its camera open for 10 s after a drop, so a quick replug resumes
instantly; after that it releases the camera.

## Rules

- The phone sends HELLO first, then codec config, then a keyframe.
- Unknown message types are skipped (forward compatibility). A HELLO with a higher `proto` major closes the connection
  with a clear message on both sides.

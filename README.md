# Qcast

Cross-platform, reliability-first desktop screencast. One machine captures its desktop,
encodes it, and streams it to another machine that decodes the frames into **our own
pipeline** (custom rendering) — not a locked fullscreen viewer.

Built on **GStreamer `webrtcbin`** (Rust / `gstreamer-rs`). WebRTC gives us NAT traversal,
congestion control, loss recovery, and encryption for free — the proven equivalent of the
hand-rolled RTP+FEC stack Sunshine/Moonlight maintains.

## Two connection modes — both always available

| | LAN mode | Web mode |
|---|---|---|
| Signaling | direct IP / mDNS (no server) | remote WebSocket on your private server |
| ICE | host candidates only | STUN + coturn TURN (relay fallback) |
| Server needed? | no | yes |
| Latency | lowest (direct) | RTT-bound |

## Workspace

- `crates/qcast-core` — shared: signaling protocol, connection modes, element selection, webrtc helpers
- `crates/qcast-sender` — capture → encode → `webrtcbin`
- `crates/qcast-receiver` — `webrtcbin` → decode → render (minimal window first)
- `crates/qcast-server` — WebSocket signaling for Web mode (pairs with coturn)

## Codec / hardware

H.264 by default (universal, low cost). Encoder is chosen at runtime, hardware first with a
software fallback: `nvh264enc` (NVENC) / `vah264lpenc` (Intel VAAPI) / `qsvh264enc` / `vtenc_h264`
→ `openh264enc` / `x264enc`. Same idea for decode.

## Build

Requires GStreamer 1.24+ with dev headers and an H.264 encoder/decoder plugin.
See the dependency install notes for your platform; on the Fedora dev box this is
`gstreamer1-{devel,plugins-base-devel,plugins-bad-free-devel}` + `gstreamer1-plugin-openh264`
(software) and the NVIDIA `nvcodec` plugin (hardware, via the NVIDIA driver).

```bash
cargo build
cargo run -p qcast-sender    # prints the selected capture source + encoder
```

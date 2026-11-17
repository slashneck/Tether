//! "Phone (browser)": any phone's camera through the Tether web page on Patchbay (iPhone included).
//!
//! Pairing: we make a random code and show a QR code for `<web>/tether#<code>`. The page in the phone's browser
//! posts its WebRTC offer under that code to Patchbay (/api/tether/pair); we pick it up, answer, and from then on the
//! video comes straight from the phone over the local network. Patchbay only relays the two short descriptions.
//!
//! WebRTC runs on its own thread with its own small tokio runtime; the engine only sees `Event`s.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rtc::interceptor::Registry;
use rtc::media::io::sample_builder::SampleBuilder;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::{MediaEngine, MIME_TYPE_H264};
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc::rtp::codec::h264::H264Packet;
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters, RtpCodecKind};
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use rtc::ice::mdns::MulticastDnsMode;
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState,
    SettingEngineBuilder,
};
use webrtc::runtime::{default_runtime, Runtime};

/// Where the Tether web page and the pairing API live.
pub const WEB_BASE: &str = "https://patchbay-sage.vercel.app";
/// A code is good for this long; then a fresh QR code is shown (the server forgets pairings after 10 minutes).
const CODE_LIFETIME: Duration = Duration::from_secs(8 * 60);

pub enum Event {
    /// Show this link as a QR code: the phone opens it to pair.
    Waiting { url: String },
    Connected,
    /// One H.264 access unit (Annex B).
    Frame(Vec<u8>),
    /// The phone went away; a new code follows.
    Lost(String),
}

pub struct BrowserLink {
    pub events: Receiver<Event>,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
}

impl BrowserLink {
    pub fn start() -> BrowserLink {
        clean_logs();
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(false));
        let (s, k) = (stop.clone(), keyframe.clone());
        std::thread::Builder::new()
            .name("tether-browser".into())
            .spawn(move || run(tx, s, k))
            .expect("browser thread");
        BrowserLink { events: rx, stop, keyframe }
    }

    /// Ask the phone for a keyframe (after a decode error).
    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }
}

impl Drop for BrowserLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Only with TETHER_WIFI_LOG=1 in the environment: pairing steps (including local network addresses) for debugging.
fn log_enabled() -> bool {
    std::env::var("TETHER_WIFI_LOG").is_ok_and(|v| v == "1")
}

fn log_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        paths.push(std::path::PathBuf::from(local).join("Tether").join("wifi.log"));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        if !dir.to_string_lossy().to_lowercase().contains("program files") {
            paths.push(dir.join("wifi.log"));
        }
    }
    paths
}

/// Logging is off: remove logs an earlier debug run left behind.
fn clean_logs() {
    if !log_enabled() {
        for p in log_paths() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Pairing steps go to %LOCALAPPDATA%\Tether\wifi.log (and next to tether.exe in a dev build), when enabled.
pub fn log(msg: &str) {
    use std::io::Write;
    if !log_enabled() {
        return;
    }
    let line = format!("{:?} {msg}\n", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    for p in log_paths() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

fn new_code() -> String {
    const ABC: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
    let mut bytes = [0u8; 22];
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|b| ABC[*b as usize % ABC.len()] as char).collect()
}

/// The PC's address on the local network (no packet is sent: connecting a UDP socket only picks the route).
fn local_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "0.0.0.0".into())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build().into()
}

/// The phone's offer for this code, once it posted one.
fn fetch_offer(code: &str) -> Option<String> {
    let url = format!("{WEB_BASE}/api/tether/pair?code={code}&want=offer");
    let mut r = match agent().get(&url).header("User-Agent", "Tether").call() {
        Ok(r) => r,
        Err(e) => {
            log(&format!("asking for the offer failed: {e}"));
            return None;
        }
    };
    let v: serde_json::Value = serde_json::from_str(&r.body_mut().read_to_string().ok()?).ok()?;
    v["offer"].as_str().map(String::from)
}

fn post_answer(code: &str, answer: &str) -> Result<(), String> {
    let body = serde_json::json!({ "code": code, "answer": answer }).to_string();
    agent()
        .post(&format!("{WEB_BASE}/api/tether/pair"))
        .header("Content-Type", "application/json")
        .header("User-Agent", "Tether")
        .send(body.as_bytes())
        .map(|_| ())
        .map_err(|e| format!("Couldn't answer the phone ({e})"))
}

fn run(tx: Sender<Event>, stop: Arc<AtomicBool>, keyframe: Arc<AtomicBool>) {
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(rt) => rt,
        Err(_) => return,
    };
    while !stop.load(Ordering::Relaxed) {
        // 1. A fresh code and its QR link; wait for the phone's offer.
        let code = new_code();
        if tx.send(Event::Waiting { url: format!("{WEB_BASE}/tether#{code}") }).is_err() {
            return;
        }
        let born = Instant::now();
        let mut offer = None;
        while offer.is_none() && born.elapsed() < CODE_LIFETIME {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            offer = fetch_offer(&code);
            if offer.is_none() {
                std::thread::sleep(Duration::from_millis(1200));
            }
        }
        let Some(offer) = offer else { continue };
        log(&format!("got the phone's offer ({} bytes)", offer.len()));

        // 2. Answer and stream until the phone goes away.
        let reason = rt.block_on(session(&code, offer, &tx, &stop, &keyframe));
        log(&format!("session ended: {reason}"));
        if tx.send(Event::Lost(reason)).is_err() {
            return;
        }
    }
}

struct Handler {
    runtime: Arc<dyn Runtime>,
    gathered: tokio::sync::mpsc::Sender<()>,
    state: tokio::sync::mpsc::Sender<RTCPeerConnectionState>,
    frames: Sender<Event>,
    keyframe: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.try_send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state.try_send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        if track.kind().await != RtpCodecKind::Video {
            return;
        }
        let Some(ssrc) = track.ssrcs().await.first().copied() else { return };

        // Keyframes on request (and one right away, so the picture starts at once).
        self.keyframe.store(true, Ordering::Relaxed);
        let (kf, pli_track) = (self.keyframe.clone(), track.clone());
        self.runtime.spawn(Box::pin(async move {
            loop {
                if kf.swap(false, Ordering::Relaxed) {
                    let pli = PictureLossIndication { sender_ssrc: 0, media_ssrc: ssrc };
                    if pli_track.write_rtcp(vec![Box::new(pli)]).await.is_err() {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }));

        // RTP packets -> whole H.264 frames -> engine.
        let frames = self.frames.clone();
        self.runtime.spawn(Box::pin(async move {
            let mut builder = SampleBuilder::new(256, H264Packet::default(), 90_000).with_max_time_delay(Duration::from_millis(500));
            while let Some(evt) = track.poll().await {
                match evt {
                    TrackRemoteEvent::OnRtpPacket(packet) => {
                        let now = Instant::now();
                        builder.push(now, packet);
                        while let Some(sample) = builder.pop(now) {
                            if frames.send(Event::Frame(sample.data.to_vec())).is_err() {
                                return;
                            }
                        }
                    }
                    TrackRemoteEvent::OnEnded => break,
                    _ => {}
                }
            }
        }));
    }
}

/// When the phone's local IPv4 addresses are all in other networks than this PC's (e.g. a guest Wi‑Fi), a message
/// that says so plainly.
fn other_network_hint(sdp: &str, ours: &str) -> Option<String> {
    let net = |ip: &str| ip.rsplitn(2, '.').nth(1).map(String::from);
    let ours_net = net(ours)?;
    let phone: Vec<String> = sdp
        .lines()
        .filter_map(|l| l.strip_prefix("a=candidate:"))
        .filter_map(|c| c.split_whitespace().nth(4))
        .filter(|ip| ip.parse::<std::net::Ipv4Addr>().is_ok_and(|a| a.is_private()))
        .map(String::from)
        .collect();
    if phone.is_empty() || phone.iter().any(|ip| net(ip).as_deref() == Some(ours_net.as_str())) {
        return None;
    }
    Some(format!(
        "Your phone ({}) is on a different network than this PC ({ours}), maybe a guest Wi‑Fi. Connect it to the same network as the PC.",
        phone[0]
    ))
}

/// "udp 192.168.1.5 host, udp abcd.local host, …" from an SDP's candidate lines (for the log).
fn candidates(sdp: &str) -> String {
    let list: Vec<String> = sdp
        .lines()
        .filter_map(|l| l.strip_prefix("a=candidate:"))
        .map(|c| {
            let f: Vec<&str> = c.split_whitespace().collect();
            let typ = f.iter().position(|x| *x == "typ").and_then(|i| f.get(i + 1)).copied().unwrap_or("?");
            format!("{} {} {}", f.get(2).unwrap_or(&"?"), f.get(4).unwrap_or(&"?"), typ)
        })
        .collect();
    if list.is_empty() { "none".into() } else { list.join(", ") }
}

fn h264(payload_type: u8, profile: &str) -> RTCRtpCodecParameters {
    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: format!("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={profile}"),
            rtcp_feedback: vec![],
        },
        payload_type,
        ..Default::default()
    }
}

async fn session(code: &str, offer: String, tx: &Sender<Event>, stop: &AtomicBool, keyframe: &Arc<AtomicBool>) -> String {
    let result: Result<String, String> = async {
        log("building the peer connection");
        let runtime = default_runtime().ok_or("no WebRTC runtime")?;
        let mut media = MediaEngine::default();
        // The H.264 flavours phones send: constrained baseline, baseline, high.
        for (pt, profile) in [(102u8, "42e01f"), (106, "42001f"), (108, "640c1f"), (112, "640034")] {
            media.register_codec(h264(pt, profile), RtpCodecKind::Video).map_err(|e| e.to_string())?;
        }
        let registry = register_default_interceptors(Registry::new(), &mut media).map_err(|e| e.to_string())?;
        // Same network: no STUN/TURN servers, nothing leaves the local network.
        let config = RTCConfigurationBuilder::new().with_ice_servers(vec![]).build();
        // Browsers hide the phone's address behind a random "….local" name; resolve those on the local network.
        let settings = SettingEngineBuilder::new()
            .with_multicast_dns_mode(MulticastDnsMode::QueryOnly)
            .with_multicast_dns_timeout(Some(Duration::from_secs(8)))
            .build();

        let (gathered_tx, mut gathered_rx) = tokio::sync::mpsc::channel(1);
        let (state_tx, mut state_rx) = tokio::sync::mpsc::channel(8);
        let handler = Arc::new(Handler {
            runtime: runtime.clone(),
            gathered: gathered_tx,
            state: state_tx,
            frames: tx.clone(),
            keyframe: keyframe.clone(),
        });
        let pc = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_media_engine(media)
            .with_interceptor_registry(registry)
            .with_setting_engine(settings)
            .with_handler(handler as Arc<dyn PeerConnectionEventHandler>)
            .with_runtime(runtime.clone())
            .with_udp_addrs(vec![format!("{}:0", local_ip())])
            .build()
            .await
            .map_err(|e| e.to_string())?;

        log(&format!("peer connection ready on {}", local_ip()));
        log(&format!("phone's addresses: {}", candidates(&offer)));
        let other_network = other_network_hint(&offer, &local_ip());
        let offer = RTCSessionDescription::offer(offer).map_err(|e| format!("The phone's offer is broken ({e})"))?;
        pc.set_remote_description(offer).await.map_err(|e| format!("set_remote_description: {e}"))?;
        let answer = pc.create_answer(None).await.map_err(|e| format!("create_answer: {e}"))?;
        pc.set_local_description(answer).await.map_err(|e| format!("set_local_description: {e}"))?;
        let _ = tokio::time::timeout(Duration::from_secs(3), gathered_rx.recv()).await;
        let sdp = pc.local_description().await.ok_or("no answer")?.sdp;
        log(&format!("answer ready ({} bytes), our addresses: {}", sdp.len(), candidates(&sdp)));
        let code2 = code.to_string();
        tokio::task::spawn_blocking(move || post_answer(&code2, &sdp)).await.map_err(|e| e.to_string())??;
        log("answer sent, waiting for the connection");

        // Stream until the phone is gone (or we're told to stop).
        let started = Instant::now();
        let mut connected = false;
        let reason = loop {
            if stop.load(Ordering::Relaxed) {
                break "stopped".to_string();
            }
            match tokio::time::timeout(Duration::from_millis(250), state_rx.recv()).await {
                Ok(Some(RTCPeerConnectionState::Connected)) => {
                    connected = true;
                    let _ = tx.send(Event::Connected);
                }
                Ok(Some(RTCPeerConnectionState::Failed)) => break "The connection to the phone failed. Are both on the same Wi‑Fi?".into(),
                Ok(Some(RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Closed)) => break "The phone disconnected.".into(),
                Ok(None) => break "The phone disconnected.".into(),
                _ => {}
            }
            if !connected && started.elapsed() > Duration::from_secs(25) {
                break other_network
                    .clone()
                    .unwrap_or_else(|| "The phone couldn't reach this PC. Are both on the same Wi‑Fi?".into());
            }
        };
        let _ = pc.close().await;
        Ok(reason)
    }
    .await;
    result.unwrap_or_else(|e| e)
}

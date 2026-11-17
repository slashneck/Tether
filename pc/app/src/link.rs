//! One TCP connection to the phone (protocol v1, see docs/PROTOCOL.md).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const PORT: u16 = 27183;

pub const HELLO: u8 = 0x01;
pub const VIDEO: u8 = 0x02;
pub const STATUS: u8 = 0x03;
pub const SETTINGS: u8 = 0x81;
pub const KEYFRAME: u8 = 0x82;
pub const BYE: u8 = 0x83;
pub const PING: u8 = 0x90;

pub const FLAG_KEYFRAME: u8 = 1;

pub struct Message {
    pub kind: u8,
    pub payload: Vec<u8>,
}

pub struct Link {
    writer: Arc<Mutex<TcpStream>>,
    pub messages: Receiver<Message>,
}

impl Link {
    pub fn connect(addr: SocketAddr) -> std::io::Result<Link> {
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
        stream.set_nodelay(true)?;
        let mut reader = stream.try_clone()?;
        let (tx, rx) = mpsc::sync_channel::<Message>(256);
        std::thread::Builder::new()
            .name("tether-link".into())
            .spawn(move || {
                let mut head = [0u8; 5];
                loop {
                    if reader.read_exact(&mut head).is_err() {
                        break;
                    }
                    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
                    if len > 16 << 20 {
                        break;
                    }
                    let mut payload = vec![0u8; len];
                    if reader.read_exact(&mut payload).is_err() {
                        break;
                    }
                    if tx.send(Message { kind: head[0], payload }).is_err() {
                        break;
                    }
                }
                let _ = reader.shutdown(std::net::Shutdown::Both);
            })?;
        Ok(Link { writer: Arc::new(Mutex::new(stream)), messages: rx })
    }

    pub fn send(&self, kind: u8, payload: &[u8]) -> std::io::Result<()> {
        let mut msg = Vec::with_capacity(5 + payload.len());
        msg.push(kind);
        msg.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        msg.extend_from_slice(payload);
        self.writer.lock().unwrap().write_all(&msg)
    }

    pub fn send_json(&self, kind: u8, value: &serde_json::Value) -> std::io::Result<()> {
        self.send(kind, value.to_string().as_bytes())
    }

    pub fn close(&self) {
        let _ = self.writer.lock().unwrap().shutdown(std::net::Shutdown::Both);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.close();
    }
}

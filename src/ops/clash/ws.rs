//! Pure-Rust RFC 6455 WebSocket client for Mihomo real-time streams
//! (`/traffic`, `/memory`, `/logs`) with zero additional external crates.
//!
//! Authentication is passed strictly via the HTTP Upgrade `Authorization: Bearer`
//! header and never in query parameters so controller secrets never leak into logs.

use crate::error::{Error, Result};
use base64::Engine as _;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_HANDSHAKE_BYTES: usize = 16 * 1024;
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Compact RFC 3174 SHA-1 used exclusively for validating `Sec-WebSocket-Accept`.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity((data.len() + 9 + 63) & !63);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 80];
    for chunk in msg.as_chunks::<64>().0 {
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub(crate) fn accept_key(client_key: &str) -> String {
    let mut input = Vec::with_capacity(client_key.len() + WS_GUID.len());
    input.extend_from_slice(client_key.as_bytes());
    input.extend_from_slice(WS_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(sha1(&input))
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    #[cfg(unix)]
    {
        if let Ok(mut f) = std::fs::File::open("/dev/urandom")
            && f.read_exact(&mut buf).is_ok()
        {
            return buf;
        }
    }
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut state = nanos ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for slot in &mut buf {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *slot = (state >> 56) as u8;
    }
    buf
}

fn ws_err(detail: &str) -> Error {
    Error::new("error.command", detail)
}

pub(crate) struct WsClient {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl WsClient {
    pub(crate) fn connect(
        socket: SocketAddr,
        host: &str,
        path: &str,
        secret: &str,
        timeout: Duration,
    ) -> Result<Self> {
        let mut stream = TcpStream::connect_timeout(&socket, timeout)
            .map_err(|_| ws_err("cannot connect to Mihomo WebSocket endpoint"))?;
        stream.set_read_timeout(Some(Duration::from_millis(120)))?;
        stream.set_write_timeout(Some(timeout))?;

        let nonce = random_bytes::<16>();
        let client_key = base64::engine::general_purpose::STANDARD.encode(nonce);
        let expected_accept = accept_key(&client_key);

        let auth = if secret.is_empty() {
            String::new()
        } else {
            format!("Authorization: Bearer {secret}\r\n")
        };
        let req = format!(
            "GET {path} HTTP/1.1\r\n\
             Host: {host}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: {client_key}\r\n\
             Sec-WebSocket-Version: 13\r\n\
             {auth}\r\n"
        );
        stream
            .write_all(req.as_bytes())
            .map_err(|_| ws_err("failed to send WebSocket handshake"))?;

        let start = Instant::now();
        let mut raw = Vec::with_capacity(512);
        let split = loop {
            if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos;
            }
            if raw.len() >= MAX_HANDSHAKE_BYTES {
                return Err(ws_err("oversized WebSocket handshake response"));
            }
            if start.elapsed() > timeout {
                return Err(ws_err("WebSocket handshake timed out"));
            }
            let mut chunk = [0u8; 1024];
            match stream.read(&mut chunk) {
                Ok(0) => return Err(ws_err("WebSocket handshake closed prematurely")),
                Ok(n) => raw.extend_from_slice(&chunk[..n]),
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(_) => return Err(ws_err("WebSocket handshake read failed")),
            }
        };

        let header_str = std::str::from_utf8(&raw[..split])
            .map_err(|_| ws_err("invalid UTF-8 in WebSocket handshake"))?;
        let mut lines = header_str.split("\r\n");
        let status_line = lines.next().unwrap_or("");
        let mut parts = status_line.split_whitespace();
        let proto_ok = matches!(parts.next(), Some("HTTP/1.1"));
        let code_ok = parts.next() == Some("101");
        if !proto_ok || !code_ok {
            return Err(ws_err("WebSocket upgrade rejected by controller"));
        }
        let mut accept_ok = false;
        for line in lines {
            if let Some((name, val)) = line.split_once(':')
                && name.eq_ignore_ascii_case("sec-websocket-accept")
                && val.trim() == expected_accept
            {
                accept_ok = true;
            }
        }
        if !accept_ok {
            return Err(ws_err("WebSocket Sec-WebSocket-Accept mismatch"));
        }

        let leftover = raw[split + 4..].to_vec();
        Ok(Self {
            stream,
            buf: leftover,
        })
    }

    fn send_masked_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<()> {
        let mask = random_bytes::<4>();
        let mut frame = Vec::with_capacity(14 + payload.len());
        frame.push(0x80 | (opcode & 0x0F));
        let len = payload.len();
        if len < 126 {
            frame.push(0x80 | (len as u8));
        } else if len <= u16::MAX as usize {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        frame.extend_from_slice(&mask);
        for (i, &b) in payload.iter().enumerate() {
            frame.push(b ^ mask[i & 3]);
        }
        self.stream
            .write_all(&frame)
            .map_err(|_| ws_err("failed to write WebSocket frame"))
    }

    fn fill_at_least(
        &mut self,
        need: usize,
        cancel: &AtomicBool,
        global_cancel: &AtomicBool,
    ) -> Result<bool> {
        let mut chunk = [0u8; 4096];
        while self.buf.len() < need {
            if cancel.load(Ordering::Relaxed) || global_cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(false),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(_) => return Err(ws_err("WebSocket read error")),
            }
        }
        Ok(true)
    }

    fn read_frame(
        &mut self,
        cancel: &AtomicBool,
        global_cancel: &AtomicBool,
    ) -> Result<Option<(bool, u8, Vec<u8>)>> {
        if !self.fill_at_least(2, cancel, global_cancel)? {
            return Ok(None);
        }
        let b0 = self.buf[0];
        let b1 = self.buf[1];
        let fin = (b0 & 0x80) != 0;
        let opcode = b0 & 0x0F;
        let masked = (b1 & 0x80) != 0;
        let mut header_len = 2usize;
        let payload_len: usize = match b1 & 0x7F {
            n @ 0..=125 => n as usize,
            126 => {
                if !self.fill_at_least(4, cancel, global_cancel)? {
                    return Ok(None);
                }
                header_len = 4;
                u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize
            }
            127 => {
                if !self.fill_at_least(10, cancel, global_cancel)? {
                    return Ok(None);
                }
                header_len = 10;
                let raw_len = u64::from_be_bytes([
                    self.buf[2],
                    self.buf[3],
                    self.buf[4],
                    self.buf[5],
                    self.buf[6],
                    self.buf[7],
                    self.buf[8],
                    self.buf[9],
                ]);
                if raw_len > MAX_FRAME_BYTES as u64 {
                    return Err(ws_err("WebSocket frame exceeds 4 MiB limit"));
                }
                raw_len as usize
            }
            _ => unreachable!(),
        };
        if payload_len > MAX_FRAME_BYTES {
            return Err(ws_err("WebSocket frame exceeds 4 MiB limit"));
        }
        let mask_len = if masked { 4 } else { 0 };
        let total = header_len + mask_len + payload_len;
        if !self.fill_at_least(total, cancel, global_cancel)? {
            return Ok(None);
        }
        let mut payload = self.buf[header_len + mask_len..total].to_vec();
        if masked {
            let mask = [
                self.buf[header_len],
                self.buf[header_len + 1],
                self.buf[header_len + 2],
                self.buf[header_len + 3],
            ];
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i & 3];
            }
        }
        self.buf.drain(..total);
        Ok(Some((fin, opcode, payload)))
    }

    /// Read the next complete UTF-8 text message, transparently handling fragmented frames,
    /// `Ping` -> `Pong` replies, and graceful `Close` / cooperative cancellation.
    pub(crate) fn read_text(
        &mut self,
        cancel: &AtomicBool,
        global_cancel: &AtomicBool,
    ) -> Result<Option<String>> {
        let mut message = Vec::new();
        let mut in_fragment = false;
        loop {
            let Some((fin, opcode, payload)) = self.read_frame(cancel, global_cancel)? else {
                return Ok(None);
            };
            match opcode {
                0x8 => {
                    let _ = self.send_masked_frame(0x8, &[]);
                    return Ok(None);
                }
                0x9 => {
                    let _ = self.send_masked_frame(0xA, &payload);
                }
                0xA => {}
                0x1 => {
                    message.clear();
                    message.extend_from_slice(&payload);
                    if fin {
                        let text = String::from_utf8(message)
                            .map_err(|_| ws_err("invalid UTF-8 in WebSocket text frame"))?;
                        return Ok(Some(text));
                    }
                    in_fragment = true;
                }
                0x0 if in_fragment => {
                    if message.len().saturating_add(payload.len()) > MAX_FRAME_BYTES {
                        return Err(ws_err("reassembled WebSocket message exceeds 4 MiB"));
                    }
                    message.extend_from_slice(&payload);
                    if fin {
                        let text = String::from_utf8(message)
                            .map_err(|_| ws_err("invalid UTF-8 in WebSocket text frame"))?;
                        return Ok(Some(text));
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};

    fn server_frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push((if fin { 0x80 } else { 0 }) | (opcode & 0x0F));
        let len = payload.len();
        if len < 126 {
            out.push(len as u8);
        } else if len <= u16::MAX as usize {
            out.push(126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn rfc6455_accept_key_matches_standard_vector() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn websocket_client_handles_auth_fragments_ping_pong_and_close() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = sock.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            assert!(req.starts_with("GET /traffic HTTP/1.1\r\n"));
            assert!(req.contains("Authorization: Bearer ws-secret-key\r\n"));
            let client_key = req
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("sec-websocket-key")
                        .then(|| v.trim().to_owned())
                })
                .unwrap();
            let accept = accept_key(&client_key);
            let resp = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            );
            sock.write_all(resp.as_bytes()).unwrap();
            // Send 1) normal text frame, 2) Ping, 3) 2-part fragmented text frame, 4) Close.
            sock.write_all(&server_frame(true, 0x1, br#"{"up":100,"down":200}"#))
                .unwrap();
            sock.write_all(&server_frame(true, 0x9, b"hi")).unwrap();
            sock.write_all(&server_frame(false, 0x1, br#"{"up":"#))
                .unwrap();
            sock.write_all(&server_frame(true, 0x0, br#"300,"down":400}"#))
                .unwrap();
            sock.write_all(&server_frame(true, 0x8, &[])).unwrap();

            // Verify client sent masked Pong for "hi".
            let mut pong = [0u8; 16];
            let m = sock.read(&mut pong).unwrap();
            assert!(m >= 8);
            assert_eq!(pong[0], 0x8A);
            assert_eq!(pong[1], 0x82);
            let mask = [pong[2], pong[3], pong[4], pong[5]];
            assert_eq!([pong[6] ^ mask[0], pong[7] ^ mask[1]], *b"hi");
        });

        let cancel = AtomicBool::new(false);
        let global = AtomicBool::new(false);
        let mut client = WsClient::connect(
            addr,
            &addr.to_string(),
            "/traffic",
            "ws-secret-key",
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(
            client.read_text(&cancel, &global).unwrap().as_deref(),
            Some(r#"{"up":100,"down":200}"#)
        );
        assert_eq!(
            client.read_text(&cancel, &global).unwrap().as_deref(),
            Some(r#"{"up":300,"down":400}"#)
        );
        assert_eq!(client.read_text(&cancel, &global).unwrap(), None);
        server.join().unwrap();
    }

    #[test]
    fn websocket_client_rejects_oversized_frame_header() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = sock.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let client_key = req
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("sec-websocket-key")
                        .then(|| v.trim().to_owned())
                })
                .unwrap();
            let accept = accept_key(&client_key);
            let resp = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            );
            sock.write_all(resp.as_bytes()).unwrap();
            let mut giant = vec![0x81u8, 127u8];
            giant.extend_from_slice(&((MAX_FRAME_BYTES as u64) + 1).to_be_bytes());
            let _ = sock.write_all(&giant);
        });

        let cancel = AtomicBool::new(false);
        let global = AtomicBool::new(false);
        let mut client =
            WsClient::connect(addr, &addr.to_string(), "/logs", "", Duration::from_secs(2))
                .unwrap();
        assert!(client.read_text(&cancel, &global).is_err());
        server.join().unwrap();
    }
}

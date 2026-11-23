//! Zero-dependency terminal clipboard integration via OSC 52.
//!
//! Works directly over SSH in modern terminals (Windows Terminal, iTerm2, Ghostty,
//! Kitty, WezTerm, Alacritty, VS Code, Tabby, Foot, Konsole, Xshell, Motty, and
//! tmux with `set-clipboard`) without requiring X11/Wayland utilities on the server.

use std::io::{self, Write};

/// Maximum raw byte payload encoded into a single OSC 52 sequence (512 KiB).
pub(super) const MAX_CLIPBOARD_BYTES: usize = 512 * 1024;

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard RFC 4648 Base64 encoder (no line wraps).
pub(super) fn encode_base64(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(B64_TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(B64_TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64_TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Cap clipboard payload to `MAX_CLIPBOARD_BYTES` along a UTF-8 char boundary,
/// keeping the trailing content (most recent log output) when oversized.
pub(super) fn bounded_payload(text: &str) -> (&str, bool) {
    if text.len() <= MAX_CLIPBOARD_BYTES {
        return (text, false);
    }
    let mut start = text.len() - MAX_CLIPBOARD_BYTES;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    (&text[start..], true)
}

/// Format an OSC 52 clipboard write sequence (`ESC ] 52 ; c ; <base64> BEL`).
pub(super) fn osc52_sequence(text: &str) -> (String, bool) {
    let (slice, truncated) = bounded_payload(text);
    let encoded = encode_base64(slice.as_bytes());
    (format!("\x1b]52;c;{encoded}\x07"), truncated)
}

/// Write an OSC 52 clipboard sequence to `writer` and flush it immediately.
pub(super) fn write_osc52(writer: &mut impl Write, text: &str) -> io::Result<bool> {
    let (seq, truncated) = osc52_sequence(text);
    writer.write_all(seq.as_bytes())?;
    writer.flush()?;
    Ok(truncated)
}

/// Copy `text` to the host terminal's system clipboard via OSC 52 on stdout.
pub(super) fn copy(text: &str) -> io::Result<bool> {
    let mut stdout = io::stdout();
    write_osc52(&mut stdout, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_base64_vectors_and_utf8_encode_correctly() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foob"), "Zm9vYg==");
        assert_eq!(encode_base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(
            encode_base64("cqs run 示例".as_bytes()),
            "Y3FzIHJ1biDnpLrkvos="
        );
    }

    #[test]
    fn osc52_sequence_wraps_base64_and_bounds_oversized_utf8() {
        let mut buf = Vec::new();
        let truncated = write_osc52(&mut buf, "cqs run system.info").unwrap();
        assert!(!truncated);
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "\x1b]52;c;Y3FzIHJ1biBzeXN0ZW0uaW5mbw==\x07"
        );

        let huge = "中".repeat((MAX_CLIPBOARD_BYTES / 3) + 50);
        let (slice, truncated) = bounded_payload(&huge);
        assert!(truncated);
        assert!(slice.len() <= MAX_CLIPBOARD_BYTES);
        assert!(slice.chars().all(|c| c == '中'));
    }
}

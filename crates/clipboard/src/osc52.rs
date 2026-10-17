use std::io::Write;

/// Encodes raw bytes to a standard Base64 string (RFC 4648) with zero external dependencies.
pub fn base64_encode(data: &[u8]) -> String {
    const BASE64_CHARS: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };

        out.push(BASE64_CHARS[(b0 >> 2) as usize] as char);
        out.push(BASE64_CHARS[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);

        if chunk.len() > 1 {
            out.push(BASE64_CHARS[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }

        if chunk.len() > 2 {
            out.push(BASE64_CHARS[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Copies text to both the local desktop clipboard (via arboard)
/// and the client terminal clipboard (via OSC 52 escape sequences).
///
/// This ensures clipboard operations succeed both on desktop sessions and
/// remote headless SSH sessions (Termius, iTerm2, Alacritty, WezTerm, etc.).
pub fn copy_to_clipboard(text: &str) {
    // 1. Try local desktop clipboard (X11 / Wayland / macOS / Windows)
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }

    // 2. Emit OSC 52 sequence for remote SSH terminal emulators (Termius, etc.)
    emit_osc52(text);
}

/// Emits an OSC 52 escape sequence for clipboard synchronization.
/// Handles both raw terminals and tmux environments (via DCS passthrough).
pub fn emit_osc52(text: &str) {
    let encoded = base64_encode(text.as_bytes());

    // Direct OSC 52: ESC ] 52 ; c ; <base64> BEL
    let osc52 = format!("\x1b]52;c;{}\x07", encoded);

    let in_tmux = std::env::var("TMUX").is_ok()
        || std::env::var("TERM")
            .map(|t| t.starts_with("tmux") || t.starts_with("screen"))
            .unwrap_or(false);

    let mut stdout = std::io::stdout();
    if in_tmux {
        // In tmux, send DCS passthrough sequence (ESC P tmux ; ESC <escaped_payload> ESC \)
        // and direct OSC 52 so both passthrough and tmux set-clipboard capture it.
        let tmux_passthrough = format!("\x1bPtmux;\x1b\x1b]52;c;{}\x07\x1b\\", encoded);
        let _ = stdout.write_all(tmux_passthrough.as_bytes());
        let _ = stdout.write_all(osc52.as_bytes());
    } else {
        let _ = stdout.write_all(osc52.as_bytes());
    }
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encode_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"Hello, World!"), "SGVsbG8sIFdvcmxkIQ==");
    }
}

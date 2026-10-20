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

static INTERNAL_CLIPBOARD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Copies text to both the local desktop clipboard (via arboard)
/// and the client terminal clipboard (via OSC 52 escape sequences),
/// while caching in internal clipboard for fast fallback retrieval.
///
/// This ensures clipboard operations succeed both on desktop sessions and
/// remote headless SSH sessions (Termius, iTerm2, Alacritty, WezTerm, etc.).
pub fn copy_to_clipboard(text: &str) {
    if let Ok(mut lock) = INTERNAL_CLIPBOARD.lock() {
        *lock = Some(text.to_string());
    }

    // 1. Try local desktop clipboard (X11 / Wayland / macOS / Windows)
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }

    // 2. Emit OSC 52 sequence for remote SSH terminal emulators (Termius, etc.)
    emit_osc52(text);
}

pub fn set_internal_clipboard(text: &str) {
    if let Ok(mut lock) = INTERNAL_CLIPBOARD.lock() {
        *lock = Some(text.to_string());
    }
}

pub fn get_internal_clipboard() -> Option<String> {
    INTERNAL_CLIPBOARD.lock().ok().and_then(|lock| lock.clone())
}

/// Retrieves text from system clipboard with comprehensive multi-platform fallbacks:
/// 1. arboard (Wayland / X11 / macOS / Windows desktop)
/// 2. wl-paste (Wayland CLI)
/// 3. xclip (X11 CLI)
/// 4. xsel (X11 CLI)
/// 5. pbpaste (macOS CLI)
/// 6. powershell.exe Get-Clipboard (WSL / Windows CLI)
/// 7. Internal clipboard (last copied text within TAG)
pub fn get_clipboard_text() -> Option<String> {
    // 1. Try local desktop clipboard (via arboard)
    if let Ok(mut cb) = arboard::Clipboard::new() {
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                set_internal_clipboard(&text);
                return Some(text);
            }
        }
    }

    // 2. Wayland fallback: wl-paste
    if let Ok(output) = std::process::Command::new("wl-paste")
        .args(["--no-newline"])
        .stdin(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            if !text.is_empty() {
                set_internal_clipboard(&text);
                return Some(text);
            }
        }
    }

    // 3. X11 fallback: xclip
    if let Ok(output) = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .stdin(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            if !text.is_empty() {
                set_internal_clipboard(&text);
                return Some(text);
            }
        }
    }

    // 4. X11 fallback: xsel
    if let Ok(output) = std::process::Command::new("xsel")
        .args(["--clipboard", "--output"])
        .stdin(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            if !text.is_empty() {
                set_internal_clipboard(&text);
                return Some(text);
            }
        }
    }

    // 5. macOS fallback: pbpaste
    if let Ok(output) = std::process::Command::new("pbpaste")
        .stdin(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            if !text.is_empty() {
                set_internal_clipboard(&text);
                return Some(text);
            }
        }
    }

    // 6. Windows / WSL fallback: powershell.exe Get-Clipboard
    if let Ok(output) = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", "Get-Clipboard"])
        .stdin(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_string();
            if !text.is_empty() {
                set_internal_clipboard(&text);
                return Some(text);
            }
        }
    }

    // 7. Internal clipboard fallback (last text copied inside TAG)
    get_internal_clipboard()
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

    #[test]
    fn test_internal_clipboard_fallback() {
        set_internal_clipboard("test_paste_content");
        assert_eq!(
            get_internal_clipboard(),
            Some("test_paste_content".to_string())
        );
    }
}

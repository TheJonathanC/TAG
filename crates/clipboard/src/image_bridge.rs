use arboard::Clipboard;
use chrono::Utc;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

pub struct ImageBridge;

enum ImageSource {
    ArboardRgba {
        width: usize,
        height: usize,
        bytes: Vec<u8>,
    },
    RawBytes(Vec<u8>),
}

impl ImageBridge {
    /// Save clipboard image to `.orchestrator/cache/img_<timestamp>.png` in worktree_path.
    /// Tries multiple clipboard sources:
    /// 1. In-process OS clipboard via `arboard` (Wayland with data-control, X11, macOS, Windows)
    /// 2. CLI clipboard fallbacks (`wl-paste`, `xclip`, `pngpaste`, `powershell.exe`)
    /// 3. Recently staged drop file (`.orchestrator/cache/drop.png` or `/tmp/tag-clipboard-drop.png`)
    pub fn save_clipboard_image(worktree_path: &Path) -> Result<PathBuf, anyhow::Error> {
        let source = Self::read_clipboard_source(worktree_path)?;

        let cache_dir = worktree_path.join(".orchestrator").join("cache");
        fs::create_dir_all(&cache_dir)?;
        Self::ensure_git_exclude(worktree_path);

        let timestamp = Utc::now().timestamp_millis();
        let filename = format!("img_{}.png", timestamp);
        let out_path = cache_dir.join(&filename);

        match source {
            ImageSource::ArboardRgba {
                width,
                height,
                bytes,
            } => {
                image::save_buffer(
                    &out_path,
                    &bytes,
                    width as u32,
                    height as u32,
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(|e| anyhow::anyhow!("Failed to encode clipboard image to PNG: {}", e))?;
            }
            ImageSource::RawBytes(raw_bytes) => {
                Self::write_raw_image_to_png(&out_path, &raw_bytes)?;
            }
        }

        info!("Saved clipboard image to {:?}", out_path);
        Ok(out_path)
    }

    /// Save arbitrary raw image bytes to `.orchestrator/cache/img_<timestamp>.png`.
    /// Decodes and normalizes to PNG format if needed.
    pub fn save_image_bytes(
        worktree_path: &Path,
        raw_bytes: &[u8],
    ) -> Result<PathBuf, anyhow::Error> {
        let cache_dir = worktree_path.join(".orchestrator").join("cache");
        fs::create_dir_all(&cache_dir)?;
        Self::ensure_git_exclude(worktree_path);

        let timestamp = Utc::now().timestamp_millis();
        let filename = format!("img_{}.png", timestamp);
        let out_path = cache_dir.join(&filename);

        Self::write_raw_image_to_png(&out_path, raw_bytes)?;
        info!("Saved image bytes to {:?}", out_path);
        Ok(out_path)
    }

    fn write_raw_image_to_png(out_path: &Path, raw_bytes: &[u8]) -> Result<(), anyhow::Error> {
        if raw_bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            fs::write(out_path, raw_bytes)?;
        } else {
            let dynamic_img = image::load_from_memory(raw_bytes)
                .map_err(|e| anyhow::anyhow!("Failed to decode image data: {}", e))?;
            dynamic_img
                .save_with_format(out_path, image::ImageFormat::Png)
                .map_err(|e| anyhow::anyhow!("Failed to save image as PNG: {}", e))?;
        }
        Ok(())
    }

    fn read_clipboard_source(worktree_path: &Path) -> Result<ImageSource, anyhow::Error> {
        // 1. Try in-process arboard clipboard (only if display server is present on Linux)
        if Self::is_gui_display_available() {
            if let Ok(mut cb) = Clipboard::new() {
                if let Ok(img) = cb.get_image() {
                    return Ok(ImageSource::ArboardRgba {
                        width: img.width,
                        height: img.height,
                        bytes: img.bytes.into_owned(),
                    });
                }
            }
        }

        // 2. Try CLI tools based on active environment
        // Wayland: wl-paste
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            if let Some(bytes) = Self::try_wl_paste() {
                return Ok(ImageSource::RawBytes(bytes));
            }
        }

        // X11: xclip
        if std::env::var("DISPLAY").is_ok() {
            if let Some(bytes) = Self::try_xclip() {
                return Ok(ImageSource::RawBytes(bytes));
            }
        }

        // macOS: pngpaste
        #[cfg(target_os = "macos")]
        {
            if let Some(bytes) = Self::try_macos_clipboard() {
                return Ok(ImageSource::RawBytes(bytes));
            }
        }

        // Windows / WSL powershell fallback
        if let Some(bytes) = Self::try_powershell_clipboard() {
            return Ok(ImageSource::RawBytes(bytes));
        }

        // 3. Try staged drop file (useful in SSH without X11 forwarding)
        if let Some(bytes) = Self::try_staged_drop(worktree_path) {
            return Ok(ImageSource::RawBytes(bytes));
        }

        // If no source succeeded, provide actionable diagnostics:
        if !Self::is_gui_display_available() {
            Err(anyhow::anyhow!(
                "No GUI display server active ($DISPLAY / $WAYLAND_DISPLAY unset). In an SSH session, reconnect with 'ssh -X' or 'ssh -Y' for remote clipboard, or use 'tag paste-image <file>'."
            ))
        } else {
            Err(anyhow::anyhow!(
                "No image found in clipboard (clipboard contains text or is empty)."
            ))
        }
    }

    fn is_gui_display_available() -> bool {
        #[cfg(target_os = "linux")]
        {
            std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok()
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }

    fn try_wl_paste() -> Option<Vec<u8>> {
        for mime in &["image/png", "image/jpeg", "image/webp"] {
            if let Ok(output) = Command::new("wl-paste").args(["--type", mime]).output() {
                if output.status.success() && !output.stdout.is_empty() {
                    return Some(output.stdout);
                }
            }
        }
        None
    }

    fn try_xclip() -> Option<Vec<u8>> {
        for target in &["image/png", "image/jpeg"] {
            if let Ok(output) = Command::new("xclip")
                .args(["-selection", "clipboard", "-t", target, "-o"])
                .output()
            {
                if output.status.success() && !output.stdout.is_empty() {
                    return Some(output.stdout);
                }
            }
        }
        None
    }

    #[cfg(target_os = "macos")]
    fn try_macos_clipboard() -> Option<Vec<u8>> {
        if let Ok(output) = Command::new("pngpaste").arg("-").output() {
            if output.status.success() && !output.stdout.is_empty() {
                return Some(output.stdout);
            }
        }
        None
    }

    fn try_powershell_clipboard() -> Option<Vec<u8>> {
        let ps_cmd = if cfg!(windows) {
            "powershell"
        } else {
            "powershell.exe"
        };
        let script = r#"
            Add-Type -AssemblyName System.Windows.Forms;
            $img = [System.Windows.Forms.Clipboard]::GetImage();
            if ($img) {
                $temp = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), "tag_ps_clip_" + [System.Guid]::NewGuid().ToString() + ".png");
                $img.Save($temp, [System.Drawing.Imaging.ImageFormat]::Png);
                Write-Output $temp;
            }
        "#;
        if let Ok(output) = Command::new(ps_cmd)
            .args(["-NoProfile", "-Command", script])
            .output()
        {
            if output.status.success() {
                let temp_path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !temp_path_str.is_empty() {
                    let temp_path = PathBuf::from(temp_path_str);
                    if temp_path.exists() {
                        let bytes = fs::read(&temp_path).ok();
                        let _ = fs::remove_file(&temp_path);
                        return bytes;
                    }
                }
            }
        }
        None
    }

    fn try_staged_drop(worktree_path: &Path) -> Option<Vec<u8>> {
        let candidates = [
            worktree_path
                .join(".orchestrator")
                .join("cache")
                .join("drop.png"),
            worktree_path
                .join(".orchestrator")
                .join("cache")
                .join("drop.jpg"),
            std::env::temp_dir().join("tag-clipboard-drop.png"),
            std::env::temp_dir().join("tag-clipboard-drop.jpg"),
            PathBuf::from("/tmp/tag-clipboard-drop.png"),
            PathBuf::from("/tmp/tag-clipboard-drop.jpg"),
        ];

        for path in &candidates {
            if path.exists() {
                if let Ok(metadata) = fs::metadata(path) {
                    if let Ok(modified) = metadata.modified() {
                        if let Ok(elapsed) = modified.elapsed() {
                            if elapsed.as_secs() <= 180 {
                                if let Ok(bytes) = fs::read(path) {
                                    let _ = fs::remove_file(path);
                                    info!("Loaded staged clipboard drop file from {:?}", path);
                                    return Some(bytes);
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn ensure_git_exclude(worktree_path: &Path) {
        let dot_git = worktree_path.join(".git");
        let exclude_file = if dot_git.is_file() {
            // Linked worktree contains a gitdir pointer
            if let Ok(content) = fs::read_to_string(&dot_git) {
                if let Some(gitdir_line) = content.lines().find(|l| l.starts_with("gitdir:")) {
                    let gitdir_path = gitdir_line["gitdir:".len()..].trim();
                    let p = PathBuf::from(gitdir_path);
                    let abs_gitdir = if p.is_relative() {
                        worktree_path.join(p)
                    } else {
                        p
                    };
                    abs_gitdir.join("info").join("exclude")
                } else {
                    dot_git.join("info").join("exclude")
                }
            } else {
                dot_git.join("info").join("exclude")
            }
        } else {
            dot_git.join("info").join("exclude")
        };

        if let Some(parent) = exclude_file.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(content) = fs::read_to_string(&exclude_file) {
            if content.contains(".orchestrator/cache") {
                return;
            }
        }

        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude_file)
        {
            let _ = writeln!(f, "\n.orchestrator/cache/");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_save_buffer() {
        let dummy = vec![0u8; 4 * 10 * 10]; // 10x10 RGBA
        let dir = std::env::temp_dir();
        let path = dir.join("test_dummy.png");
        let res = image::save_buffer(&path, &dummy, 10, 10, image::ExtendedColorType::Rgba8);
        assert!(res.is_ok(), "save_buffer failed: {:?}", res);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_staged_drop() {
        let dir = std::env::temp_dir();
        let drop_path = dir.join("tag-clipboard-drop.png");
        let dummy = vec![1, 2, 3, 4];
        let _ = fs::write(&drop_path, &dummy);

        let retrieved = ImageBridge::try_staged_drop(Path::new("/nonexistent"));
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), dummy);
        assert!(!drop_path.exists());
    }

    #[test]
    fn test_save_image_bytes_png() {
        let dir = std::env::temp_dir().join("tag_test_worktree");
        let _ = fs::create_dir_all(&dir);

        // Valid 1x1 PNG bytes
        let png_1x1: [u8; 67] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];

        let res = ImageBridge::save_image_bytes(&dir, &png_1x1);
        assert!(res.is_ok(), "save_image_bytes failed: {:?}", res);
        let out_path = res.unwrap();
        assert!(out_path.exists());
        assert!(out_path.to_string_lossy().ends_with(".png"));

        let _ = fs::remove_dir_all(&dir);
    }
}

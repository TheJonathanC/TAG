use arboard::Clipboard;
use chrono::Utc;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct ImageBridge;

impl ImageBridge {
    pub fn save_clipboard_image(worktree_path: &Path) -> Result<PathBuf, anyhow::Error> {
        let mut clipboard = Clipboard::new()?;
        let image = match clipboard.get_image() {
            Ok(img) => img,
            Err(e) => return Err(anyhow::anyhow!("No image found in clipboard: {}", e)),
        };

        let cache_dir = worktree_path.join(".orchestrator").join("cache");
        fs::create_dir_all(&cache_dir)?;

        // Ensure .git/info/exclude ignores .orchestrator/cache
        Self::ensure_git_exclude(worktree_path);

        let timestamp = Utc::now().timestamp_millis();
        let filename = format!("img_{}.png", timestamp);
        let out_path = cache_dir.join(&filename);

        // Encode image as simple raw/PNG if image crate is not present, or write bytes
        // Using basic PPM/PNG or saving rgba bytes
        let mut f = fs::File::create(&out_path)?;
        // Write simple PPM (P6) header or raw image if image encoding:
        // P6\nwidth height\n255\n[RGB data]
        writeln!(f, "P6\n{} {}\n255", image.width, image.height)?;
        for chunk in image.bytes.as_chunks::<4>().0 {
            f.write_all(&chunk[0..3])?; // write R, G, B
        }

        info!("Saved clipboard image to {:?}", out_path);
        Ok(out_path)
    }

    fn ensure_git_exclude(worktree_path: &Path) {
        let exclude_file = worktree_path.join(".git").join("info").join("exclude");
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

use std::cmp::Ordering;
use std::process::Command;

const INSTALL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/TheJonathanC/TAG/main/scripts/install.sh";

/// Check GitHub Releases for newer TAG releases.
/// Returns Some(newer_version) if an update is available.
pub fn check_for_github_update(current_version: &str) -> Option<String> {
    let output = Command::new("curl")
        .args([
            "-s",
            "--connect-timeout",
            "2",
            "-m",
            "4",
            "-H",
            "User-Agent: tag-cli",
            "-H",
            "Accept: application/vnd.github.v3+json",
            "https://api.github.com/repos/TheJonathanC/TAG/releases",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let releases: serde_json::Value = serde_json::from_str(&body).ok()?;
    newest_release_for(releases.as_array()?, current_version)
}

/// Pick the newest release that is newer than `current_version`.
/// Stable installations ignore pre-releases; pre-release installations see both.
fn newest_release_for(releases: &[serde_json::Value], current_version: &str) -> Option<String> {
    let current_is_prerelease = is_prerelease_version(current_version);
    let mut best: Option<String> = None;

    for rel in releases {
        let Some(tag_name) = rel.get("tag_name").and_then(|t| t.as_str()) else {
            continue;
        };
        if rel.get("draft").and_then(|d| d.as_bool()).unwrap_or(false) {
            continue;
        }
        let clean_tag = tag_name.trim_start_matches('v');
        let is_prerelease = rel
            .get("prerelease")
            .and_then(|p| p.as_bool())
            .unwrap_or(false)
            || is_prerelease_version(clean_tag);

        // If current installation is stable, ignore pre-releases
        if !current_is_prerelease && is_prerelease {
            continue;
        }

        if !is_version_newer(clean_tag, current_version) {
            continue;
        }
        if best
            .as_deref()
            .map(|b| is_version_newer(clean_tag, b))
            .unwrap_or(true)
        {
            best = Some(clean_tag.to_string());
        }
    }

    best
}

fn is_prerelease_version(version: &str) -> bool {
    version.trim_start_matches('v').contains('-')
}

/// Compare two semver strings (e.g. "0.2.0" vs "0.1.0", or "0.2.0-rc.1" vs "0.2.0-beta.3")
pub fn is_version_newer(remote: &str, current: &str) -> bool {
    match (parse_version(remote), parse_version(current)) {
        (Some(r), Some(c)) => compare_versions(&r, &c) == Ordering::Greater,
        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedVersion {
    major: u32,
    minor: u32,
    patch: u32,
    /// None for stable releases; Some((label rank, number)) for pre-releases.
    pre: Option<(u8, u32)>,
}

fn compare_versions(a: &ParsedVersion, b: &ParsedVersion) -> Ordering {
    (a.major, a.minor, a.patch)
        .cmp(&(b.major, b.minor, b.patch))
        .then_with(|| match (&a.pre, &b.pre) {
            (None, None) => Ordering::Equal,
            // A stable release is newer than any pre-release of the same version
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(ap), Some(bp)) => ap.cmp(bp),
        })
}

fn parse_version(s: &str) -> Option<ParsedVersion> {
    let s = s.trim_start_matches('v');
    let s = s.split('+').next()?; // ignore build metadata
    let (main_part, pre_part) = match s.split_once('-') {
        Some((m, p)) => (m, Some(p)),
        None => (s, None),
    };

    let mut nums = main_part.split('.');
    let major: u32 = nums.next()?.parse().ok()?;
    let minor: u32 = nums.next()?.parse().ok()?;
    let patch: u32 = nums.next()?.parse().ok()?;
    if nums.next().is_some() {
        return None;
    }

    let pre = pre_part.map(|pre| {
        let mut ids = pre.split('.');
        let label = ids.next().unwrap_or("").to_ascii_lowercase();
        let rank = match label.as_str() {
            "alpha" => 0,
            "beta" => 1,
            "rc" => 2,
            _ => 0,
        };
        let num = ids.find_map(|p| p.parse::<u32>().ok()).unwrap_or(0);
        (rank, num)
    });

    Some(ParsedVersion {
        major,
        minor,
        patch,
        pre,
    })
}

/// Download the official installation script and run it to update the binary in place
/// (into the directory the running `tag` executable lives in).
pub fn run_install_script(beta: bool) -> Result<(), anyhow::Error> {
    println!("Fetching and updating to the latest TAG release...");

    let script_dir = std::env::temp_dir().join(format!("tag-update-{}", std::process::id()));
    std::fs::create_dir_all(&script_dir)?;
    let script_path = script_dir.join("install.sh");

    let download = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&script_path)
        .arg(INSTALL_SCRIPT_URL)
        .status();
    let download_ok = matches!(download, Ok(s) if s.success());
    if !download_ok {
        let _ = std::fs::remove_dir_all(&script_dir);
        return Err(anyhow::anyhow!(
            "Failed to download the installer from {}",
            INSTALL_SCRIPT_URL
        ));
    }

    let mut cmd = Command::new("bash");
    cmd.arg(&script_path)
        .arg(if beta { "--beta" } else { "--stable" });
    if let Some(dir) = current_install_dir() {
        cmd.arg("--dir").arg(dir);
    }
    let status = cmd.status();
    let _ = std::fs::remove_dir_all(&script_dir);
    let status = status?;

    if status.success() {
        println!("\nTAG update finished. Run 'tag --version' to confirm the installed version.");
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Update failed with script exit code: {:?}",
            status.code()
        ))
    }
}

/// Directory containing the currently running `tag` binary, so the update replaces it
/// rather than installing a second copy elsewhere on PATH.
fn current_install_dir() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    exe.parent().map(|p| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_newer_major_minor_patch() {
        assert!(is_version_newer("0.2.0", "0.1.0"));
        assert!(is_version_newer("1.0.0", "0.9.9"));
        assert!(is_version_newer("0.1.1", "0.1.0"));
        assert!(!is_version_newer("0.1.0", "0.1.0"));
        assert!(!is_version_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn test_version_newer_beta() {
        assert!(is_version_newer("0.2.0", "0.2.0-beta.1"));
        assert!(is_version_newer("0.2.0-beta.2", "0.2.0-beta.1"));
        assert!(!is_version_newer("0.2.0-beta.1", "0.2.0-beta.2"));
        assert!(!is_version_newer("0.2.0-beta.1", "0.2.0"));
    }

    #[test]
    fn test_version_newer_prerelease_labels() {
        assert!(is_version_newer("0.2.0-rc.1", "0.2.0-beta.3"));
        assert!(is_version_newer("0.2.0-beta.1", "0.2.0-alpha.9"));
        assert!(!is_version_newer("0.2.0-alpha.9", "0.2.0-rc.1"));
        assert!(is_version_newer("v0.2.0", "0.2.0-rc.4"));
    }

    #[test]
    fn test_newest_release_respects_channel() {
        let releases = serde_json::json!([
            {"tag_name": "v0.3.0-beta.1", "prerelease": true},
            {"tag_name": "v0.2.0", "prerelease": false},
            {"tag_name": "v0.2.0-rc.1", "prerelease": true},
        ]);
        let arr = releases.as_array().unwrap();
        assert_eq!(newest_release_for(arr, "0.1.0").as_deref(), Some("0.2.0"));
        assert_eq!(
            newest_release_for(arr, "0.2.0-beta.1").as_deref(),
            Some("0.3.0-beta.1")
        );
        assert_eq!(newest_release_for(arr, "0.3.0-beta.1"), None);
        // A prerelease tag that was not flagged as such is still ignored by stable users
        let unflagged = serde_json::json!([{"tag_name": "v0.9.0-alpha.1", "prerelease": false}]);
        assert_eq!(
            newest_release_for(unflagged.as_array().unwrap(), "0.1.0"),
            None
        );
    }
}

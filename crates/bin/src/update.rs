use std::process::Command;

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
    let releases_array = releases.as_array()?;

    let is_current_beta = current_version.contains("beta");

    for rel in releases_array {
        let tag_name = rel.get("tag_name")?.as_str()?;
        let is_prerelease = rel
            .get("prerelease")
            .and_then(|p| p.as_bool())
            .unwrap_or(false);

        // If current installation is stable, ignore pre-releases
        if !is_current_beta && is_prerelease {
            continue;
        }

        let clean_tag = tag_name.trim_start_matches('v');
        if is_version_newer(clean_tag, current_version) {
            return Some(clean_tag.to_string());
        }
    }

    None
}

/// Compare two semver strings (e.g. "0.2.0" vs "0.1.0", or "0.2.0-beta.2" vs "0.2.0-beta.1")
pub fn is_version_newer(remote: &str, current: &str) -> bool {
    let remote_parsed = parse_version_tuple(remote);
    let current_parsed = parse_version_tuple(current);

    match (remote_parsed, current_parsed) {
        (Some((r_maj, r_min, r_pat, r_pre)), Some((c_maj, c_min, c_pat, c_pre))) => {
            if (r_maj, r_min, r_pat) > (c_maj, c_min, c_pat) {
                return true;
            }
            if (r_maj, r_min, r_pat) == (c_maj, c_min, c_pat) {
                match (r_pre, c_pre) {
                    (None, Some(_)) => true, // Stable release is newer than beta of same version
                    (Some(r_b), Some(c_b)) => r_b > c_b,
                    _ => false,
                }
            } else {
                false
            }
        }
        _ => false,
    }
}

fn parse_version_tuple(s: &str) -> Option<(u32, u32, u32, Option<u32>)> {
    let s = s.trim_start_matches('v');
    let mut parts = s.split('-');
    let main_part = parts.next()?;
    let pre_part = parts.next();

    let mut nums = main_part.split('.');
    let maj: u32 = nums.next()?.parse().ok()?;
    let min: u32 = nums.next()?.parse().ok()?;
    let pat: u32 = nums.next()?.parse().ok()?;

    let pre_num = if let Some(pre) = pre_part {
        pre.split('.').filter_map(|p| p.parse::<u32>().ok()).next()
    } else {
        None
    };

    Some((maj, min, pat, pre_num))
}

/// Run the official installation script to update the binary in-place
pub fn run_install_script(beta: bool) -> Result<(), anyhow::Error> {
    println!("Fetching and updating to the latest TAG release...");
    let script_url = "https://raw.githubusercontent.com/TheJonathanC/TAG/main/scripts/install.sh";
    let channel_env = if beta { "beta" } else { "stable" };

    let status = Command::new("bash")
        .env("CHANNEL", channel_env)
        .arg("-c")
        .arg(format!("curl -fsSL {} | bash", script_url))
        .status()?;

    if status.success() {
        println!("\nTAG has been updated to the latest version!");
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Update failed with script exit code: {:?}",
            status.code()
        ))
    }
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
}

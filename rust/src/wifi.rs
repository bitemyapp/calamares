// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only snapshot from the live user's NM session. Never log profile data.
use anyhow::{Result, ensure};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};
use zeroize::Zeroizing;

#[cfg(feature = "network")]
mod nm;
#[cfg(feature = "network")]
pub use nm::snapshot;

pub fn validate_profiles(profiles: &[String], username: &str) -> Result<()> {
    ensure!(
        profiles.len() <= 32 && profiles.iter().all(|p| p.len() <= 16384),
        "Too many or oversized Wi-Fi profiles"
    );
    let mut uuids = std::collections::BTreeSet::new();
    for profile in profiles {
        let normalized = normalize(profile, username)?;
        let uuid = normalized
            .lines()
            .find_map(|line| line.strip_prefix("uuid="))
            .ok_or_else(|| anyhow::anyhow!("Missing Wi-Fi profile identity"))?;
        ensure!(
            uuids.insert(uuid.to_string()),
            "Duplicate Wi-Fi profile identity"
        );
    }
    Ok(())
}

#[cfg(feature = "network")]
fn normalize(profile: &str, username: &str) -> Result<Zeroizing<String>> {
    nm::normalize(profile, username)
}

#[cfg(not(feature = "network"))]
fn normalize(_: &str, _: &str) -> Result<Zeroizing<String>> {
    anyhow::bail!("Wi-Fi support was not compiled into this binary")
}

pub fn write_profiles(root: &Path, profiles: &[String], username: &str) -> Result<()> {
    validate_profiles(profiles, username)?;
    if profiles.is_empty() {
        return Ok(());
    }
    let dir = root.join("etc/NetworkManager/system-connections");
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    for (index, profile) in profiles.iter().enumerate() {
        let normalized = normalize(profile, username)?;
        // Names are generated, never derived from an SSID or client path.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join(format!("installer-wifi-{index}.nmconnection")))?;
        file.write_all(normalized.as_bytes())?;
        file.sync_all()?;
    }
    Ok(())
}

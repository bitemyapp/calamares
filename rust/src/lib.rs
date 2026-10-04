// SPDX-License-Identifier: GPL-3.0-or-later
pub mod config;
pub mod desktop;
pub mod disk;
pub mod install;
pub mod process;
pub mod timezone;
pub mod wifi;
pub use desktop::Desktop;
#[cfg(test)]
mod validation_tests;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, os::unix::fs::MetadataExt, path::Path};
use zeroize::Zeroize;

pub const SETTINGS: &str = "/etc/calamares-nixos/settings.json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub template_dir: String,
    pub zoneinfo: String,
    pub state_version: String,
    pub kernel: Kernel,
    // Set only by a root-owned VM test configuration, never by the UI request.
    #[serde(default)]
    pub test_diagnostics: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kernel {
    Lts,
    Latest,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Firmware {
    Uefi,
    Bios,
}
impl Firmware {
    pub fn current() -> Self {
        if Path::new("/sys/firmware/efi").exists() {
            Self::Uefi
        } else {
            Self::Bios
        }
    }
}

// Deliberately no Debug/Clone for a request containing a plaintext secret.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub disk: disk::Identity,
    pub firmware: Firmware,
    pub hostname: String,
    pub username: String,
    pub full_name: String,
    pub password: String,
    pub locale: String,
    pub timezone: String,
    pub keyboard: String,
    pub desktops: Vec<Desktop>,
    pub default_desktop: Desktop,
    pub copy_wifi: bool,
    pub wifi_profiles: Vec<String>,
    pub allow_unfree: bool,
    pub confirmation: String,
}
impl Drop for Request {
    fn drop(&mut self) {
        self.password.zeroize();
        self.wifi_profiles.zeroize();
    }
}

pub const LOCALES: &[&str] = &[
    "en_US.UTF-8",
    "en_GB.UTF-8",
    "de_DE.UTF-8",
    "fr_FR.UTF-8",
    "es_ES.UTF-8",
    "it_IT.UTF-8",
    "ja_JP.UTF-8",
    "pt_BR.UTF-8",
];
pub const KEYBOARDS: &[&str] = &["us", "gb", "de", "fr", "es", "it", "jp", "br"];

fn trusted_component(path: &Path, uid: u32, mode: u32, directory: bool) -> bool {
    // Nix's store root is root:nixbld 1775 on the live image. The sticky
    // directory prevents builders from replacing root-owned store entries.
    // Every referenced entry below it is still checked for root ownership
    // and no group/other writes. Do not extend this exception to arbitrary
    // shared directories or to a store without the sticky bit.
    let store_root = path == Path::new("/nix/store") && directory && mode & 0o1777 == 0o1775;
    uid == 0 && (mode & 0o022 == 0 || store_root)
}

pub fn read_trusted(path: &Path) -> Result<Vec<u8>> {
    // All parents, including symlink targets, must be owned by root and not
    // writable by another user. NixOS /etc links into /nix/store are expected.
    let resolved = path.canonicalize()?;
    for p in path.ancestors().chain(resolved.ancestors()) {
        let m = fs::metadata(p)?;
        ensure!(
            fs::symlink_metadata(p)?.uid() == 0
                && trusted_component(p, m.uid(), m.mode(), m.is_dir()),
            "Untrusted installer configuration: {}",
            p.display()
        );
    }
    ensure!(
        fs::metadata(&resolved)?.is_file(),
        "Not a regular settings file"
    );
    ensure!(
        fs::metadata(&resolved)?.len() < 1024 * 1024,
        "Settings too large"
    );
    Ok(fs::read(resolved)?)
}

impl Settings {
    pub fn load() -> Result<Self> {
        let settings: Self = serde_json::from_slice(&read_trusted(Path::new(SETTINGS))?)?;
        ensure!(
            matches!(settings.state_version.as_str(), "26.05" | "26.11"),
            "Unsupported state version"
        );
        ensure!(
            Path::new(&settings.template_dir).is_absolute()
                && Path::new(&settings.zoneinfo).is_absolute(),
            "Settings paths must be absolute"
        );
        Ok(settings)
    }
}

pub fn validate(request: &Request, settings: &Settings) -> Result<()> {
    let h = request.hostname.as_bytes();
    ensure!(
        !h.is_empty()
            && h.len() <= 63
            && h[0].is_ascii_alphanumeric()
            && h[h.len() - 1].is_ascii_alphanumeric()
            && h.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-'),
        "Hostname must be one DNS label (letters, numbers and interior hyphens)"
    );
    let u = request.username.as_bytes();
    ensure!(
        !u.is_empty()
            && u.len() <= 31
            && u[0].is_ascii_lowercase()
            && u.iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_'),
        "Username must start with a lowercase letter and contain only lowercase letters, digits, hyphens or underscores"
    );
    ensure!(
        ![
            "root",
            "nixos",
            "daemon",
            "nobody",
            "systemd-network",
            "messagebus",
            "sshd",
            "polkituser",
            "sddm",
            "nixbld"
        ]
        .contains(&request.username.as_str())
            && !request.username.starts_with("nixbld"),
        "Reserved username"
    );
    ensure!(
        request.full_name.len() <= 128
            && !request.full_name.contains(':')
            && !request.full_name.chars().any(char::is_control),
        "Invalid full name"
    );
    ensure!(
        request.password.chars().count() >= 12
            && request.password.len() <= 1024
            && !request.password.chars().any(char::is_control),
        "Password must have at least 12 characters, at most 1024 bytes, and no control characters"
    );
    ensure!(
        LOCALES.contains(&request.locale.as_str()),
        "Unsupported locale"
    );
    ensure!(
        KEYBOARDS.contains(&request.keyboard.as_str()),
        "Unsupported keyboard layout"
    );
    timezone::validate(&request.timezone, Path::new(&settings.zoneinfo))?;
    ensure!(
        !request.desktops.is_empty() && request.desktops.len() <= Desktop::ALL.len(),
        "Select at least one desktop environment"
    );
    for (index, desktop) in request.desktops.iter().enumerate() {
        ensure!(
            !request.desktops[..index].contains(desktop),
            "Duplicate desktop selection"
        );
    }
    ensure!(
        !(request.desktops.contains(&Desktop::Gnome)
            && request.desktops.contains(&Desktop::Cinnamon)),
        "GNOME and Cinnamon cannot currently be combined: their pinned NixOS modules conflict on GSettings overrides. Select one of those two; other desktops can be combined."
    );
    ensure!(
        request.desktops.contains(&request.default_desktop),
        "The default session must be a selected desktop"
    );
    ensure!(
        request.copy_wifi || request.wifi_profiles.is_empty(),
        "Wi-Fi transfer is disabled but profiles were supplied"
    );
    wifi::validate_profiles(&request.wifi_profiles, &request.username)?;
    ensure!(
        request.confirmation == format!("ERASE {}", request.disk.path),
        "Type ERASE followed by the selected device path to confirm"
    );
    Ok(())
}

#[cfg(test)]
mod trust_tests {
    use super::*;
    #[test]
    fn sticky_nix_store_is_the_only_shared_directory_exception() {
        assert!(trusted_component(Path::new("/nix/store"), 0, 0o41775, true));
        assert!(trusted_component(
            Path::new("/nix/store/root-owned-entry"),
            0,
            0o40755,
            true
        ));
        assert!(!trusted_component(
            Path::new("/nix/store"),
            0,
            0o40775,
            true
        ));
        assert!(!trusted_component(
            Path::new("/nix/store"),
            1000,
            0o41775,
            true
        ));
        assert!(!trusted_component(
            Path::new("/nix/store"),
            0,
            0o101775,
            false
        ));
        assert!(!trusted_component(Path::new("/tmp"), 0, 0o41777, true));
        assert!(!trusted_component(
            Path::new("/nix/store/writable-entry"),
            0,
            0o40775,
            true
        ));
    }
}

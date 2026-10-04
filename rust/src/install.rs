// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    ConfirmedInstall, Firmware, Settings,
    config::{self, Template},
    disk, filesystem,
    process::output,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha_crypt::{Sha512Params, sha512_simple};
use std::{
    cell::Cell,
    fs,
    io::Write,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Progress { step: u8, message: String },
    Complete,
    Failed { message: String },
}
pub fn event(event: Event) {
    // A closed UI pipe must not panic and interrupt a destructive installation.
    if let Ok(text) = serde_json::to_string(&event) {
        let _ = writeln!(std::io::stdout(), "{text}");
        let _ = std::io::stdout().flush();
    }
}
fn progress(step: u8, message: &str) {
    event(Event::Progress {
        step,
        message: message.into(),
    });
}

pub fn live_guard(settings: &Settings) -> Result<()> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "The install helper requires root (use the GUI's authorization dialog)"
    );
    ensure!(
        fs::read_to_string("/etc/os-release")?
            .lines()
            .any(|l| l == "ID=nixos" || l == "ID=\"nixos\""),
        "Only NixOS live installation media are supported"
    );
    ensure!(
        output(
            "findmnt",
            &["--noheadings", "--output", "FSTYPE", "--mountpoint", "/"],
            15
        )?
        .trim()
            == "tmpfs",
        "Refusing to run on an installed host: root is not temporary"
    );
    ensure!(
        output(
            "findmnt",
            &["--noheadings", "--output", "FSTYPE", "--mountpoint", "/iso"],
            15
        )?
        .trim()
            == "iso9660",
        "No read-only live ISO filesystem at /iso"
    );
    if settings.test_diagnostics {
        disk::vm_test_disk()?;
    }
    Ok(())
}

// Never recursively delete this directory: an unmount failure must not turn
// cleanup into deletion of an installed filesystem.
struct Target(PathBuf, Cell<bool>);
impl Target {
    fn new() -> Result<Self> {
        Ok(Self(
            tempfile::Builder::new()
                .prefix("calamares-target-")
                .tempdir_in("/run")?
                .keep(),
            Cell::new(false),
        ))
    }
    fn unmount(&self) -> Result<()> {
        output("umount", &["--recursive", self.0.to_str().unwrap()], 60)?;
        self.1.set(false);
        Ok(())
    }
}
impl Drop for Target {
    fn drop(&mut self) {
        if self.1.get() {
            let _ = output("umount", &["--recursive", self.0.to_str().unwrap()], 60);
        }
        let _ = fs::remove_dir(&self.0);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    Preflight,
    Execute,
}

pub fn install(confirmed: ConfirmedInstall, mode: InstallMode) -> Result<()> {
    let mut request = confirmed.into_plan();
    let settings = request.settings().clone();
    live_guard(&settings)?;
    ensure!(
        !fs::read_to_string("/etc/passwd")?
            .lines()
            .any(|l| l.split(':').next() == Some(request.username().as_str())),
        "Username is already reserved by the live system"
    );
    ensure!(
        request.firmware() == Firmware::current(),
        "Firmware changed since review"
    );
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open("/run/calamares-nixos.lock")?;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Another installation is already running"
    );
    disk::revalidate(request.disk())?;
    request.filesystem().preflight(request.firmware())?;
    let template = Template::load(&settings, request.hostname())?;
    progress(
        0,
        "Checking installation media, settings, target identity and pinned inputs",
    );
    let staging = tempfile::Builder::new()
        .prefix("calamares-preflight-")
        .tempdir_in("/run")?;
    template.write(staging.path())?;
    // Resolve/cache locked sources BEFORE erasing anything. Never update a lock.
    output(
        "nix",
        &[
            "flake",
            "metadata",
            "--no-write-lock-file",
            "--json",
            &format!("path:{}", staging.path().display()),
        ],
        600,
    )?;
    ensure!(
        fs::read(staging.path().join("flake.lock"))? == template.lock,
        "Nix modified the pinned lock"
    );
    if mode == InstallMode::Preflight {
        progress(0, "Preflight passed; no disk writes were made");
        return Ok(());
    }
    let params = Sha512Params::new(100_000)
        .map_err(|e| anyhow::anyhow!("Password hashing parameters: {e:?}"))?;
    let password = request.take_password();
    let hash = Zeroizing::new(
        sha512_simple(&password, &params)
            .map_err(|_| anyhow::anyhow!("Password hashing failed"))?,
    );
    drop(password);
    // Lock while editing the partition table. Re-probe its stable identity and
    // all mount/holder state immediately before the first write.
    let device = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&request.disk().path)?;
    ensure!(
        unsafe { libc::flock(device.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Disk is locked by another installer"
    );
    disk::revalidate(request.disk())?;
    let dev = request.disk().path.as_str();
    let target = Target::new()?;
    let mount = target.0.to_str().context("Target path encoding")?;
    let storage = (|| -> Result<(String, Option<String>)> {
        progress(
            1,
            "Erasing the selected disk and creating a GPT partition table",
        );
        output("wipefs", &["--all", "--force", dev], 60)?;
        output("parted", &["--script", dev, "mklabel", "gpt"], 60)?;
        match request.firmware() {
            Firmware::Uefi => {
                output(
                    "parted",
                    &["--script", dev, "mkpart", "ESP", "fat32", "1MiB", "1025MiB"],
                    60,
                )?;
                output("parted", &["--script", dev, "set", "1", "esp", "on"], 60)?;
            }
            Firmware::Bios => {
                output(
                    "parted",
                    &["--script", dev, "mkpart", "BIOS", "1MiB", "3MiB"],
                    60,
                )?;
                output(
                    "parted",
                    &["--script", dev, "set", "1", "bios_grub", "on"],
                    60,
                )?;
            }
        }
        let start = if request.firmware() == Firmware::Uefi {
            "1025MiB"
        } else {
            "3MiB"
        };
        output(
            "parted",
            &[
                "--script",
                dev,
                "mkpart",
                "root",
                request.filesystem().name(),
                start,
                "100%",
            ],
            60,
        )?;
        // Keep udev from observing partially written superblocks. Partition nodes
        // come from the kernel/devtmpfs; do not wait for udev while holding its lock.
        output("blockdev", &["--rereadpt", dev], 30)?;
        let root = disk::partition(dev, 2);
        let boot = disk::partition(dev, 1);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match disk::verify_layout(request.disk(), request.firmware()) {
                Ok(()) => break,
                Err(error) if Instant::now() >= deadline => {
                    return Err(error.context("Kernel partition layout did not become ready"));
                }
                Err(_) => thread::sleep(Duration::from_millis(200)),
            }
        }
        disk::revalidate(request.disk())?;
        progress(2, "Formatting, verifying and mounting the new filesystems");
        request.filesystem().format(&root)?;
        if request.firmware() == Firmware::Uefi {
            filesystem::format_efi(&boot)?;
        } else {
            // A BIOS boot partition must not retain an old filesystem signature.
            output("wipefs", &["--all", "--force", &boot], 60)?;
        }
        device.sync_all()?;
        drop(device);
        // A fresh explicit change event updates UUID/type information, including
        // events skipped while locked. Never rely on pre-format udev/blkid caches.
        output("udevadm", &["trigger", "--action=change", &root, &boot], 30)?;
        output("udevadm", &["settle", "--timeout=60"], 70)?;
        disk::revalidate(request.disk())?;
        request.filesystem().mount(&root, mount)?;
        target.1.set(true);
        if request.firmware() == Firmware::Uefi {
            fs::create_dir(target.0.join("boot"))?;
            filesystem::mount_efi(&boot, target.0.join("boot").to_str().unwrap())?;
        }
        let root_uuid = filesystem::probe_uuid(&root, request.filesystem().name())?;
        let boot_uuid = if request.firmware() == Firmware::Uefi {
            Some(filesystem::probe_uuid(&boot, "vfat")?)
        } else {
            None
        };
        Ok((root_uuid, boot_uuid))
    })();
    let (root_uuid, boot_uuid) = storage.map_err(|error| filesystem::diagnose(error, dev))?;
    progress(
        3,
        "Generating hardware settings and the pinned Determinate flake",
    );
    output("nixos-generate-config", &["--root", mount], 120)?;
    let dir = target.0.join("etc/nixos");
    ensure!(
        dir.join("hardware-configuration.nix").is_file(),
        "Hardware configuration was not generated"
    );
    fs::write(
        dir.join("configuration.nix"),
        config::installed_configuration(&request, &root_uuid, boot_uuid.as_deref())?,
    )?;
    template.write(&dir)?;
    // Do not put secrets inside the flake source: use a sibling under /etc.
    // configuration.nix uses this external absolute runtime path.
    let secret_dir = target.0.join("etc/nixos-secrets");
    fs::create_dir(&secret_dir)?;
    fs::set_permissions(&secret_dir, fs::Permissions::from_mode(0o700))?;
    config::write_secret(&secret_dir.join("user-password.hash"), &hash)?;
    request.wifi().write_to(&target.0)?;
    progress(
        4,
        "Building and installing NixOS (downloads can take a while)",
    );
    output(
        "nixos-install",
        &[
            "--root",
            mount,
            "--flake",
            &format!("path:{}#{}", dir.display(), request.hostname().as_str()),
            "--no-root-passwd",
            "--no-channel-copy",
            "--option",
            "build-dir",
            "/nix/var/nix/builds",
        ],
        7200,
    )?;
    ensure!(
        fs::read(dir.join("flake.lock"))? == template.lock,
        "Installed lock differs from the installation media"
    );
    progress(5, "Flushing writes and unmounting the installed system");
    output("sync", &[], 120)?;
    target.unmount()?;
    progress(
        6,
        "Installation complete. Shut down, remove the installer media, then boot the installed disk.",
    );
    event(Event::Complete);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_never_recursively_deletes_contents() {
        let outer = tempfile::tempdir().unwrap();
        let path = outer.path().join("target");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), "important").unwrap();
        drop(Target(path.clone(), Cell::new(false)));
        assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "important");
    }
}

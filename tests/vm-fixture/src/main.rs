// SPDX-License-Identifier: GPL-3.0-or-later
//! Separate test crate. Never shipped in the installer package.
//! Fixed public test credentials, only inside serial/size/live-root guarded VMs.
use anyhow::{Context, Result, ensure};
use calamares_nixos::{Firmware, Kernel, Request, Settings, config, disk, process::output};
use std::{
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::{Command, Stdio},
};

const PASSWORD: &str = "Qemu-Only-Test-123!";
// Public synthetic network, no real wireless credentials or host NM access.
const TEST_WIFI: &str = "[connection]\nid=Installer synthetic WiFi\nuuid=135ea3d9-d456-44b1-ae42-1e7081f66666\ntype=wifi\npermissions=user:nixos:;\n[wifi]\nssid=Installer synthetic WiFi\nmode=infrastructure\n[wifi-security]\nkey-mgmt=wpa-psk\npsk=WiFi-Synthetic-Only-123!\npsk-flags=0\n[ipv4]\nmethod=auto\n[ipv6]\nmethod=auto\n";
fn invoke(request: Request, preflight: bool) -> Result<()> {
    let package = fs::read_to_string("/run/calamares-package-path")?;
    let helper = if std::env::var_os("CALAMARES_VM_DEV_BACKEND").is_some() {
        Path::new("/workspace/rust/target/x86_64-unknown-linux-musl/release/calamares-nixos-helper")
            .to_path_buf()
    } else {
        Path::new(package.trim()).join("bin/calamares-nixos-helper")
    };
    let mut child = Command::new(helper)
        .arg(if preflight { "preflight" } else { "install" })
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request)?)?;
    ensure!(
        child.wait()?.success(),
        "Packaged installation helper failed"
    );
    Ok(())
}
fn pam_authenticate(password: &str) -> Result<bool> {
    // Run su from an ordinary account, not root (which could bypass PAM).
    // Testing a wrong password first proves this is not a passwordless path.
    let mut child = Command::new("/run/current-system/sw/bin/runuser")
        .args([
            "--pty",
            "--user",
            "rusttest",
            "--",
            "/run/wrappers/bin/su",
            "--login",
            "rusttest",
            "--command",
            "id -un",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    use std::{
        io::Read,
        os::fd::AsRawFd,
        time::{Duration, Instant},
    };
    let mut input = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    unsafe {
        libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
    }
    let mut text = Vec::new();
    let mut sent = false;
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let mut buffer = [0; 1024];
        match stdout.read(&mut buffer) {
            Ok(n) => text.extend_from_slice(&buffer[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        };
        if !sent && String::from_utf8_lossy(&text).contains("Password:") {
            writeln!(input, "{password}")?;
            input.flush()?;
            sent = true;
        }
        if let Some(status) = child.try_wait()? {
            return Ok(sent
                && status.success()
                && String::from_utf8_lossy(&text)
                    .lines()
                    .any(|l| l.trim() == "rusttest"));
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("PAM test timed out waiting for an interactive response");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn guard() -> Result<()> {
    ensure!(unsafe { libc::geteuid() } == 0, "VM fixture requires root");
    ensure!(
        fs::read_to_string("/sys/class/block/vda/serial")?.trim() == "RESPIN_TEST_ONLY",
        "Not the disposable test VM"
    );
    ensure!(
        output("blockdev", &["--getsize64", "/dev/vda"], 10)?.trim() == "42949672960",
        "Not a 40 GiB virtual disk"
    );
    Ok(())
}
fn request() -> Result<Request> {
    Ok(Request {
        disk: disk::discover()?
            .into_iter()
            .find(|d| d.identity.path == "/dev/vda")
            .unwrap()
            .identity,
        firmware: Firmware::current(),
        hostname: "rust-test".into(),
        username: "rusttest".into(),
        full_name: "Rust ${literal} Test".into(),
        password: PASSWORD.into(),
        locale: "en_US.UTF-8".into(),
        timezone: "America/Chicago".into(),
        keyboard: "us".into(),
        desktops: vec![
            calamares_nixos::Desktop::Plasma,
            calamares_nixos::Desktop::Xfce,
        ],
        default_desktop: calamares_nixos::Desktop::Plasma,
        copy_wifi: true,
        wifi_profiles: vec![TEST_WIFI.into()],
        allow_unfree: false,
        confirmation: "ERASE /dev/vda".into(),
    })
}
fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("desktop-configurations") {
        // Non-destructive host-side generation for Nix module evaluation.
        // Produces the actual backend output, never a second implementation.
        use calamares_nixos::Desktop;
        let settings = Settings {
            template_dir: "/unused".into(),
            zoneinfo: "/unused".into(),
            state_version: "26.11".into(),
            kernel: Kernel::Lts,
            test_diagnostics: false,
        };
        let mut configs = std::collections::BTreeMap::new();
        for bits in 1u8..64 {
            let desktops: Vec<_> = Desktop::ALL
                .iter()
                .enumerate()
                .filter_map(|(i, d)| (bits & (1 << i) != 0).then_some(*d))
                .collect();
            if desktops.contains(&Desktop::Gnome) && desktops.contains(&Desktop::Cinnamon) {
                continue;
            }
            let r = Request {
                disk: disk::Identity {
                    path: "/dev/vda".into(),
                    major_minor: "252:0".into(),
                    bytes: 40 * 1024u64.pow(3),
                    serial: "test".into(),
                    wwn: "".into(),
                    model: "test".into(),
                },
                firmware: Firmware::Uefi,
                hostname: "desktop-test".into(),
                username: "alice".into(),
                full_name: "Test".into(),
                password: "Public-Test-Only!".into(),
                locale: "en_US.UTF-8".into(),
                timezone: "America/Chicago".into(),
                keyboard: "us".into(),
                default_desktop: desktops[0],
                desktops,
                copy_wifi: false,
                wifi_profiles: vec![],
                allow_unfree: false,
                confirmation: "ERASE /dev/vda".into(),
            };
            let name = r
                .desktops
                .iter()
                .map(|d| d.session())
                .collect::<Vec<_>>()
                .join("-");
            configs.insert(name, config::configuration(&r, &settings));
        }
        println!("{}", serde_json::to_string_pretty(&configs)?);
        return Ok(());
    }
    guard()?;
    match std::env::args().nth(1).as_deref() {
        Some("prepare-integrated") => {
            ensure!(
                output("findmnt", &["-n", "-o", "FSTYPE", "/"], 10)?.trim() == "tmpfs",
                "Not live media"
            );
            // Exercise the ISO's real configuration and package. Only enable
            // diagnostics in this guarded VM; never mutate a Nix store file.
            let mut settings = Settings::load()?;
            ensure!(!settings.test_diagnostics, "Diagnostics shipped enabled");
            config::Template::load(&settings, "rust-test")?;
            let helper =
                Path::new("/run/current-system/sw/bin/calamares-nixos-helper").canonicalize()?;
            let package = helper.parent().unwrap().parent().unwrap();
            ensure!(package.starts_with("/nix/store"), "Unpackaged helper");
            fs::write(
                "/run/calamares-package-path",
                package.as_os_str().as_encoded_bytes(),
            )?;
            settings.test_diagnostics = true;
            let pending = Path::new("/etc/calamares-nixos/vm-settings.json");
            use std::fs::OpenOptions;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .open(pending)?;
            file.write_all(&serde_json::to_vec(&settings)?)?;
            file.sync_all()?;
            fs::rename(pending, calamares_nixos::SETTINGS)?;
            let wifi =
                Path::new("/etc/NetworkManager/system-connections/installer-fixture.nmconnection");
            let mut profile = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(wifi)?;
            profile.write_all(TEST_WIFI.as_bytes())?;
            profile.sync_all()?;
            output("nmcli", &["connection", "load", wifi.to_str().unwrap()], 15)?;
            output("timedatectl", &["set-timezone", "America/Chicago"], 15)?;
            println!("PASS: original media settings validated; VM diagnostics enabled");
            println!("PASS: synthetic Wi-Fi profile seeded; live zone set to America/Chicago");
        }
        Some("prepare") => {
            ensure!(
                output("findmnt", &["-n", "-o", "FSTYPE", "/"], 10)?.trim() == "tmpfs",
                "Not live media"
            );
            let package = std::env::args()
                .nth(2)
                .ok_or_else(|| anyhow::anyhow!("Missing package path"))?;
            ensure!(
                package.starts_with("/nix/store/") && Path::new(&package).is_dir(),
                "Invalid package path"
            );
            fs::write("/run/calamares-package-path", package)?;
            let dir = Path::new("/etc/calamares-nixos");
            fs::create_dir_all(dir)?;
            fs::write(
                dir.join("flake.nix.in"),
                fs::read("/etc/determinate-installer/flake.nix.in")?,
            )?;
            fs::write(
                dir.join("flake.lock"),
                fs::read("/etc/determinate-installer/flake.lock")?,
            )?;
            let settings = Settings {
                template_dir: dir.to_string_lossy().into_owned(),
                zoneinfo: "/etc/zoneinfo".into(),
                state_version: "26.11".into(),
                kernel: Kernel::Lts,
                test_diagnostics: true,
            };
            // Locate the live tzdata through the root-owned localtime link.
            // The live ISO can leave /etc/localtime unset. Find its trusted
            // store tzdata for this fixture; production settings pin the path.
            let zones = fs::read_dir("/nix/store")?
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_name().to_string_lossy().contains("-tzdata-"))
                .map(|entry| entry.path().join("share/zoneinfo"))
                .find(|path| path.join("Etc/UTC").is_file())
                .context("Live ISO contains no tzdata store path")?;
            let settings = Settings {
                zoneinfo: zones.to_string_lossy().into_owned(),
                ..settings
            };
            fs::write(dir.join("settings.json"), serde_json::to_vec(&settings)?)?;
            println!("{}", serde_json::to_string(&request()?)?);
        }
        Some("install") => {
            ensure!(
                output("findmnt", &["-n", "-o", "FSTYPE", "/"], 10)?.trim() == "tmpfs",
                "Not live media"
            );
            ensure!(
                output("blkid", &["-p", "/dev/vda"], 10).is_err(),
                "Refusing an initialized test disk"
            );
            let before = output("sha256sum", &["/etc/calamares-nixos/flake.lock"], 10)?;
            // Bad confirmation must fail without creating a partition table.
            let mut bad = request()?;
            bad.confirmation = "ERASE /dev/not-the-disk".into();
            ensure!(
                invoke(bad, false).is_err(),
                "Accepted mismatched erase confirmation"
            );
            ensure!(
                output("blkid", &["-p", "/dev/vda"], 10).is_err(),
                "Negative test wrote to disk"
            );
            invoke(request()?, true)?;
            ensure!(
                output("blkid", &["-p", "/dev/vda"], 10).is_err(),
                "Preflight wrote to disk"
            );
            invoke(request()?, false)?;
            ensure!(
                before == output("sha256sum", &["/etc/calamares-nixos/flake.lock"], 10)?,
                "Media lock changed"
            );
        }
        Some("verify") => {
            ensure!(
                output("findmnt", &["-n", "-o", "FSTYPE", "/"], 10)?.trim() == "ext4",
                "Not installed root"
            );
            ensure!(
                output("hostname", &[], 10)?.trim() == "rust-test",
                "Wrong hostname"
            );
            ensure!(
                output("nix", &["--version"], 15)?.contains("Determinate Nix"),
                "Wrong Nix distribution"
            );
            println!("{}", output("nix", &["--version"], 15)?);
            println!("{}", output("fh", &["--version"], 15)?);
            output(
                "systemctl",
                &["is-active", "determinate-nixd.socket", "display-manager"],
                30,
            )?;
            let shadow = fs::read_to_string("/etc/shadow")?;
            let hash = shadow
                .lines()
                .find_map(|l| l.strip_prefix("rusttest:"))
                .unwrap()
                .split(':')
                .next()
                .unwrap();
            sha_crypt::sha512_check(PASSWORD, hash)
                .map_err(|_| anyhow::anyhow!("Installed password hash does not authenticate"))?;
            ensure!(
                !pam_authenticate("deliberately-wrong-password")?,
                "PAM unexpectedly accepted a wrong password"
            );
            ensure!(
                pam_authenticate(PASSWORD)?,
                "PAM rejected the installed user's password"
            );
            let root_hash = shadow
                .lines()
                .find_map(|l| l.strip_prefix("root:"))
                .unwrap()
                .split(':')
                .next()
                .unwrap();
            ensure!(
                root_hash.starts_with('!') || root_hash.starts_with('*'),
                "Root account is not locked"
            );
            ensure!(
                !shadow.lines().any(|l| l.starts_with("nixos:")),
                "Live user leaked into target"
            );
            let secret = Path::new("/etc/nixos-secrets/user-password.hash");
            let metadata = fs::metadata(secret)?;
            ensure!(
                metadata.uid() == 0 && metadata.permissions().mode() & 0o777 == 0o600,
                "Password file permissions"
            );
            ensure!(
                fs::read_to_string(secret)?.trim() == hash,
                "Runtime hash mismatch"
            );
            ensure!(
                !Path::new("/etc/nixos/secrets").exists(),
                "Secret inside flake source"
            );
            let configuration = fs::read_to_string("/etc/nixos/configuration.nix")?;
            let selected = calamares_nixos::Desktop::ALL
                .iter()
                .find(|d| configuration.contains(&format!("defaultSession = \"{}\";", d.session())))
                .context("No explicit default desktop")?;
            println!("DESKTOP_SESSION={}", selected.session());
            let enabled = calamares_nixos::Desktop::ALL
                .iter()
                .filter(|d| configuration.contains(&format!("{}.enable = true;", d.option())))
                .map(|d| d.session())
                .collect::<Vec<_>>()
                .join(",");
            println!("DESKTOPS={enabled}");
            ensure!(
                output(
                    "timedatectl",
                    &["show", "--property=Timezone", "--value"],
                    10
                )?
                .trim()
                    == "America/Chicago",
                "Installed time zone is not US Central"
            );
            let winter = output("date", &["--date=2026-01-15 12:00:00 UTC", "+%z %Z"], 10)?;
            let summer = output("date", &["--date=2026-07-15 12:00:00 UTC", "+%z %Z"], 10)?;
            ensure!(
                winter.trim() == "-0600 CST" && summer.trim() == "-0500 CDT",
                "Central daylight saving rules were not preserved"
            );
            let wifi =
                Path::new("/etc/NetworkManager/system-connections/installer-wifi-0.nmconnection");
            let metadata = fs::metadata(wifi)?;
            ensure!(
                metadata.uid() == 0 && metadata.permissions().mode() & 0o777 == 0o600,
                "Wi-Fi profile not root-only"
            );
            let profile = fs::read_to_string(wifi)?;
            ensure!(
                profile.contains("psk=WiFi-Synthetic-Only-123!")
                    && profile.contains("user:rusttest:;")
                    && !profile.contains("user:nixos:"),
                "Wi-Fi credentials or permissions not migrated"
            );
            ensure!(
                !configuration.contains("WiFi-Synthetic")
                    && !configuration.contains("Installer synthetic"),
                "Wi-Fi leaked into generated flake"
            );
            let known = output(
                "nmcli",
                &[
                    "-g",
                    "connection.type",
                    "connection",
                    "show",
                    "uuid",
                    "135ea3d9-d456-44b1-ae42-1e7081f66666",
                ],
                15,
            )?;
            ensure!(
                known.trim() == "802-11-wireless",
                "Installed NetworkManager did not load transferred Wi-Fi"
            );
            println!(
                "PASS: America/Chicago, winter CST/summer CDT; Wi-Fi credentials and user restrictions persisted root-only and loaded by NetworkManager"
            );
            ensure!(
                !configuration.contains(hash) && !configuration.contains(PASSWORD),
                "Secret in configuration"
            );
            ensure!(
                configuration.contains(&config::nix_string("Rust ${literal} Test")),
                "Nix interpolation escaping"
            );
            for path in [
                "/etc/determinate-installer",
                "/etc/calamares-nixos",
                "/run/current-system/sw/bin/calamares",
                "/run/current-system/sw/bin/calamares-nixos",
            ] {
                ensure!(!Path::new(path).exists(), "Live installer leaked: {path}");
            }
            output("id", &["rusttest"], 10)?;
            println!("{}", output("sha256sum", &["/etc/nixos/flake.lock"], 10)?);
            ensure!(
                output("id", &["-nG", "rusttest"], 10)?
                    .split_whitespace()
                    .any(|g| g == "wheel"),
                "User cannot administer machine"
            );
            println!(
                "PASS: installed system, password hash and PAM authentication, locked root, escaped configuration, desktop and Determinate services"
            );
        }
        _ => anyhow::bail!("prepare|prepare-integrated|install|verify"),
    }
    Ok(())
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! NixOS option semantics adapted from calamares-nixos-extensions 0.3.23.
//! No Python/C++ module or global-storage hooks are executed by this installer.
use crate::{Desktop, Filesystem, Firmware, Hostname, InstallPlan, Kernel, Settings, read_trusted};
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeSet, fs, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

pub fn nix_string(value: &str) -> String {
    // JSON quoting does not escape Nix interpolation. Escape dollars *after*
    // JSON encoding so a literal ${...} cannot become an executable expression.
    serde_json::to_string(value)
        .expect("string serialization")
        .replace("${", "\\${")
}

pub struct Template {
    pub flake: String,
    pub lock: Vec<u8>,
}
impl Template {
    pub fn load(settings: &Settings, hostname: &Hostname) -> Result<Self> {
        let dir = Path::new(&settings.template_dir);
        let template = String::from_utf8(read_trusted(&dir.join("flake.nix.in"))?)?;
        let lock = read_trusted(&dir.join("flake.lock"))?;
        Self::parse(template, lock, hostname)
    }
    pub fn parse(template: String, lock: Vec<u8>, hostname: &Hostname) -> Result<Self> {
        ensure!(
            template.matches("@HOSTNAME@").count() == 1,
            "Template must contain exactly one hostname placeholder"
        );
        let value: serde_json::Value = serde_json::from_slice(&lock)?;
        ensure!(value["version"] == 7, "Unsupported lock version");
        let root = value["root"].as_str().context("Missing lock root")?;
        let inputs = value["nodes"][root]["inputs"]
            .as_object()
            .context("Missing inputs")?;
        ensure!(
            inputs.keys().map(String::as_str).collect::<BTreeSet<_>>()
                == BTreeSet::from(["nixpkgs", "determinate", "fh"]),
            "Unexpected target flake inputs"
        );
        for name in ["nixpkgs", "determinate", "fh"] {
            let node = inputs[name]
                .as_str()
                .context("Expected a directly locked input")?;
            ensure!(
                value["nodes"][node]["locked"]["narHash"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("sha256-")),
                "Unlocked input: {name}"
            );
        }
        Ok(Self {
            flake: template.replace("@HOSTNAME@", &nix_string(hostname.as_str())),
            lock,
        })
    }
    pub fn write(&self, dir: &Path) -> Result<()> {
        fs::write(dir.join("flake.nix"), &self.flake)?;
        fs::write(dir.join("flake.lock"), &self.lock)?;
        Ok(())
    }
}

pub fn configuration(request: &InstallPlan) -> String {
    render_configuration(request, "")
}

/// Pin the freshly probed identities instead of trusting hardware detection's
/// choice among potentially stale /dev/disk/by-uuid aliases.
pub fn installed_configuration(
    request: &InstallPlan,
    root_uuid: &str,
    boot_uuid: Option<&str>,
) -> Result<String> {
    crate::filesystem::check_uuid(root_uuid, request.filesystem().name())?;
    ensure!(
        boot_uuid.is_some() == (request.firmware() == Firmware::Uefi),
        "EFI filesystem identity does not match the reviewed firmware"
    );
    let mut devices = format!(
        "  # Use UUIDs read directly from the newly formatted filesystems.\n  fileSystems.\"/\".device = lib.mkForce {};\n",
        nix_string(&format!("/dev/disk/by-uuid/{root_uuid}"))
    );
    if let Some(uuid) = boot_uuid {
        crate::filesystem::check_uuid(uuid, "vfat")?;
        devices.push_str(&format!(
            "  fileSystems.\"/boot\".device = lib.mkForce {};\n",
            nix_string(&format!("/dev/disk/by-uuid/{uuid}"))
        ));
    }
    Ok(render_configuration(request, &devices))
}

fn render_configuration(request: &InstallPlan, filesystem_devices: &str) -> String {
    let settings = request.settings();
    let q = nix_string;
    let boot = match request.firmware() {
        Firmware::Uefi => "boot.loader.systemd-boot.enable = true;\n  boot.loader.efi.canTouchEfiVariables = true;".into(),
        Firmware::Bios => format!("boot.loader.grub.enable = true;\n  boot.loader.grub.device = {};\n  boot.loader.grub.useOSProber = false;", q(&request.disk().path)),
    };
    let kernel = match settings.kernel {
        Kernel::Latest => "  boot.kernelPackages = pkgs.linuxPackages_latest;\n",
        Kernel::Lts => "",
    };
    // Upstream hardware detection preserves Btrfs subvolumes, but not compression.
    let filesystem_options = if request.filesystem() == Filesystem::Btrfs {
        "  fileSystems.\"/\".options = [ \"compress=zstd\" ];\n"
    } else {
        ""
    };
    let diagnostic = if settings.test_diagnostics {
        "  # Disposable QEMU verification only.\n  services.qemuGuest.enable = true;\n  boot.kernelParams = [ \"console=ttyS0,115200n8\" \"console=tty0\" ];\n"
    } else {
        ""
    };
    let desktops = request
        .desktops()
        .selected()
        .iter()
        .map(|desktop| format!("  {}.enable = true;\n", desktop.option()))
        .collect::<String>();
    let default_session = q(request.desktops().default().session());
    let display_manager = if request.desktops().selected() == [Desktop::Gnome] {
        "gdm"
    } else {
        "sddm"
    };
    // Multiple desktops set equally-prioritized defaults for these options.
    // Resolve the shared helpers explicitly, while portals remain per-session.
    let helpers = if request.desktops().contains(Desktop::Plasma) {
        "  programs.ssh.askPassword = \"${pkgs.kdePackages.ksshaskpass}/bin/ksshaskpass\";\n  programs.gnupg.agent.pinentryPackage = pkgs.pinentry-qt;\n"
    } else {
        "  programs.ssh.askPassword = \"${pkgs.x11_ssh_askpass}/libexec/x11-ssh-askpass\";\n  programs.gnupg.agent.pinentryPackage = pkgs.pinentry-gnome3;\n"
    };
    format!(
        r#"# Generated by the NixOS-focused Rust Calamares installer.
{{ config, lib, pkgs, ... }}: {{
  imports = [ ./hardware-configuration.nix ];
  {boot}
{kernel}{filesystem_options}{filesystem_devices}  networking.hostName = {hostname};
  networking.networkmanager.enable = true;
  # Include redistributable device firmware even when additional unfree
  # packages are declined. This is not a strictly free-software-only system.
  hardware.enableRedistributableFirmware = true;
  time.timeZone = {timezone};
  i18n.defaultLocale = {locale};
  services.xserver.enable = true;
  services.xserver.xkb.layout = {keyboard};
  console.useXkbConfig = true;
  services.displayManager.{display_manager}.enable = true;
  services.displayManager.defaultSession = {default_session};
{desktops}{helpers}  # Each desktop contributes its own session-specific portal configuration.
  services.printing.enable = true;
  security.rtkit.enable = true;
  services.pipewire = {{ enable = true; alsa.enable = true; alsa.support32Bit = true; pulse.enable = true; }};
  users.mutableUsers = true;
  users.users.root.initialHashedPassword = "!";
  users.users.{username} = {{
    isNormalUser = true;
    description = {full_name};
    extraGroups = [ "networkmanager" "wheel" ];
    # A root-only file on the target, never a Nix store source path.
    hashedPasswordFile = "/etc/nixos-secrets/user-password.hash";
  }};
  programs.firefox.enable = true;
  nixpkgs.config.allowUnfree = {unfree};
  system.stateVersion = {state};
{diagnostic}}}
"#,
        hostname = q(request.hostname().as_str()),
        timezone = q(request.timezone().as_str()),
        locale = q(request.locale()),
        keyboard = q(request.keyboard()),
        username = q(request.username().as_str()),
        full_name = q(request.full_name()),
        unfree = request.allow_unfree(),
        state = q(&settings.state_version)
    )
}

pub fn write_secret(path: &Path, hash: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    writeln!(file, "{hash}")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escape_interpolation_and_quotes() {
        assert_eq!(nix_string("${abort \"x\"}"), "\"\\${abort \\\"x\\\"}\"");
        assert_eq!(nix_string("x\\y"), "\"x\\\\y\"");
    }
    #[test]
    fn lock_and_placeholder_are_mandatory() {
        let hostname = Hostname::parse("test").unwrap();
        assert!(Template::parse("no placeholder".into(), b"{}".to_vec(), &hostname).is_err());
        assert!(
            Template::parse(
                "@HOSTNAME@".into(),
                br#"{"version":7,"root":"root","nodes":{"root":{"inputs":{}}}}"#.to_vec(),
                &hostname
            )
            .is_err()
        );
    }
    #[test]
    fn secrets_are_exclusive_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("hash");
        write_secret(&path, "$6$test").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(write_secret(&path, "overwrite").is_err());
    }
}

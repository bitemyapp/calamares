# NixOS-focused Rust Calamares

This branch contains a new native Rust/GTK4 installer for the Determinate NixOS
graphical respin. It is **not** a translation of every upstream Calamares module,
and it is not compatible with Calamares's Python/C++ plugin API. The original
upstream source and licenses remain in the repository for provenance; the Nix
package builds only `rust/`.

The GitHub fork preserves Calamares history. Rust development starts at upstream
Codeberg tag `v3.4.2`, commit `36d30c492e5c7b5d6d32fed5c5d9790522e1eea3`.
The new implementation is GPL-3.0-or-later. It has no upstream endorsement.

## Supported first-release workflow

- x86_64 NixOS live ISO; network connection required. The live desktop is Plasma.
- Multi-select installed desktops: Plasma (default), GNOME, Xfce, Cinnamon,
  MATE and LXQt, with an explicit default login session. GNOME and Cinnamon
  cannot be combined because the pinned NixOS modules conflict on GSettings.
- Guided erase of one whole disk, GPT, ext4, EFI/systemd-boot or BIOS/GRUB.
- Hostname, normal user with sudo, password, full name, time zone, a selection
  of eight system locales and keyboard layouts. Allowing unfree packages is
  enabled by default, with an explicit checkbox opt-out and review summary.
- Pinned Nixpkgs, Determinate Nix and `fh` inputs supplied by the installation
  media. The installed system keeps the same lock file.

No manual partitioning, existing-OS preservation, encryption, RAID/LVM, Btrfs,
offline installation, upstream plugins or interface
translations are implemented. The GUI says this before offering installation.
Treat this as an experimental, NixOS-focused fork; VM verification is not a
guarantee for every physical machine. Do not erase irreplaceable data without a
backup.

### Wi-Fi and time zone

Every generated desktop configuration enables NetworkManager and redistributable
device firmware. That firmware can be proprietary; unchecking the additional
unfree-packages option does not promise a strictly free-software-only system.
The unfree setting permits packages, not automatic selection of every vendor
driver. NVIDIA/hybrid-GPU configuration remains hardware-specific. The upstream
hardware generator still supplies detected storage, CPU microcode and device
settings; the installer does not force NVIDIA or Broadcom drivers on all PCs.

Wi-Fi transfer is enabled by default, with an opt-out and a profile count on
the review page. The GUI worker queries NetworkManager as the live user, so
its unlocked Secret Agent/wallet can supply saved passwords. The helper
independently parses and validates the profiles with libnm before erasing.
It writes root-owned mode-0600 keyfiles under
`/etc/NetworkManager/system-connections`, never into the flake or Nix store.
Live-user restrictions are remapped to the installed username; agent-owned
credentials become system-keyfile secrets. Ethernet and VPN profiles are not
copied. Live settings are never modified. Missing secrets or external EAP
certificate/key references stop review rather than silently installing a
broken connection: unlock/save the live connection or disable transfer and
configure that enterprise network afterwards. No actual host Wi-Fi profiles
are accessed by the tests; they use public synthetic credentials in guests.

Time-zone detection prefers the live system's regional IANA zone. An unset/UTC
live default triggers a bounded HTTPS request to `https://ipapi.co/timezone/`;
the service sees the public IP address, not Wi-Fi data. Internet detection can
be disabled. Its approximate result is identified as such (VPN/mobile routing
can be wrong), and a user must review and confirm the zone. Manual edits win
over late responses. Offline or invalid responses never silently guess a US
zone. `America/Chicago` retains Central daylight-saving rules; no locale,
country or numeric-offset-to-zone inference is used. Detection runs on a worker
with its own progress indicator. Selecting a zone does not change the live OS.

## Architecture and safeguards

`calamares-nixos` runs as the live user. GTK callbacks only handle widgets and
bounded channel messages. Disk discovery, filesystem reads, validation,
authorization and helper communication run on workers; hashing, downloads,
partitioning and installation run in the privileged Rust helper. The UI shows
an active spinner throughout work. No shell command is assembled from user
input by the installer.

`calamares-nixos-helper` accepts a strict JSON request on stdin. The GUI starts
it through NixOS's setuid Polkit wrapper, using a policy restricted to this
executable. Passwords never appear in argv, process titles or progress messages.
The backend independently parses the request and requires root, NixOS,
a temporary root filesystem, a mounted live ISO and root-owned settings.

### Parse once per process boundary

The strict JSON `RawRequest` is only a transport/form object. Consuming it with
`parse` produces an immutable `InstallPlan`; configuration generation accepts
only that plan. `Hostname`, `Username`, and `TimeZone` retain checked values.
`DesktopSelection` keeps the nonempty, unique, compatible selection together
with a default that belongs to it. `WifiTransfer` represents either opt-out or
a bounded collection of parsed, normalized profiles with unique connection
identities. The writer uses those retained bytes, without parsing them again.

Review does not fabricate an erase phrase. A separate consuming `confirm`
transition binds the exact phrase to the plan's disk and returns
`ConfirmedInstall`, the only input accepted by the executor. Serializing for
IPC deliberately returns to raw data: the privileged helper independently
parses and confirms it using its own root-owned settings. Neither plan type
implements `Deserialize`, `Debug`, or `Clone`; password/profile owners zeroize
their Rust buffers when dropped. External GTK/libnm allocations are not claimed
to be zeroized.

This is intentionally not a type for every string or process step. Full names
remain strings; locale/keyboard choices resolve to allowed static values;
cross-field rules belong in aggregate constructors. Runtime checks still guard
live/root status, current firmware, mount state and disk identity immediately
before writes. A parsed plan describes intent, not permanently safe hardware.
Parsing that reads tzdata or calls libnm runs on workers, not the GTK thread.

No target is selected automatically. The review page requires both a destructive
checkbox and typing `ERASE /dev/<device>`. Before writing, the helper rechecks
the device path, major/minor, size, serial, WWN and model. Mounted filesystems,
live media, active swap, device-mapper/RAID holders, read-only devices, virtual
memory devices and disks below 24 GiB are rejected. The `lsblk` device tree is
explicitly requested; a flat partition list fails closed. Installer and device
locks prevent concurrent cooperating instances. Disconnecting disks during an
installation is unsupported.

The helper hashes the password with a random salt and SHA-512 crypt (100,000
rounds), then zeroizes its plaintext buffer. A root-only hash file lives at
`/etc/nixos-secrets/user-password.hash`, **outside** the `/etc/nixos` flake
source. Nix references it as a runtime string path, not a store path. Root login
is locked. The normal user belongs to `wheel`. To make a password change persist
through subsequent NixOS activation, update/remove the declarative
`hashedPasswordFile` setting as appropriate; protect the hash file and backups.

Once destructive work starts the window cannot be closed normally. Failures are
reported as incomplete installation, never success. Command timeouts terminate
only the process group spawned by that operation. Cleanup attempts to unmount;
it never recursively deletes a target mount directory. There is no automatic
reboot. The installer cannot promise rollback after a partition table is erased.

## Build and test

```
cargo test --manifest-path rust/Cargo.toml --locked
cargo clippy --manifest-path rust/Cargo.toml --all-targets --all-features -- -D warnings
cargo fmt --manifest-path rust/Cargo.toml --check
nix build --no-write-lock-file
```

Local GUI builds need GTK4 and libnm development libraries. Nix supplies them. The Nix
package includes the GUI, helper, desktop entry and Polkit policy; it deliberately
does not contain the separate `tests/vm-fixture` crate.

Root-owned `/etc/calamares-nixos/settings.json` supplies `template_dir`,
`zoneinfo`, `state_version` (`26.11` or `26.05`), and `kernel` (`lts` or `latest`).
The template directory contains `flake.nix.in` with exactly one `@HOSTNAME@`
placeholder and a version-7 lock with exactly `nixpkgs`, `determinate`, `fh` root
inputs. `test_diagnostics` defaults false; it enables QEMU diagnostics only when
the guarded disposable test disk is present. It is not accepted in UI requests.

`scripts/build-rootless.rs` reuses the graphical respin's existing 100 GiB
regular-file-backed builder VM. `scripts/verify-qemu.rs` boots the verified
baseline ISO read-only as USB media and creates a fresh 40 GiB regular-file disk.
Both scripts use pinned Rust tooling from the published respin migration commit.
They require rust-script, QEMU/KVM, and the prerequisite respin builder setup.

```
scripts/build-rootless.rs /path/to/determinate-nixos-graphical /path/to/bootstrap.iso
scripts/verify-qemu.rs /path/to/verified-graphical.iso uefi
scripts/verify-qemu.rs /path/to/verified-graphical.iso bios
```

For faster backend iteration, append `--dev-backend` to use a newly compiled
static Rust helper with the last packaged GUI. Such runs are explicitly marked
development-only in their result JSON and do not count as release verification.
Repeat without that flag against a fresh Nix build before publishing a release.

Use executable shebangs or `rust-script --force` so shared-source edits are
recompiled. Tests launch the actual Nix-packaged GUI and helper. The automation
invokes the helper's normal stdin protocol for installation; a GUI screenshot
alone does not verify every interactive widget path. The installed system boots
without the ISO, and tests check its password hash, PAM authentication with wrong
and correct passwords through a pseudo-terminal, locked root, desktop,
Determinate services and absence of live-only installer configuration. This is
not itself an interactive graphical desktop-login test. The integrated ISO's
supervised GUI tests and desktop-login evidence are recorded in the graphical
repository's [verification report](https://github.com/bitemyapp/determinate-nixos-graphical/blob/codex/rust-calamares-integration/TESTING.md).

Artifacts and disposable disks stay under ignored `artifacts/` and `.work/`.
No host sudo, real block-device write, physical USB modification or host reboot
is needed for these tests.

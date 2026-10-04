# Rust installer verification

These results concern the NixOS-focused Rust implementation, not upstream
Calamares's distribution/plugin feature set. The first-release scope and
limitations are in [RUST-INSTALLER.md](RUST-INSTALLER.md).

## Build and local checks

- Locked Nix build on Nixpkgs `c59305bab2065cfecc4944690d9eedbb56f3a9fa`.
- 12 backend unit tests and one GUI selection test pass in the Nix sandbox.
- Rustfmt and Clippy with warnings denied, including the separate VM fixture.
- Latest package: `/nix/store/091zgxadxyww1yvmzkj59611n7cwp0jn-calamares-nixos-rust-0.1.0`.

## Disposable-machine integration tests

The baseline graphical ISO SHA-256 was
`3dd85d328e420af4c88c016b45dc9c4f69460c82b7d628218371b17110676447`.
It was attached read-only as USB mass storage. Each run used a new 40 GiB
regular-file-backed virtual target with serial `RESPIN_TEST_ONLY`, followed by
an orderly shutdown and a boot with **no ISO attached**.

| Firmware | Successful run | Packaged backend |
| --- | --- | --- |
| UEFI | `uefi-1791074916109-48874` | `f3h45hixda4zx0q87814gfrxz69in2rw` |
| BIOS | `bios-1791075333637-50870` | `wcgx3laar7k88rw0nfh1rcqnxfa61v4p` |
| UEFI, final package | `uefi-1791075670909-51959` | `091zgxadxyww1yvmzkj59611n7cwp0jn` |

All three runs exercised the real Nix-packaged helper, wrong-erase-confirmation
rejection without partitioning, non-destructive preflight, whole-disk
installation, installed-system boot, and Determinate Nix 3.23.0 / Nix 2.35.2
and `fh` 0.1.27. Installed-system checks included desktop services, root account
locking, the normal user's password hash, actual PAM rejection/acceptance of
wrong/correct passwords through a pseudo-terminal, protected password-file
permissions outside the flake, and absence of live installer settings.
The full name `Rust ${literal} Test` remained literal, testing Nix escaping.

The real GTK GUI was launched unprivileged and screenshots inspected, including
the unselected-disk placeholder and visible review button. Installed SDDM login
screens were visually checked. These component runs invoke installation through
the helper protocol, not by clicking every GUI control or logging into Plasma.
The later graphical ISO integration must verify its own autostart and Polkit
wiring; package-only testing cannot establish those properties.

## Development failures retained locally

- A test fixture assumed `/etc/localtime` existed on the live ISO; it now uses
  trusted tzdata. Production media supplies an explicit immutable tzdata path.
- Holding the device flock while waiting for udev stalled partition events;
  the lock is now released/closed before settling, while the installer-wide
  lock stays held. A fresh packaged install verified the fix.
- Initial PAM automation did not wait for the password prompt; it now does,
  keeps the PTY input open, and tests a wrong password first.
- Early screenshots exposed an automatic disk selection and a clipped review
  button. A real placeholder and a fixed footer corrected these UI issues.

Failed/development runs were not relabeled as passes. Their logs and disks
remain in ignored local artifact directories. No host sudo, host block device,
physical USB write or host reboot was used. Secure Boot, physical hardware,
encrypted/manual partitioning and upstream plugin compatibility are untested
and are not claimed.

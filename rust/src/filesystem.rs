// SPDX-License-Identifier: GPL-3.0-or-later
//! Closed filesystem choices and the shared format/probe/mount protocol.
use crate::{Firmware, process::output};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Filesystem {
    #[default]
    Ext4,
    Btrfs,
    Xfs,
}
impl Filesystem {
    pub const ALL: [Self; 3] = [Self::Ext4, Self::Btrfs, Self::Xfs];
    pub fn name(self) -> &'static str {
        match self {
            Self::Ext4 => "ext4",
            Self::Btrfs => "btrfs",
            Self::Xfs => "xfs",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Ext4 => "ext4 — general purpose (default)",
            Self::Btrfs => "Btrfs — checksums and compression",
            Self::Xfs => "XFS — large files and parallel I/O",
        }
    }
    fn formatter(self) -> &'static str {
        match self {
            Self::Ext4 => "mkfs.ext4",
            Self::Btrfs => "mkfs.btrfs",
            Self::Xfs => "mkfs.xfs",
        }
    }
    pub(crate) fn preflight(self, firmware: Firmware) -> Result<()> {
        preflight_with(self, firmware, output, || {
            Ok(fs::read_to_string("/proc/filesystems")?)
        })
    }
    pub(crate) fn format(self, device: &str) -> Result<()> {
        format_with(self, device, output)
            .with_context(|| format!("Formatting {device} as {}", self.name()))
    }
    pub(crate) fn mount(self, device: &str, target: &str) -> Result<()> {
        mount_with(
            self.name(),
            device,
            target,
            if self == Self::Btrfs {
                "compress=zstd"
            } else {
                "defaults"
            },
            output,
        )
    }
}

fn preflight_with(
    filesystem: Filesystem,
    firmware: Firmware,
    mut run: impl FnMut(&str, &[&str], u64) -> Result<String>,
    read_filesystems: impl FnOnce() -> Result<String>,
) -> Result<()> {
    run(filesystem.formatter(), &[if filesystem == Filesystem::Btrfs { "--version" } else { "-V" }], 15)
        .context("The installation media is missing a working filesystem formatter; no disk writes were made")?;
    run("modprobe", &[filesystem.name()], 15)?;
    if firmware == Firmware::Uefi {
        run("mkfs.fat", &["--help"], 15)?;
        run("modprobe", &["vfat"], 15)?;
    }
    let supported = read_filesystems()?;
    for name in [
        Some(filesystem.name()),
        (firmware == Firmware::Uefi).then_some("vfat"),
    ]
    .into_iter()
    .flatten()
    {
        ensure!(
            supported
                .lines()
                .any(|line| line.split_whitespace().last() == Some(name)),
            "The live kernel does not support {name}; no disk writes were made"
        );
    }
    Ok(())
}

fn verify_with(
    device: &str,
    expected: &str,
    run: &mut impl FnMut(&str, &[&str], u64) -> Result<String>,
) -> Result<()> {
    // Low-level probing bypasses both blkid's cache and udev's cached ID_FS_TYPE.
    // An ambiguous signature (exit 8) is an error, never a reason to guess.
    let actual = run(
        "blkid",
        &[
            "--probe",
            "--output",
            "value",
            "--match-tag",
            "TYPE",
            device,
        ],
        30,
    )?;
    ensure!(
        actual.trim() == expected,
        "Filesystem verification failed on {device}: expected {expected}, found {:?}",
        actual.trim()
    );
    Ok(())
}

pub(crate) fn check_uuid(uuid: &str, kind: &str) -> Result<()> {
    let (length, separators): (usize, &[usize]) = if kind == "vfat" {
        (9, &[4])
    } else {
        (36, &[8, 13, 18, 23])
    };
    ensure!(
        uuid.len() == length
            && uuid.bytes().enumerate().all(|(index, byte)| {
                if separators.contains(&index) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            }),
        "Invalid {kind} filesystem UUID: {uuid:?}"
    );
    Ok(())
}

fn uuid_from_probe(probe: &str, expected: &str) -> Result<String> {
    let field = |name: &str| -> Result<&str> {
        let prefix = format!("{name}=");
        let values = probe
            .lines()
            .filter_map(|line| line.strip_prefix(&prefix))
            .collect::<Vec<_>>();
        ensure!(
            values.len() == 1,
            "Missing or ambiguous {name} in filesystem probe"
        );
        Ok(values[0])
    };
    ensure!(
        field("TYPE")? == expected,
        "Filesystem type changed during UUID probing"
    );
    let uuid = field("UUID")?;
    check_uuid(uuid, expected)?;
    Ok(uuid.to_owned())
}

pub(crate) fn probe_uuid(device: &str, expected: &str) -> Result<String> {
    // Old /dev/disk/by-uuid aliases can outlive a reformat even after udev
    // settles. Read the superblock itself, never select an alias by its rdev.
    let probe = output("blkid", &["--probe", "--output", "export", device], 30)?;
    uuid_from_probe(&probe, expected)
        .with_context(|| format!("Reading the new filesystem UUID on {device}"))
}
fn format_with(
    filesystem: Filesystem,
    device: &str,
    mut run: impl FnMut(&str, &[&str], u64) -> Result<String>,
) -> Result<()> {
    // Wiping the whole disk only removes its partition table, not signatures
    // inside its partitions. Do this even when mkfs would accept -f/-F.
    run("wipefs", &["--all", "--force", device], 60)?;
    run(
        filesystem.formatter(),
        &[
            if filesystem == Filesystem::Ext4 {
                "-F"
            } else {
                "-f"
            },
            device,
        ],
        300,
    )?;
    verify_with(device, filesystem.name(), &mut run)
}
pub(crate) fn format_efi(device: &str) -> Result<()> {
    output("wipefs", &["--all", "--force", device], 60)?;
    output("mkfs.fat", &["-F", "32", device], 60)?;
    verify_with(device, "vfat", &mut output)
}
pub(crate) fn mount_efi(device: &str, target: &str) -> Result<()> {
    mount_with("vfat", device, target, "umask=0077", output)
}
fn mount_with(
    kind: &str,
    device: &str,
    target: &str,
    options: &str,
    mut run: impl FnMut(&str, &[&str], u64) -> Result<String>,
) -> Result<()> {
    verify_with(device, kind, &mut run)?;
    run(
        "mount",
        &["--types", kind, "--options", options, "--", device, target],
        30,
    )
    .with_context(|| format!("Mounting verified {kind} filesystem {device} at {target}"))?;
    Ok(())
}

pub(crate) fn diagnose(error: anyhow::Error, disk: &str) -> anyhow::Error {
    let mut report = format!("Storage preparation failed on {disk}: {error:#}\n");
    let root = crate::disk::partition(disk, 2);
    let boot = crate::disk::partition(disk, 1);
    for (program, args) in [
        (
            "lsblk",
            vec![
                "--bytes",
                "--output",
                "PATH,TYPE,SIZE,FSTYPE,FSVER,UUID,MOUNTPOINTS",
                disk,
            ],
        ),
        ("blkid", vec!["--probe", root.as_str()]),
        ("blkid", vec!["--probe", boot.as_str()]),
        ("dmesg", vec!["--ctime", "--level=err,warn"]),
    ] {
        let text = match output(program, &args, 15) {
            Ok(text) => text,
            Err(error) => format!("{error:#}"),
        };
        let tail: String = text
            .chars()
            .rev()
            .take(12000)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        report.push_str(&format!("\n{program}:\n{tail}\n"));
    }
    let saved = (|| -> Result<_> {
        // NamedTempFile is mode 0600; diagnostics are local and contain no request secrets.
        let mut file = tempfile::Builder::new()
            .prefix("calamares-storage-")
            .suffix(".log")
            .tempfile_in("/run")?;
        file.write_all(report.as_bytes())?;
        let (_, path) = file.keep()?;
        Ok(path)
    })();
    match saved {
        Ok(path) => error.context(format!(
            "Could not prepare {disk}. Storage diagnostics saved to {} (copy before rebooting).",
            path.display()
        )),
        Err(save_error) => error.context(format!(
            "Could not prepare {disk}; saving storage diagnostics also failed: {save_error}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filesystem_uuid_requires_one_matching_unambiguous_identity() {
        let uuid = "11111111-2222-3333-4444-555555555555";
        assert_eq!(
            uuid_from_probe(&format!("TYPE=ext4\nUUID={uuid}\n"), "ext4").unwrap(),
            uuid
        );
        assert_eq!(
            uuid_from_probe("TYPE=vfat\nUUID=60F4-E5D4\n", "vfat").unwrap(),
            "60F4-E5D4"
        );
        for probe in [
            "TYPE=ext4\n",
            "TYPE=vfat\nUUID=60F4-E5D4\n",
            "TYPE=ext4\nUUID=\n",
            "TYPE=ext4\nUUID=../../device\n",
            "TYPE=ext4\nUUID=11111111-2222-3333-4444-555555555555\nUUID=11111111-2222-3333-4444-555555555555\n",
            "TYPE=ext4\nTYPE=xfs\nUUID=11111111-2222-3333-4444-555555555555\n",
        ] {
            assert!(uuid_from_probe(probe, "ext4").is_err(), "{probe}");
        }
    }
    #[test]
    fn refuses_wrong_or_ambiguous_signatures_before_mounting() {
        for answer in [
            Ok("ntfs\n".into()),
            Ok("ext4\nbtrfs\n".into()),
            Err(anyhow::anyhow!("ambiguous probe")),
        ] {
            let mut answer = Some(answer);
            let mut calls = vec![];
            assert!(
                mount_with(
                    "ext4",
                    "/dev/nvme0n1p2",
                    "/target",
                    "defaults",
                    |program, args, _| {
                        calls.push(program.to_owned());
                        assert_eq!(args[0], "--probe");
                        answer.take().unwrap()
                    }
                )
                .is_err()
            );
            assert_eq!(calls, ["blkid"]);
        }
    }
    #[test]
    fn formatting_stops_at_each_failed_step() {
        for fs in Filesystem::ALL {
            for failure in 0..3 {
                let mut calls = 0;
                assert!(
                    format_with(fs, "/dev/nvme0n1p2", |_, _, _| {
                        let current = calls;
                        calls += 1;
                        if current == failure {
                            anyhow::bail!("injected I/O failure");
                        }
                        Ok(String::new())
                    })
                    .is_err()
                );
                assert_eq!(calls, failure + 1);
            }
        }
    }
    #[test]
    fn every_formatter_wipes_then_probes_and_mounts_explicitly() {
        for fs in Filesystem::ALL {
            let mut calls = vec![];
            let mut run = |program: &str, args: &[&str], _: u64| {
                calls.push((
                    program.to_owned(),
                    args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                ));
                Ok(if program == "blkid" {
                    format!("{}\n", fs.name())
                } else {
                    String::new()
                })
            };
            format_with(fs, "/dev/nvme0n1p2", &mut run).unwrap();
            mount_with(fs.name(), "/dev/nvme0n1p2", "/target", "defaults", &mut run).unwrap();
            assert_eq!(
                calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
                ["wipefs", fs.formatter(), "blkid", "blkid", "mount"]
            );
            assert_eq!(calls[0].1, ["--all", "--force", "/dev/nvme0n1p2"]);
            assert_eq!(
                calls[4].1,
                [
                    "--types",
                    fs.name(),
                    "--options",
                    "defaults",
                    "--",
                    "/dev/nvme0n1p2",
                    "/target"
                ]
            );
        }
    }
    #[test]
    fn missing_kernel_support_and_helpers_fail_preflight() {
        assert!(
            preflight_with(
                Filesystem::Xfs,
                Firmware::Uefi,
                |_, _, _| Ok(String::new()),
                || Ok("nodev\ttmpfs\n\text4\n\tvfat\n".into())
            )
            .is_err()
        );
        let mut calls = 0;
        assert!(
            preflight_with(
                Filesystem::Ext4,
                Firmware::Bios,
                |_, _, _| {
                    calls += 1;
                    anyhow::bail!("missing formatter")
                },
                || panic!("must stop before inspecting the kernel")
            )
            .is_err()
        );
        assert_eq!(calls, 1);
    }
    /// Run only inside a disposable Linux VM/container with loop and mount access.
    /// Every write is confined to a loop device backed by our newly created file.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires Linux root, loop devices and filesystem tools"]
    fn real_reformat_mount_matrix() -> Result<()> {
        ensure!(
            unsafe { libc::geteuid() } == 0,
            "Needs disposable Linux root"
        );
        struct Image {
            device: String,
            dir: std::path::PathBuf,
        }
        impl Drop for Image {
            fn drop(&mut self) {
                let mount = self.dir.join("mount");
                if let Some(mount) = mount.to_str() {
                    let _ = output("umount", &[mount], 30);
                }
                // Never recursively delete a mountpoint, including on failure.
                if output("losetup", &["--detach", &self.device], 30).is_ok() {
                    let _ = fs::remove_file(self.dir.join("disk.raw"));
                }
                let _ = fs::remove_dir(mount);
                let _ = fs::remove_dir(&self.dir);
            }
        }
        for sector_size in ["512", "4096"] {
            let dir = tempfile::Builder::new()
                .prefix("calamares-fs-test-")
                .tempdir()?
                .keep();
            let backing = dir.join("disk.raw");
            fs::File::create(&backing)?.set_len(2 * 1024u64.pow(3))?;
            fs::create_dir(dir.join("mount"))?;
            let device = output(
                "losetup",
                &[
                    "--find",
                    "--show",
                    "--sector-size",
                    sector_size,
                    backing.to_str().unwrap(),
                ],
                30,
            )?
            .trim()
            .to_owned();
            let image = Image { device, dir };
            ensure!(
                image.device.starts_with("/dev/loop"),
                "Unexpected test device"
            );
            ensure!(
                output(
                    "losetup",
                    &["--noheadings", "--output", "BACK-FILE", &image.device],
                    30
                )?
                .trim()
                    == backing.to_str().unwrap(),
                "Loop backing file changed"
            );
            let mount = image.dir.join("mount");
            let mount = mount.to_str().unwrap();
            for previous in Filesystem::ALL {
                for next in Filesystem::ALL {
                    previous.format(&image.device)?;
                    let old_uuid = probe_uuid(&image.device, previous.name())?;
                    // Prime the ordinary userspace probe before replacing it.
                    output("blkid", &[&image.device], 30)?;
                    next.format(&image.device)?;
                    let new_uuid = probe_uuid(&image.device, next.name())?;
                    ensure!(new_uuid != old_uuid, "Reformat retained the previous UUID");
                    next.mount(&image.device, mount)?;
                    ensure!(
                        output(
                            "findmnt",
                            &["--noheadings", "--output", "FSTYPE", "--mountpoint", mount],
                            15
                        )?
                        .trim()
                            == next.name(),
                        "Mounted wrong filesystem"
                    );
                    fs::write(
                        std::path::Path::new(mount).join("sentinel"),
                        b"persistent storage",
                    )?;
                    output("sync", &[], 30)?;
                    output("umount", &[mount], 30)?;
                    next.mount(&image.device, mount)?;
                    ensure!(
                        fs::read(std::path::Path::new(mount).join("sentinel"))?
                            == b"persistent storage",
                        "Data lost on remount"
                    );
                    output("umount", &[mount], 30)?;
                    println!(
                        "PASS sector={sector_size} {} -> {}",
                        previous.name(),
                        next.name()
                    );
                }
            }
            format_efi(&image.device)?;
            let old_uuid = probe_uuid(&image.device, "vfat")?;
            output("blkid", &[&image.device], 30)?;
            format_efi(&image.device)?;
            ensure!(
                probe_uuid(&image.device, "vfat")? != old_uuid,
                "FAT reformat retained the previous UUID"
            );
            mount_efi(&image.device, mount)?;
            ensure!(
                output(
                    "findmnt",
                    &["-n", "-o", "FSTYPE", "--mountpoint", mount],
                    15
                )?
                .trim()
                    == "vfat",
                "Wrong EFI filesystem"
            );
            output("umount", &[mount], 30)?;
        }
        Ok(())
    }
}

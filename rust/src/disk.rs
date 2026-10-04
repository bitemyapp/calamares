// SPDX-License-Identifier: GPL-3.0-or-later
use crate::process::output;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
};

#[allow(clippy::unnecessary_cast)] // dev_t is u64 on Linux, i32 on Darwin.
fn device_number(meta: &fs::Metadata) -> libc::dev_t {
    meta.rdev() as libc::dev_t
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub path: String,
    pub major_minor: String,
    pub bytes: u64,
    pub serial: String,
    pub wwn: String,
    pub model: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Disk {
    pub identity: Identity,
    pub blocked: Option<String>,
}

fn busy(node: &Value) -> bool {
    node["mountpoints"]
        .as_array()
        .is_some_and(|a| a.iter().any(|m| m.as_str().is_some_and(|s| !s.is_empty())))
        || node["children"]
            .as_array()
            .is_some_and(|a| a.iter().any(busy))
}

pub fn parse(text: &str) -> Result<Vec<Disk>> {
    let data: Value = serde_json::from_str(text)?;
    let nodes = data["blockdevices"]
        .as_array()
        .context("lsblk did not return block devices")?;
    ensure!(
        !nodes.iter().any(|n| n["type"] == "part"),
        "lsblk returned a flat list, not the required device tree"
    );
    let mut disks = Vec::new();
    for node in nodes {
        if node["type"] != "disk" {
            continue;
        }
        let s = |key: &str| node[key].as_str().unwrap_or("").trim().to_string();
        let bytes = node["size"].as_u64().context("Missing disk size")?;
        let blocked = if node["ro"] != false {
            Some("Read-only device")
        } else if busy(node) {
            Some("Contains a mounted filesystem or active swap (including live media)")
        } else if bytes < 24 * 1024u64.pow(3) {
            Some("Needs at least 24 GiB")
        } else if !s("path").starts_with("/dev/") {
            Some("Invalid device path")
        } else {
            None
        };
        disks.push(Disk {
            identity: Identity {
                path: s("path"),
                major_minor: s("maj:min"),
                bytes,
                serial: s("serial"),
                wwn: s("wwn"),
                model: s("model"),
            },
            blocked: blocked.map(str::to_owned),
        });
    }
    Ok(disks)
}

pub fn discover() -> Result<Vec<Disk>> {
    let text = output(
        "lsblk",
        &[
            "--json",
            "--tree",
            "--bytes",
            "--paths",
            "--output",
            "PATH,TYPE,SIZE,RO,MODEL,SERIAL,WWN,MAJ:MIN,MOUNTPOINTS",
        ],
        30,
    )?;
    let mut disks = parse(&text)?;
    for disk in &mut disks {
        let p = Path::new(&disk.identity.path);
        let meta = fs::metadata(p)?;
        ensure!(
            meta.file_type().is_block_device() && p.canonicalize()? == p,
            "Noncanonical block device"
        );
        let major = libc::major(device_number(&meta));
        let minor = libc::minor(device_number(&meta));
        ensure!(
            disk.identity.major_minor == format!("{major}:{minor}"),
            "Disk changed during discovery"
        );
        let sys = Path::new("/sys/dev/block")
            .join(&disk.identity.major_minor)
            .canonicalize()?;
        if sys.starts_with("/sys/devices/virtual/block") {
            disk.blocked =
                Some("Virtual memory or logical block device is not an installation target".into());
        }
        // Holders (LVM, dm-crypt, RAID) must block the whole drive even when
        // no filesystem on the logical device is currently mounted.
        for node in std::iter::once(sys.clone()).chain(
            fs::read_dir(&sys)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.join("partition").exists()),
        ) {
            if fs::read_dir(node.join("holders"))?.next().is_some() {
                disk.blocked =
                    Some("Device or partition is used by LVM, encryption or RAID".into());
            }
        }
    }
    Ok(disks)
}

pub fn revalidate(identity: &Identity) -> Result<()> {
    let disks = discover()?;
    let current = disks
        .iter()
        .find(|d| d.identity.path == identity.path)
        .context("Selected disk disappeared")?;
    ensure!(
        &current.identity == identity,
        "Disk identity changed; rescan and review again"
    );
    ensure!(
        current.blocked.is_none(),
        "Disk is unavailable: {}",
        current.blocked.as_deref().unwrap_or("")
    );
    Ok(())
}

pub fn partition(path: &str, index: u8) -> String {
    format!(
        "{path}{}{index}",
        if path.ends_with(|c: char| c.is_ascii_digit()) {
            "p"
        } else {
            ""
        }
    )
}

/// Test diagnostics are only available with our marked, disposable QEMU disk.
/// Both transports must satisfy the same machine, serial and size checks.
pub fn vm_test_disk() -> Result<Identity> {
    ensure!(
        fs::read_to_string("/sys/class/dmi/id/product_serial")?.trim() == "RESPIN_VM_ONLY",
        "Not the disposable test VM"
    );
    let mut matches = discover()?.into_iter().filter(|d| {
        ["/dev/vda", "/dev/nvme0n1"].contains(&d.identity.path.as_str())
            && d.identity.serial == "RESPIN_TEST_ONLY"
            && d.identity.bytes == 40 * 1024u64.pow(3)
    });
    let disk = matches.next().context("Missing marked 40 GiB VM disk")?;
    ensure!(matches.next().is_none(), "Multiple VM test disks");
    Ok(disk.identity)
}

/// Require the kernel's actual partition geometry, not merely existing names.
/// sysfs start/size are always in 512-byte sectors, including on 4Kn NVMe.
pub(crate) fn verify_layout(disk: &Identity, firmware: crate::Firmware) -> Result<()> {
    let parent = Path::new("/sys/dev/block")
        .join(&disk.major_minor)
        .canonicalize()?;
    let root_start = if firmware == crate::Firmware::Uefi {
        1025
    } else {
        3
    } * 1024
        * 1024;
    for index in [1, 2] {
        let device = partition(&disk.path, index);
        let meta = fs::metadata(&device)?;
        ensure!(
            meta.file_type().is_block_device()
                && Path::new(&device).canonicalize()? == Path::new(&device),
            "Invalid partition node {device}"
        );
        let node = Path::new("/sys/dev/block")
            .join(format!(
                "{}:{}",
                libc::major(device_number(&meta)),
                libc::minor(device_number(&meta))
            ))
            .canonicalize()?;
        ensure!(
            node.parent() == Some(parent.as_path()),
            "Partition {device} belongs to another disk"
        );
        let number = |name: &str| -> Result<u64> {
            Ok(fs::read_to_string(node.join(name))?.trim().parse()?)
        };
        ensure!(
            number("partition")? == u64::from(index),
            "Wrong partition number for {device}"
        );
        let start = number("start")?
            .checked_mul(512)
            .context("Partition start overflow")?;
        let size = number("size")?
            .checked_mul(512)
            .context("Partition size overflow")?;
        ensure!(
            layout_matches(index, start, size, root_start, disk.bytes),
            "Unexpected partition geometry for {device}: start {start}, size {size}"
        );
    }
    Ok(())
}
fn layout_matches(index: u8, start: u64, size: u64, root_start: u64, disk_bytes: u64) -> bool {
    if index == 1 {
        start == 1024 * 1024 && size == root_start - start
    } else {
        start == root_start
            && size > 0
            && start
                .checked_add(size)
                .is_some_and(|end| end <= disk_bytes && disk_bytes - end <= 2 * 1024 * 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_stale_partition_geometry() {
        let bytes = 40 * 1024u64.pow(3);
        let start = 1025 * 1024 * 1024;
        assert!(layout_matches(1, 1024 * 1024, 1024u64.pow(3), start, bytes));
        assert!(layout_matches(
            2,
            start,
            bytes - start - 512 * 33,
            start,
            bytes
        ));
        assert!(!layout_matches(
            2,
            3 * 1024 * 1024,
            bytes - start,
            start,
            bytes
        ));
        assert!(!layout_matches(2, start, bytes, start, bytes));
        assert!(!layout_matches(2, start, 1024u64.pow(3), start, bytes));
    }
    #[test]
    fn flat_device_list_is_rejected() {
        assert!(parse(r#"{"blockdevices":[{"type":"disk","size":42949672960,"ro":false,"path":"/dev/vda"},{"type":"part","mountpoints":["/"]}]}"#).is_err());
    }
    #[test]
    fn partition_names() {
        assert_eq!(partition("/dev/vda", 2), "/dev/vda2");
        assert_eq!(partition("/dev/nvme0n1", 2), "/dev/nvme0n1p2");
        assert_eq!(partition("/dev/mmcblk0", 2), "/dev/mmcblk0p2");
    }
    #[test]
    fn refuses_live_swap_small_and_readonly() {
        for extra in [
            r#""ro":true"#,
            r#""ro":false,"mountpoints":["[SWAP]"]"#,
            r#""ro":false,"children":[{"mountpoints":["/iso"]}]"#,
        ] {
            let json = format!(
                r#"{{"blockdevices":[{{"type":"disk","path":"/dev/vda","size":42949672960,{extra}}}]}}"#
            );
            assert!(parse(&json).unwrap()[0].blocked.is_some());
        }
        assert!(
            parse(r#"{"blockdevices":[{"type":"disk","path":"/dev/vda","size":1000,"ro":false}]}"#)
                .unwrap()[0]
                .blocked
                .is_some()
        );
    }
}

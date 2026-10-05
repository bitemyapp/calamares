// SPDX-License-Identifier: GPL-3.0-or-later
//! NVIDIA GPUs found in sysfs. Installed systems use NVIDIA's own driver for
//! them instead of nouveau; laptops also get PRIME offload so the integrated
//! GPU drives the panel and the NVIDIA GPU sleeps when unused.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

const NVIDIA: &str = "0x10de";
const INTEL: &str = "0x8086";
const AMD: &str = "0x1002";
/// SMBIOS chassis types: portable, laptop, notebook, sub-notebook,
/// convertible, detachable.
const LAPTOP_CHASSIS: [u32; 6] = [8, 9, 10, 14, 31, 32];

/// The integrated GPU of a laptop with an NVIDIA GPU, as a NixOS bus ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum Offload {
    Intel(String),
    Amd(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graphics {
    /// The first NVIDIA GPU, as a NixOS bus ID such as `PCI:1:0:0`.
    pub nvidia: Option<String>,
    pub offload: Option<Offload>,
}

/// NixOS's `PCI:bus:device:function` form, in decimal, of a sysfs slot name
/// such as `0000:01:00.0`; other PCI domains use `PCI:bus@domain:...`.
fn bus_id(slot: &str) -> Option<String> {
    let (rest, function) = slot.rsplit_once('.')?;
    let mut parts = rest.split(':');
    let (domain, bus, device) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let hex = |s: &str| u32::from_str_radix(s, 16).ok();
    let (domain, bus, device, function) = (hex(domain)?, hex(bus)?, hex(device)?, hex(function)?);
    Some(if domain == 0 {
        format!("PCI:{bus}:{device}:{function}")
    } else {
        format!("PCI:{bus}@{domain}:{device}:{function}")
    })
}

fn valid_bus_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("PCI:") else {
        return false;
    };
    let fields: Vec<&str> = rest.split(':').collect();
    let number = |s: &str| !s.is_empty() && s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit());
    fields.len() == 3
        && fields[1..].iter().all(|f| number(f))
        && match fields[0].split_once('@') {
            Some((bus, domain)) => number(bus) && number(domain),
            None => number(fields[0]),
        }
}

impl Graphics {
    pub fn detect() -> Self {
        Self::scan(
            Path::new("/sys/bus/pci/devices"),
            Path::new("/sys/class/dmi/id/chassis_type"),
        )
    }

    /// Display controllers (PCI class 0x03) under `devices`, in slot order.
    pub fn scan(devices: &Path, chassis: &Path) -> Self {
        let mut found: Vec<(String, String)> = fs::read_dir(devices)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let read = |name: &str| fs::read_to_string(entry.path().join(name)).ok();
                let class = read("class")?;
                let display = class.trim().starts_with("0x03");
                let vendor = read("vendor")?.trim().to_owned();
                let slot = entry.file_name().to_string_lossy().into_owned();
                display.then_some((slot, vendor))
            })
            .collect();
        found.sort();
        let first = |vendor: &str| {
            found
                .iter()
                .find(|(_, v)| v == vendor)
                .and_then(|(slot, _)| bus_id(slot))
        };
        let Some(nvidia) = first(NVIDIA) else {
            return Self::default();
        };
        let laptop = fs::read_to_string(chassis)
            .ok()
            .and_then(|t| t.trim().parse().ok())
            .is_some_and(|t: u32| LAPTOP_CHASSIS.contains(&t));
        let offload = laptop
            .then(|| {
                first(INTEL)
                    .map(Offload::Intel)
                    .or_else(|| first(AMD).map(Offload::Amd))
            })
            .flatten();
        Self {
            nvidia: Some(nvidia),
            offload,
        }
    }

    /// Requests carry the GUI's detection; the helper accepts only bus IDs.
    pub fn check(&self) -> Result<()> {
        let ids = [
            self.nvidia.as_deref(),
            match &self.offload {
                Some(Offload::Intel(id) | Offload::Amd(id)) => Some(id.as_str()),
                None => None,
            },
        ];
        ensure!(
            ids.iter().flatten().all(|id| valid_bus_id(id)),
            "Invalid GPU bus ID"
        );
        ensure!(
            self.nvidia.is_some() || self.offload.is_none(),
            "PRIME offload needs an NVIDIA GPU"
        );
        Ok(())
    }

    /// For the review page.
    pub fn describe(&self, allow_unfree: bool) -> String {
        match (&self.nvidia, &self.offload, allow_unfree) {
            (None, _, _) => "No NVIDIA GPU; the open-source drivers are used".into(),
            (Some(_), _, false) => {
                "NVIDIA GPU found, but its driver needs unfree packages: nouveau is used".into()
            }
            (Some(_), None, true) => "NVIDIA driver (latest release)".into(),
            (Some(_), Some(Offload::Intel(_)), true) => {
                "NVIDIA driver (latest release), offloading from Intel graphics".into()
            }
            (Some(_), Some(Offload::Amd(_)), true) => {
                "NVIDIA driver (latest release), offloading from AMD graphics".into()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(root: &Path, slot: &str, vendor: &str, class: &str) {
        let dir = root.join(slot);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("vendor"), format!("{vendor}\n")).unwrap();
        fs::write(dir.join("class"), format!("{class}\n")).unwrap();
    }

    #[test]
    fn hybrid_laptops_offload_from_the_integrated_gpu() {
        let dir = tempfile::tempdir().unwrap();
        let devices = dir.path().join("devices");
        device(&devices, "0000:00:02.0", INTEL, "0x030000");
        device(&devices, "0000:01:00.0", NVIDIA, "0x030000");
        device(&devices, "0000:01:00.1", NVIDIA, "0x040300"); // HDMI audio
        device(&devices, "0000:02:00.0", "0x144d", "0x010802"); // NVMe
        let chassis = dir.path().join("chassis_type");
        fs::write(&chassis, "10\n").unwrap();
        let laptop = Graphics::scan(&devices, &chassis);
        assert_eq!(laptop.nvidia.as_deref(), Some("PCI:1:0:0"));
        assert_eq!(laptop.offload, Some(Offload::Intel("PCI:0:2:0".into())));
        laptop.check().unwrap();
        // A desktop drives its monitors from the NVIDIA card: no offload.
        fs::write(&chassis, "3\n").unwrap();
        let desktop = Graphics::scan(&devices, &chassis);
        assert_eq!(desktop.nvidia.as_deref(), Some("PCI:1:0:0"));
        assert_eq!(desktop.offload, None);
    }

    #[test]
    fn amd_laptops_and_machines_without_nvidia() {
        let dir = tempfile::tempdir().unwrap();
        let devices = dir.path().join("devices");
        let chassis = dir.path().join("chassis_type");
        fs::write(&chassis, "31").unwrap();
        device(&devices, "0000:c1:00.0", AMD, "0x038000");
        assert_eq!(Graphics::scan(&devices, &chassis), Graphics::default());
        device(&devices, "0000:64:00.0", NVIDIA, "0x030200");
        let found = Graphics::scan(&devices, &chassis);
        assert_eq!(found.nvidia.as_deref(), Some("PCI:100:0:0"));
        assert_eq!(found.offload, Some(Offload::Amd("PCI:193:0:0".into())));
    }

    #[test]
    fn bus_ids_are_decimal_and_validated() {
        assert_eq!(bus_id("0000:0a:1f.7").as_deref(), Some("PCI:10:31:7"));
        assert_eq!(bus_id("0001:01:00.0").as_deref(), Some("PCI:1@1:0:0"));
        assert_eq!(bus_id("garbage"), None);
        for good in ["PCI:1:0:0", "PCI:1@1:0:0", "PCI:193:0:0"] {
            assert!(valid_bus_id(good), "{good}");
        }
        for bad in [
            "",
            "PCI:1:0",
            "PCI:1:0:0:0",
            "PCI:x:0:0",
            "pci:1:0:0",
            "PCI:1:0:0\"; x",
        ] {
            assert!(!valid_bus_id(bad), "{bad}");
        }
        let forged = Graphics {
            nvidia: None,
            offload: Some(Offload::Intel("PCI:0:2:0".into())),
        };
        assert!(forged.check().is_err());
    }
}

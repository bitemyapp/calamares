// SPDX-License-Identifier: GPL-3.0-or-later
//! Detection is advisory, never based on locale/country/UTC offset. Run on a worker.
use anyhow::{Result, ensure};
use std::{fs, io::Read, path::Path};

pub struct Detection {
    pub zone: TimeZone,
    pub explanation: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeZone(String);
impl TimeZone {
    pub fn parse(zone: &str, root: &Path) -> Result<Self> {
        ensure!(
            !zone.is_empty()
                && zone.len() <= 100
                && zone.split('/').all(|p| !p.is_empty()
                    && p != "."
                    && p != ".."
                    && p.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_+-".contains(&b))),
            "Choose a valid IANA time zone, for example America/Chicago"
        );
        let root = root.canonicalize()?;
        let file = root.join(zone).canonicalize()?;
        ensure!(
            file.starts_with(root) && file.is_file(),
            "Unknown time zone"
        );
        let mut magic = [0; 4];
        fs::File::open(file)?.read_exact(&mut magic)?;
        ensure!(&magic == b"TZif", "Not a time-zone data file");
        Ok(Self(zone.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn usable_live_zone(zone: &str, root: &Path) -> Option<TimeZone> {
    let zone = zone.trim();
    // These are the untouched live ISO defaults, not a location guess.
    if matches!(
        zone,
        "" | "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT" | "UCT" | "Etc/UCT"
    ) {
        return None;
    }
    TimeZone::parse(zone, root).ok()
}

pub fn detect(root: &Path, internet: bool) -> Result<Detection> {
    let local = crate::process::output(
        "timedatectl",
        &["show", "--property=Timezone", "--value"],
        5,
    )
    .unwrap_or_default();
    if let Some(zone) = usable_live_zone(&local, root) {
        return Ok(Detection {
            zone,
            explanation: "From the live system. Check this matches your location.".into(),
        });
    }
    ensure!(
        internet,
        "The live system has no regional time zone. Choose one manually or enable internet detection."
    );
    let mut args = vec![
        "--fail",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--tlsv1.2",
        "--connect-timeout",
        "3",
        "--max-time",
        "8",
        "--max-filesize",
        "4096",
    ];
    if let Some(ca) = option_env!("CALAMARES_CA_FILE") {
        args.extend(["--cacert", ca]);
    }
    args.extend(["--url", "https://ipapi.co/timezone/"]);
    let text = crate::process::output("curl", &args, 10)
        .map_err(|_| anyhow::anyhow!("Internet detection unavailable. Choose your time zone manually; no default has been guessed."))?;
    let zone = text.trim();
    let zone = TimeZone::parse(zone, root).map_err(|_| {
        anyhow::anyhow!("The location service returned no valid time zone. Choose one manually.")
    })?;
    Ok(Detection { zone, explanation: "Approximate public-IP location from ipapi.co. VPNs and mobile networks can be wrong — please confirm.".into() })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn central_and_eastern_remain_distinct_and_utc_is_not_a_location() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("America")).unwrap();
        for zone in ["Chicago", "New_York"] {
            fs::write(tmp.path().join("America").join(zone), b"TZif").unwrap();
        }
        assert_eq!(
            usable_live_zone("America/Chicago\n", tmp.path())
                .as_ref()
                .map(TimeZone::as_str),
            Some("America/Chicago")
        );
        assert_eq!(
            usable_live_zone("America/New_York", tmp.path())
                .as_ref()
                .map(TimeZone::as_str),
            Some("America/New_York")
        );
        for zone in ["UTC", "Etc/UTC", "US", "-0500", "../etc/passwd", ""] {
            assert!(usable_live_zone(zone, tmp.path()).is_none());
        }
        fs::write(tmp.path().join("zone.tab"), "not TZif").unwrap();
        assert!(TimeZone::parse("zone.tab", tmp.path()).is_err());
    }

    #[test]
    fn aliases_stay_inside_tzdata_and_non_tzif_files_fail() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(root.path().join("Region"), b"TZif").unwrap();
        fs::write(root.path().join("NotZone"), b"text").unwrap();
        fs::write(outside.path().join("Zone"), b"TZif").unwrap();
        std::os::unix::fs::symlink("Region", root.path().join("Alias")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("Zone"), root.path().join("Escape"))
            .unwrap();
        assert_eq!(
            TimeZone::parse("Alias", root.path()).unwrap().as_str(),
            "Alias"
        );
        assert!(TimeZone::parse("NotZone", root.path()).is_err());
        assert!(TimeZone::parse("Escape", root.path()).is_err());
    }
}

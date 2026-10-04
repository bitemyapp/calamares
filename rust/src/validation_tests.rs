// SPDX-License-Identifier: GPL-3.0-or-later
use crate::*;
fn fixture() -> (tempfile::TempDir, Settings, Request) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("Etc")).unwrap();
    fs::write(dir.path().join("Etc/UTC"), "TZif").unwrap();
    let settings = Settings {
        template_dir: "/unused".into(),
        zoneinfo: dir.path().to_string_lossy().into_owned(),
        state_version: "26.11".into(),
        kernel: Kernel::Lts,
        test_diagnostics: false,
    };
    let request = Request {
        disk: disk::Identity {
            path: "/dev/vda".into(),
            major_minor: "252:0".into(),
            bytes: 40 * 1024u64.pow(3),
            serial: "test".into(),
            wwn: "".into(),
            model: "test".into(),
        },
        firmware: Firmware::Uefi,
        hostname: "my-machine".into(),
        username: "alice".into(),
        full_name: "Alice ${literal}".into(),
        password: "long-enough-test-password".into(),
        locale: "en_US.UTF-8".into(),
        timezone: "Etc/UTC".into(),
        keyboard: "us".into(),
        desktops: vec![Desktop::Plasma],
        default_desktop: Desktop::Plasma,
        copy_wifi: false,
        wifi_profiles: vec![],
        allow_unfree: false,
        confirmation: "ERASE /dev/vda".into(),
    };
    (dir, settings, request)
}
#[test]
fn valid_request() {
    let (_dir, s, r) = fixture();
    validate(&r, &s).unwrap();
}
#[test]
fn desktop_selection_is_explicit_and_validated() {
    let (_dir, s, mut r) = fixture();
    for desktop in Desktop::ALL {
        r.desktops = vec![desktop];
        r.default_desktop = desktop;
        validate(&r, &s).unwrap();
        let text = config::configuration(&r, &s);
        assert!(text.contains(&format!("{}.enable = true;", desktop.option())));
        assert!(text.contains(&format!("defaultSession = \"{}\"", desktop.session())));
        if desktop != Desktop::Plasma {
            assert!(!text.contains("services.desktopManager.plasma6.enable"));
        }
    }
    for desktops in [
        vec![],
        vec![Desktop::Plasma, Desktop::Plasma],
        vec![Desktop::Gnome, Desktop::Cinnamon],
    ] {
        r.desktops = desktops;
        assert!(validate(&r, &s).is_err());
    }
    r.desktops = vec![Desktop::Plasma, Desktop::Xfce];
    r.default_desktop = Desktop::Xfce;
    validate(&r, &s).unwrap();
    r.default_desktop = Desktop::Gnome;
    assert!(validate(&r, &s).is_err());
}
#[test]
fn wifi_opt_out_rejects_injected_profiles() {
    let (_dir, s, mut r) = fixture();
    r.wifi_profiles.push("private data".into());
    assert!(
        validate(&r, &s)
            .unwrap_err()
            .to_string()
            .contains("disabled")
    );
}
#[test]
fn reject_names_secrets_zone_and_confirmation() {
    let (_dir, s, mut r) = fixture();
    for value in ["", "-host", "host-", "a/b", "${builtins.abort \"x\"}"] {
        r.hostname = value.into();
        assert!(validate(&r, &s).is_err());
    }
    r.hostname = "host".into();
    for value in ["root", "nixos", "nixbld23", "User", "a;id", "a/b", ""] {
        r.username = value.into();
        assert!(validate(&r, &s).is_err());
    }
    r.username = "alice".into();
    for value in ["short", "new\nlineeeeeeeee", ""] {
        r.password = value.into();
        assert!(validate(&r, &s).is_err());
    }
    r.password = "long-enough-password".into();
    for value in ["../etc/shadow", "/Etc/UTC", "Etc//UTC", "Etc/missing"] {
        r.timezone = value.into();
        assert!(validate(&r, &s).is_err());
    }
    r.timezone = "Etc/UTC".into();
    r.confirmation = "ERASE /dev/sda".into();
    assert!(validate(&r, &s).is_err());
}
#[test]
fn config_contains_no_password_and_no_test_services() {
    let (_dir, s, r) = fixture();
    let text = config::configuration(&r, &s);
    assert!(!text.contains(&r.password));
    assert!(!text.contains("qemuGuest"));
    assert!(text.contains("hashedPasswordFile = \"/etc/nixos-secrets/"));
    assert!(text.contains("systemd-boot.enable = true"));
    assert!(!text.contains("grub.device"));
    assert!(text.contains("\\${literal}"));
}
#[test]
fn bios_latest_and_test_diagnostics_are_root_settings() {
    let (_dir, mut s, mut r) = fixture();
    r.firmware = Firmware::Bios;
    s.kernel = Kernel::Latest;
    s.test_diagnostics = true;
    let text = config::configuration(&r, &s);
    assert!(text.contains("grub.device = \"/dev/vda\""));
    assert!(text.contains("linuxPackages_latest"));
    assert!(text.contains("qemuGuest.enable = true"));
    let mut value = serde_json::to_value(&r).unwrap();
    value["test_diagnostics"] = true.into();
    assert!(serde_json::from_value::<Request>(value).is_err());
}
#[test]
fn hashing_authenticates_only_correct_password() {
    let params = sha_crypt::Sha512Params::new(1000).unwrap();
    let a = sha_crypt::sha512_simple("secret", &params).unwrap();
    let b = sha_crypt::sha512_simple("secret", &params).unwrap();
    assert_ne!(a, b);
    sha_crypt::sha512_check("secret", &a).unwrap();
    assert!(sha_crypt::sha512_check("wrong", &a).is_err());
}

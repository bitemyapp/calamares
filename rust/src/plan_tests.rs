// SPDX-License-Identifier: GPL-3.0-or-later
use crate::*;
use desktop::DesktopSelection;
fn fixture() -> (tempfile::TempDir, Settings, RawRequest) {
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
    let request = RawRequest {
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
fn review_and_confirmation_are_distinct_transitions() {
    for phrase in ["", "ERASE /dev/sda", "ERASE /dev/vda ", "erase /dev/vda"] {
        let (_dir, settings, mut raw) = fixture();
        raw.confirmation = phrase.into();
        let plan = raw.parse(&settings).unwrap();
        assert_eq!(plan.hostname().as_str(), "my-machine");
        assert!(plan.confirm(phrase).is_err());
        let (_dir, settings, mut raw) = fixture();
        raw.confirmation = phrase.into();
        assert!(raw.parse_confirmed(&settings).is_err());
    }
    let (_dir, settings, raw) = fixture();
    raw.parse(&settings)
        .unwrap()
        .confirm("ERASE /dev/vda")
        .unwrap();
}

#[test]
fn ipc_downgrades_to_raw_and_helper_parses_again() {
    let (_dir, settings, raw) = fixture();
    let plan = raw.parse(&settings).unwrap();
    let expected = config::configuration(&plan);
    let wire = plan.confirm("ERASE /dev/vda").unwrap().into_request();
    let json = zeroize::Zeroizing::new(serde_json::to_vec(&wire).unwrap());
    let received: RawRequest = serde_json::from_slice(&json).unwrap();
    let confirmed = received.parse_confirmed(&settings).unwrap();
    assert_eq!(config::configuration(&confirmed.into_plan()), expected);
    // A well-formed wire object is not trusted just because a GUI produced it.
    let mut received: RawRequest = serde_json::from_slice(&json).unwrap();
    received.disk.path = "/dev/sda".into();
    assert!(received.parse_confirmed(&settings).is_err());
    let mut received: RawRequest = serde_json::from_slice(&json).unwrap();
    received.username = "root".into();
    assert!(received.parse_confirmed(&settings).is_err());
}

#[test]
fn every_desktop_subset_and_default_obeys_the_contract() {
    let mut supported = 0;
    for bits in 0u8..64 {
        let selected: Vec<_> = Desktop::ALL
            .iter()
            .enumerate()
            .filter_map(|(i, d)| (bits & (1 << i) != 0).then_some(*d))
            .collect();
        let compatible = !selected.is_empty()
            && !(selected.contains(&Desktop::Gnome) && selected.contains(&Desktop::Cinnamon));
        supported += usize::from(compatible);
        for default in Desktop::ALL {
            let parsed = DesktopSelection::parse(selected.clone(), default);
            assert_eq!(parsed.is_ok(), compatible && selected.contains(&default));
            if let Ok(parsed) = parsed {
                assert_eq!(parsed.selected(), selected);
                assert_eq!(parsed.default(), default);
            }
        }
    }
    assert_eq!(supported, 47);
    assert!(DesktopSelection::parse(vec![Desktop::Plasma; 2], Desktop::Plasma).is_err());
    assert!(DesktopSelection::parse(vec![Desktop::Plasma; 7], Desktop::Plasma).is_err());
}

#[test]
fn single_desktop_configs_keep_selected_session() {
    for desktop in Desktop::ALL {
        let (_dir, settings, mut raw) = fixture();
        raw.desktops = vec![desktop];
        raw.default_desktop = desktop;
        let text = config::configuration(&raw.parse(&settings).unwrap());
        assert!(text.contains(&format!("{}.enable = true;", desktop.option())));
        assert!(text.contains(&format!("defaultSession = \"{}\"", desktop.session())));
        if desktop != Desktop::Plasma {
            assert!(!text.contains("services.desktopManager.plasma6.enable"));
        }
        assert!(text.contains("networking.networkmanager.enable = true;"));
    }
}

#[test]
fn names_have_checked_constructors() {
    for value in [
        "",
        "-host",
        "host-",
        "a/b",
        "${builtins.abort \"x\"}",
        &"x".repeat(64),
    ] {
        assert!(Hostname::parse(value).is_err());
    }
    for value in ["a", "Abc-123", &"x".repeat(63)] {
        assert!(Hostname::parse(value).is_ok());
    }
    for value in [
        "root",
        "nixos",
        "nixbld23",
        "User",
        "a;id",
        "a/b",
        "",
        &"a".repeat(32),
    ] {
        assert!(Username::parse(value).is_err());
    }
    for value in ["a", "alice-2_test", &"a".repeat(31)] {
        assert!(Username::parse(value).is_ok());
    }
}

#[test]
fn aggregate_parser_checks_ordinary_fields_without_needless_newtypes() {
    let invalid: &[(&str, &str)] = &[
        ("password", "short"),
        ("password", "new\nlineeeeeeeee"),
        ("password", ""),
        ("timezone", "../etc/shadow"),
        ("timezone", "/Etc/UTC"),
        ("timezone", "Etc//UTC"),
        ("timezone", "Etc/missing"),
        ("locale", "unsupported"),
        ("keyboard", "unsupported"),
        ("full_name", "Alice:extra"),
        ("full_name", "Alice\nextra"),
    ];
    for &(field, value) in invalid {
        let (_dir, settings, mut raw) = fixture();
        match field {
            "password" => raw.password = value.into(),
            "timezone" => raw.timezone = value.into(),
            "locale" => raw.locale = value.into(),
            "keyboard" => raw.keyboard = value.into(),
            "full_name" => raw.full_name = value.into(),
            _ => unreachable!(),
        }
        assert!(raw.parse(&settings).is_err(), "accepted invalid {field}");
    }
    for (password, valid) in [
        ("x".repeat(11), false),
        ("é".repeat(12), true),
        ("é".repeat(512), true),
        ("é".repeat(513), false),
    ] {
        let (_dir, settings, mut raw) = fixture();
        raw.password = password;
        assert_eq!(raw.parse(&settings).is_ok(), valid);
    }
    for (name, valid) in [
        ("", true),
        (&"x".repeat(128), true),
        (&"x".repeat(129), false),
    ] {
        let (_dir, settings, mut raw) = fixture();
        raw.full_name = name.into();
        assert_eq!(raw.parse(&settings).is_ok(), valid);
    }
}

#[test]
fn wifi_opt_out_and_empty_snapshot_are_different_states() {
    let user = Username::parse("alice").unwrap();
    let skip = wifi::WifiTransfer::parse(false, vec![], &user).unwrap();
    let copy = wifi::WifiTransfer::parse(true, vec![], &user).unwrap();
    assert!(!skip.enabled());
    assert!(copy.enabled());
    assert_eq!(skip.profile_count(), 0);
    assert_eq!(copy.profile_count(), 0);
    for (enabled, profiles) in [
        (false, vec!["private data".into()]),
        (true, vec![String::new(); 33]),
        (true, vec!["x".repeat(16385)]),
    ] {
        assert!(wifi::WifiTransfer::parse(enabled, profiles, &user).is_err());
    }
    let root = tempfile::tempdir().unwrap();
    skip.write_to(root.path()).unwrap();
    copy.write_to(root.path()).unwrap();
    assert!(!root.path().join("etc").exists());
}

#[test]
fn config_contains_no_password_and_no_test_services() {
    let (_dir, settings, raw) = fixture();
    let secret = raw.password.clone();
    let text = config::configuration(&raw.parse(&settings).unwrap());
    assert!(!text.contains(&secret));
    assert!(!text.contains("qemuGuest"));
    assert!(text.contains("hashedPasswordFile = \"/etc/nixos-secrets/"));
    assert!(text.contains("systemd-boot.enable = true"));
    assert!(!text.contains("grub.device"));
    assert!(text.contains("\\${literal}"));
}

#[test]
fn plan_keeps_settings_used_at_parse_and_wire_cannot_inject_them() {
    let (_dir, mut settings, mut raw) = fixture();
    raw.firmware = Firmware::Bios;
    settings.kernel = Kernel::Latest;
    settings.test_diagnostics = true;
    let mut value = serde_json::to_value(&raw).unwrap();
    value["test_diagnostics"] = true.into();
    assert!(serde_json::from_value::<RawRequest>(value).is_err());
    let plan = raw.parse(&settings).unwrap();
    settings.kernel = Kernel::Lts;
    settings.test_diagnostics = false;
    let text = config::configuration(&plan);
    assert!(text.contains("grub.device = \"/dev/vda\""));
    assert!(text.contains("linuxPackages_latest"));
    assert!(text.contains("qemuGuest.enable = true"));
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

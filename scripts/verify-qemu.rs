#!/usr/bin/env -S rust-script --force
//! Real package installation in a disposable VM; all host disks are regular files.
//! Usage: verify-qemu.rs BASELINE_ISO bios|uefi
//! ```cargo
//! [dependencies]
//! anyhow = "=1.0.100"
//! serde_json = "=1.0.145"
//! respin-tools = { git = "https://github.com/bitemyapp/determinate-nixos-graphical.git", rev = "a207aa578c1fd27a9574de8134740053d93f8941" }
//! ```
use anyhow::{Result, ensure};
use respin_tools::{
    support::*,
    vm::{Rpc, Vm},
};
use serde_json::json;
use std::{
    fs,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
const SHORT: Duration = Duration::from_secs(120);
const FIXTURE: &str =
    "/workspace/rust/target/x86_64-unknown-linux-musl/release/calamares-vm-fixture";
fn poweroff(vm: &mut Vm, work: &Path) -> Result<()> {
    vm.execute(
        "systemd-run --on-active=2 /run/current-system/sw/bin/poweroff",
        SHORT,
    )?;
    let end = Instant::now() + Duration::from_secs(60);
    loop {
        if UnixStream::connect(work.join("qmp.sock")).is_err() {
            break;
        }
        ensure!(Instant::now() < end, "VM did not power off");
        pause(Duration::from_secs(1))?;
    }
    vm.stop();
    Ok(())
}
fn share(vm: &mut Vm) -> Result<()> {
    vm.execute(
        "mkdir -p /workspace; mount -t 9p -o trans=virtio,version=9p2000.L project /workspace",
        SHORT,
    )?;
    Ok(())
}
fn main() -> Result<()> {
    init_signals()?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 2 || (args.len() == 3 && args[2] == "--dev-backend"),
        "Usage: verify-qemu.rs ISO bios|uefi [--dev-backend]"
    );
    let development = args.len() == 3;
    let iso = regular(Path::new(&args[0]))?;
    let firmware = &args[1];
    ensure!(
        ["bios", "uefi"].contains(&firmware.as_str()),
        "Unknown firmware"
    );
    let repo = PathBuf::from(std::env::var("RUST_SCRIPT_PATH")?)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .canonicalize()?;
    run(Command::new("cargo")
        .args([
            "build",
            "--locked",
            "--release",
            "--target",
            "x86_64-unknown-linux-musl",
            "--manifest-path",
        ])
        .arg(repo.join("tests/vm-fixture/Cargo.toml"))
        .arg("--target-dir")
        .arg(repo.join("rust/target")))?;
    if development {
        run(Command::new("cargo")
            .args([
                "build",
                "--locked",
                "--release",
                "--target",
                "x86_64-unknown-linux-musl",
                "--manifest-path",
            ])
            .arg(repo.join("rust/Cargo.toml"))
            .args(["--no-default-features", "--bin", "calamares-nixos-helper"]))?;
    }
    let name = format!("{firmware}-{}", stamp());
    let work = repo.join(".work").join(&name);
    let artifacts = repo.join("artifacts").join(&name);
    fs::create_dir_all(&work)?;
    fs::create_dir_all(&artifacts)?;
    let disk = work.join("target.raw");
    sparse(&disk, 40 * 1024u64.pow(3))?;
    let package = fs::read_to_string(repo.join("artifacts/package-path"))?;
    ensure!(
        package.trim().starts_with("/nix/store/") && !package.trim().contains(['\n', '\r', '\'']),
        "Unexpected package path"
    );
    let mut result = json!({"firmware":firmware,"iso_sha256":sha256(&iso)?,"package":package.trim(),"passed":false,"run":name});
    result["backend"] = json!(if development {
        "development-static (not release verification)"
    } else {
        "nix-package"
    });
    println!("Running {name}; artifacts: {}", artifacts.display());
    let mut vm = Vm::start(&work, firmware, Some(&iso), Some(&disk), Some(&repo))?;
    let outcome = (|| -> Result<()> {
        vm.wait_agent()?;
        share(&mut vm)?;
        vm.execute("for i in $(seq 1 90); do test -S /run/user/1000/wayland-0 && systemctl is-active --quiet display-manager && exit 0; sleep 1; done; exit 1",SHORT)?;
        let command = format!(
            "nix copy --from file:///workspace/.work/store --no-check-sigs {} > /workspace/artifacts/{name}/import.log 2>&1",
            shell_quote(package.trim())
        );
        vm.execute(&command, Duration::from_secs(600))?;
        // Root-owned test settings copied from the verified media; guard runs
        // before creating any fixture files. Public test credentials stay in VM.
        vm.execute(&format!("set -e; {FIXTURE} prepare {} > /run/calamares-test-request.json; chmod 600 /run/calamares-test-request.json",shell_quote(package.trim())),SHORT)?;
        let discovery = vm.execute(
            &format!("{}/bin/calamares-nixos-helper discover", package.trim()),
            SHORT,
        )?;
        fs::write(artifacts.join("disks.json"), &discovery)?;
        let disks: serde_json::Value = serde_json::from_str(&discovery)?;
        ensure!(
            disks
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["identity"]["path"] == "/dev/vda" && d["blocked"].is_null()),
            "Test target unavailable"
        );
        ensure!(
            disks
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d["identity"]["path"] != "/dev/vda")
                .all(|d| !d["blocked"].is_null()),
            "Live media appeared eligible for erase"
        );
        // Start the actual packaged Rust GUI in the existing live desktop.
        vm.execute(&format!("systemctl is-active display-manager; pkill -x calamares || true; systemd-run --uid=1000 --setenv=DISPLAY=:0 --setenv=XDG_RUNTIME_DIR=/run/user/1000 --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus --setenv=WAYLAND_DISPLAY=wayland-0 --setenv=GSK_RENDERER=cairo --unit=calamares-rust-test {}/bin/calamares-nixos",package.trim()),SHORT)?;
        pause(Duration::from_secs(15))?;
        vm.execute(
            "pkill -f '^/nix/store/[^ ]*/bin/[.]?calamares(-wrapped)?( |$)' || true",
            SHORT,
        )?;
        pause(Duration::from_secs(2))?;
        let gui = vm.execute(
            "systemctl is-active calamares-rust-test; journalctl -u calamares-rust-test --no-pager",
            SHORT,
        )?;
        fs::write(artifacts.join("gui.log"), gui)?;
        // The baseline ISO also autostarts the old Qt installer. Raise the
        // native window by its title bar before capturing this fixed-size VM.
        let mut qmp = Rpc::connect(&work.join("qmp.sock"), true, 20)?;
        qmp.call("input-send-event",json!({"events":[{"type":"abs","data":{"axis":"x","value":15360}},{"type":"abs","data":{"axis":"y","value":600}},{"type":"btn","data":{"down":true,"button":"left"}}]}))?;
        qmp.call(
            "input-send-event",
            json!({"events":[{"type":"btn","data":{"down":false,"button":"left"}}]}),
        )?;
        drop(qmp);
        pause(Duration::from_millis(500))?;
        vm.screenshot(&artifacts.join("live-rust-installer.png"))?;
        println!("Native GUI running; installing onto disposable {firmware} disk");
        vm.execute(
            &format!(
                "{}{FIXTURE} install > /workspace/artifacts/{name}/install.log 2>&1",
                if development {
                    "CALAMARES_VM_DEV_BACKEND=1 "
                } else {
                    ""
                }
            ),
            Duration::from_secs(7500),
        )?;
        poweroff(&mut vm, &work)?;
        vm = Vm::start(&work, firmware, None, Some(&disk), Some(&repo))?;
        vm.wait_agent()?;
        share(&mut vm)?;
        vm.execute("for i in $(seq 1 90); do systemctl is-active --quiet display-manager && exit 0; sleep 1; done; exit 1",SHORT)?;
        let verified = vm.execute(&format!("{FIXTURE} verify"), Duration::from_secs(180))?;
        fs::write(artifacts.join("installed-verification.log"), &verified)?;
        result["verification"] = json!(verified);
        pause(Duration::from_secs(10))?;
        vm.screenshot(&artifacts.join("installed-login.png"))?;
        poweroff(&mut vm, &work)?;
        Ok(())
    })();
    result["passed"] = json!(outcome.is_ok());
    if let Err(e) = &outcome {
        result["error"] = json!(format!("{e:#}"));
        let _ = vm.screenshot(&artifacts.join("failure.png"));
        if let Ok(log) = vm.execute("journalctl -b -p warning --no-pager | tail -100", SHORT) {
            fs::write(artifacts.join("failure.log"), log)?;
        }
    }
    vm.stop();
    for name in ["serial.log", "qemu.log"] {
        optional_copy(&work.join(name), &artifacts.join(name))?;
    }
    write_json(&artifacts.join("result.json"), &result)?;
    outcome?;
    println!("PASS: {}", artifacts.display());
    Ok(())
}

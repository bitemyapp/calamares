#!/usr/bin/env -S rust-script --force
//! Reuse an existing rootless respin builder disk, never a host block device.
//! Usage: build-rootless.rs RESPIN_REPO BOOTSTRAP_ISO
//! ```cargo
//! [dependencies]
//! anyhow = "=1.0.100"
//! respin-tools = { git = "https://github.com/bitemyapp/determinate-nixos-graphical.git", rev = "a207aa578c1fd27a9574de8134740053d93f8941" }
//! ```
use anyhow::{Context, Result, ensure};
use respin_tools::support::*;
use std::{
    fs::{self, File},
    io::{Read, Write},
    net::TcpListener,
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    init_signals()?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 2,
        "Usage: build-rootless.rs RESPIN_REPO BOOTSTRAP_ISO"
    );
    let respin = PathBuf::from(&args[0]).canonicalize()?;
    let iso = regular(&PathBuf::from(&args[1]))?;
    ensure!(
        sha256(&iso)? == "80588c226d84e16fe11b2e4afa9fc4add02902e7041dcb220960df5a6cde5fb5",
        "Bootstrap checksum mismatch"
    );
    let repo = PathBuf::from(std::env::var("RUST_SCRIPT_PATH")?)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .canonicalize()?;
    let base = respin.join(".work/rootless");
    let disk = regular(&base.join("builder.raw"))?;
    ensure!(
        fs::metadata(&disk)?.len() == 100 * 1024u64.pow(3),
        "Wrong builder disk size"
    );
    // QEMU enforces its own exclusive image lock. Do not delete another
    // builder's lock file or recreate/format its existing disk here.
    let work = repo.join(".work").join(format!("builder-{}", stamp()));
    fs::create_dir_all(&work)?;
    fs::create_dir_all(repo.join("artifacts"))?;
    let cfg = output(
        Command::new("bsdtar")
            .arg("-xOf")
            .arg(&iso)
            .arg("isolinux/isolinux.cfg"),
    )?;
    let entry = cfg
        .split_once("LABEL boot\n")
        .context("Missing boot entry")?
        .1
        .split("\nLABEL ")
        .next()
        .unwrap();
    let value = |key: &str| {
        entry
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .context("Missing boot field")
    };
    let kernel = base.join(value("LINUX ")?.trim_start_matches('/').replace("//", "/"));
    let initrd = base.join(value("INITRD ")?.trim_start_matches('/').replace("//", "/"));
    regular(&kernel)?;
    regular(&initrd)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let serial_path = work.join("serial.sock");
    let log = File::create(work.join("qemu.log"))?;
    let mut qemu = Process(
        Command::new("qemu-system-x86_64")
            .args([
                "-name",
                "calamares-rust-builder",
                "-machine",
                "q35,accel=kvm",
                "-cpu",
                "host",
                "-smp",
                "6",
                "-m",
                "12288",
                "-display",
                "none",
                "-monitor",
                "none",
                "-no-reboot",
                "-serial",
            ])
            .arg(format!("unix:{},server=on,wait=off", qpath(&serial_path)?))
            .arg("-kernel")
            .arg(kernel)
            .arg("-initrd")
            .arg(initrd)
            .arg("-append")
            .arg(format!("{} console=ttyS0,115200n8", value("APPEND ")?))
            .arg("-drive")
            .arg(format!(
                "file={},format=raw,media=cdrom,readonly=on",
                qpath(&iso)?
            ))
            .arg("-drive")
            .arg(format!(
                "file={},format=raw,if=none,id=builder",
                qpath(&disk)?
            ))
            .args([
                "-device",
                "virtio-blk-pci,drive=builder,serial=RESPIN_BUILDER_ONLY",
                "-netdev",
            ])
            .arg(format!("user,id=net0,hostfwd=tcp:127.0.0.1:{port}-:22"))
            .args(["-device", "virtio-net-pci,netdev=net0", "-virtfs"])
            .arg(format!(
                "local,path={},mount_tag=project,security_model=none,id=project",
                qpath(&respin)?
            ))
            .arg("-virtfs")
            .arg(format!(
                "local,path={},mount_tag=fork,security_model=none,id=fork",
                qpath(&repo)?
            ))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(600);
    while !serial_path.exists() {
        ensure!(
            qemu.0.try_wait()?.is_none() && Instant::now() < deadline,
            "QEMU startup failed"
        );
        pause(Duration::from_millis(200))?;
    }
    let mut serial = UnixStream::connect(&serial_path)?;
    serial.set_read_timeout(Some(Duration::from_secs(1)))?;
    let mut text = String::new();
    let mut sent = false;
    let mut log = File::create(work.join("serial.log"))?;
    loop {
        check_interrupt()?;
        ensure!(Instant::now() < deadline, "Builder boot timed out");
        let mut b = [0; 8192];
        let n = match serial.read(&mut b) {
            Ok(n) => n,
            Err(e)
                if [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut]
                    .contains(&e.kind()) =>
            {
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        ensure!(n > 0, "Serial disconnected");
        log.write_all(&b[..n])?;
        text.push_str(&String::from_utf8_lossy(&b[..n]));
        if !sent && text.contains("nixos@nixos:") {
            writeln!(
                serial,
                "sudo mkdir -p /workspace /fork && sudo mount -t 9p -o trans=virtio,version=9p2000.L project /workspace && sudo mount -t 9p -o trans=virtio,version=9p2000.L fork /fork && sudo /workspace/.work/guest-target/x86_64-unknown-linux-musl/release/respin-tools prepare-builder"
            )?;
            sent = true;
            text.clear();
        }
        if sent && text.contains("RESPIN_BUILDER_READY") {
            break;
        }
    }
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut serial, &mut log);
    });
    let ssh = |remote: &str| {
        let mut c = Command::new("ssh");
        c.arg("-i")
            .arg(base.join("ssh-key"))
            .arg("-p")
            .arg(port.to_string())
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
            ])
            .arg(format!(
                "UserKnownHostsFile={}",
                base.join("known-hosts").display()
            ))
            .arg("root@127.0.0.1")
            .arg(remote);
        c
    };
    println!(
        "Building Rust installer; log: {}",
        repo.join("artifacts/build.log").display()
    );
    let log = File::create(repo.join("artifacts/build.log"))?;
    run(ssh("set -e; export TMPDIR=/build/tmp; git config --global --add safe.directory /fork; nix build /fork#default --no-write-lock-file --out-link /build/calamares-rust -L --cores 6 --max-jobs 2; nix copy --to 'file:///fork/.work/store?compression=zstd' /build/calamares-rust; readlink -f /build/calamares-rust").stdout(log.try_clone()?).stderr(log))?;
    let package = output(&mut ssh("readlink -f /build/calamares-rust"))?;
    fs::write(repo.join("artifacts/package-path"), package)?;
    let _ = run(&mut ssh("poweroff"));
    ensure!(
        qemu.wait(Duration::from_secs(60))?.success(),
        "Builder shutdown failed"
    );
    Ok(())
}

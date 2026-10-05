# Installer handoff

The current task spans this repository and
[bitemyapp/determinate-nixos-graphical](https://github.com/bitemyapp/determinate-nixos-graphical).
Both use `codex/filesystem-install-reliability`.

Read the complete [installer and SanDisk handoff](https://github.com/bitemyapp/determinate-nixos-graphical/blob/codex/filesystem-install-reliability/docs/agent-handoff.md)
before continuing. Locally it is
`/Users/callen/work/determinate-nixos-graphical/docs/agent-handoff.md`.
It includes source/image identities, implementation entry points, passed and
incomplete tests, retained artifacts, build environment and next work.

The SanDisk was successfully written, fully compared against the rebuilt ISO,
and ejected. The image contains Calamares commit
`8547c691978b76c39a6fcbdcaf67d782331d1adb`: filesystem reliability fixes, ext4/Btrfs/XFS
choices, 31 optional applications, Rustup selecting build tools, and rootless
Docker Engine + Compose. This handoff changes documentation only; do not repin
or rebuild merely because the branch HEAD is now newer than the image pin.

The user requested immediate hardware testing and stopped the slow emulated
install matrix. Those ten installs reached NixOS build/install but did not finish.
Full installed application runtime/GUI tests and physical ThinkPad installation
remain unverified. The new target is **under one minute from clicking Install
after supplying all inputs to completion on real hardware**; it has not been
measured or achieved by this work. Obtain the user's hardware result first.

Draft pull requests: [Calamares](https://github.com/bitemyapp/calamares/pull/1)
and [graphical ISO](https://github.com/bitemyapp/determinate-nixos-graphical/pull/1).
Neither has been merged. All owned emulator tests and build containers are
stopped. Do not restart the old ten-way matrix or rewrite the completed USB
without a reason arising from the next task.

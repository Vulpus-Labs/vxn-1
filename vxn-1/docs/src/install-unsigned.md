# Unsigned binaries

Pre-release VXN1 builds are not yet code-signed or notarised. The OS will block them on first launch.

## macOS

After copying `VXN1.clap` into the install location, clear the Gatekeeper quarantine attribute:

```sh
xattr -dr com.apple.quarantine ~/Library/Audio/Plug-Ins/CLAP/VXN1.clap
```

Restart the DAW after running these commands. If the plugin still doesn't load, check the host's plugin scan log — some hosts (Logic, Ableton Live) cache scan failures and need an explicit rescan after the quarantine flag is cleared.

## Windows

SmartScreen may warn on first DAW launch after install. Click "More info" → "Run anyway". This is a per-host-binary prompt, not per-plugin, so it shouldn't recur.

## Linux

No signing required. If the plugin doesn't load, check `dmesg` for SELinux denials and the DAW's log for symbol-resolution errors against `libstdc++` / `libc` / `libGL`.

## Building your own

Building VXN1 yourself from source (see [Installing VXN1](install.md)) sidesteps the unsigned-binary issue entirely — the local build is trusted by the OS as long as you trust the toolchain that produced it.

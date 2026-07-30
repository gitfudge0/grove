# GPUI rewrite — spike findings

Resolved dependency pins (lockstep rule: gpui-component's Cargo.lock pin for
`gpui` is authoritative for ZED_REV; zed's own workspace pin for
`alacritty_terminal` at that rev is authoritative for the terminal backend).

- GPUI_COMPONENT_REV: `88f102d13654fe25aa2fede076274b6b751a3704` (longbridge/gpui-component, HEAD at resolution time)
- ZED_REV: `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba` (zed-industries/zed — taken from gpui-component's Cargo.lock; gpui-component's Cargo.toml itself has no rev/branch pin, it floats on zed's default branch, so the lockfile is the only authoritative source)
- alacritty_terminal: `git = "https://github.com/zed-industries/alacritty", rev = "4c129667ce56611becdc82de6e28218c80e2e88f"` (from zed's root Cargo.toml workspace.dependencies at ZED_REV)
- portable-pty: `0.9`

Note: gpui's `[features]` default set at ZED_REV is
`["font-kit", "wayland", "x11", "windows-manifest"]`, so no extra Cargo
features were needed for spikes/term to compile the Linux platform backends.

## S1 Terminal element

## S2 Text inputs

## S3 Zoom

## S4 Linux platform

## Build status

Toolchain: zed at ZED_REV requires rustc 1.95.0 (`std::hint::cold_path`,
stabilized in 1.95); Arch's packaged rustc is 1.94.1, which fails with
E0658. Resolved by installing rustup **user-locally** (`~/.cargo`, no system
package change) and pinning `spikes/rust-toolchain.toml` to `1.95.0`.
`cd spikes && cargo build` → `Finished` — all four spike binaries build,
including gpui + alacritty_terminal for spike-term. gpui's default features
already include `font-kit`/`wayland`/`x11`; nothing extra needed.

## Go/No-go

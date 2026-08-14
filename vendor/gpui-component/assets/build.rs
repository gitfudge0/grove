use std::{env, path::Path};

fn main() {
    // Publishes assets/icons's absolute path via `cargo:icons-dir=...` -> DEP_GPUI_COMPONENT_DEFAULT_ICONS_ICONS_DIR, so build-time consumers (IconName's proc-macro) find it without a sibling-crate reference; see the `links` field in Cargo.toml.
    let manifest_dir =
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set by cargo");
    let icons_dir = Path::new(&manifest_dir).join("assets/icons");

    // Bail loudly rather than publish a bad path that surfaces as a confusing error later.
    if !icons_dir.is_dir() {
        panic!(
            "expected default icons at {}, but the directory is missing",
            icons_dir.display(),
        );
    }

    println!("cargo:icons-dir={}", icons_dir.display());
    println!("cargo:rerun-if-changed=assets/icons");
    println!("cargo:rerun-if-changed=build.rs");
}

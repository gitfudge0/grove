use std::env;

fn main() {
    // Re-publishes the assets crate's icons dir (via cargo's `links` mechanism) as a rustc-env for `icon_named!`, avoiding a sibling-crate reference that would break `cargo vendor`/`publish`.
    let icons_dir = env::var("DEP_GPUI_COMPONENT_DEFAULT_ICONS_ICONS_DIR").expect(
        "DEP_GPUI_COMPONENT_DEFAULT_ICONS_ICONS_DIR is set by gpui-component-assets's \
         build.rs via its `links` field; make sure the regular dependency on \
         gpui-component-assets is intact in Cargo.toml",
    );

    println!("cargo:rustc-env=GPUI_COMPONENT_DEFAULT_ICONS_DIR={icons_dir}");

    // Cargo invalidates each build script independently, so this stays in lockstep with the assets crate's own rerun-if-changed.
    println!("cargo:rerun-if-changed={icons_dir}");
    println!("cargo:rerun-if-changed=build.rs");
}

fn main() {
    // Reference tooling compiles each validated generated document independently.
    // No generated document is interpreted or loaded by the production app.
    println!("cargo:rustc-check-cfg=cfg(grove_reference_capture)");
    println!("cargo:rerun-if-env-changed=GROVE_REFERENCE_GENERATED");
    if let Some(path) = std::env::var_os("GROVE_REFERENCE_GENERATED") {
        let path = std::path::PathBuf::from(path);
        assert!(
            path.is_absolute() && path.is_file(),
            "reference generated source must be an existing absolute file path: {}",
            path.display()
        );
        println!("cargo:rerun-if-changed={}", path.display());
        println!(
            "cargo:rustc-env=GROVE_REFERENCE_GENERATED={}",
            path.display()
        );
        println!("cargo:rustc-cfg=grove_reference_capture");
    }

    println!("cargo:rerun-if-changed=src/platform/inline_html.m");
    println!("cargo:rerun-if-changed=src/platform/inline_html_keyboard.inc");
    println!("cargo:rerun-if-changed=src/platform/inline_html_accessibility.inc");
    println!("cargo:rerun-if-changed=src/platform/inline_html_linux.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let mut build = cc::Build::new();
        for package in ["webkit2gtk-4.1", "gtk+-3.0", "x11", "xext"] {
            let library = pkg_config::Config::new()
                .cargo_metadata(false)
                .probe(package)
                .unwrap_or_else(|error| {
                    panic!("Inline HTML requires {package} development files: {error}")
                });
            for include in library.include_paths {
                build.include(include);
            }
        }
        build
            .file("src/platform/inline_html_linux.c")
            .compile("grove_inline_html_linux");
        // Native dependencies follow the archive that references their symbols.
        for package in ["webkit2gtk-4.1", "gtk+-3.0", "x11", "xext"] {
            pkg_config::Config::new()
                .probe(package)
                .unwrap_or_else(|error| {
                    panic!("Inline HTML requires {package} development files: {error}")
                });
        }
        println!("cargo:rustc-link-lib=m");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/platform/inline_html.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("grove_inline_html");
        println!("cargo:rustc-link-lib=framework=WebKit");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=QuartzCore");
    }
}

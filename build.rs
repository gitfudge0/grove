fn main() {
    println!("cargo:rerun-if-changed=src/platform/inline_html.m");
    println!("cargo:rerun-if-changed=src/platform/inline_html_keyboard.inc");
    println!("cargo:rerun-if-changed=src/platform/inline_html_accessibility.inc");
    println!("cargo:rerun-if-changed=src/platform/inline_html_linux.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let mut build = cc::Build::new();
        for package in ["webkit2gtk-4.1", "gtk+-3.0", "x11"] {
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
        for package in ["webkit2gtk-4.1", "gtk+-3.0", "x11"] {
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

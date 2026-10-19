fn main() {
    println!("cargo:rerun-if-changed=src/platform/inline_html.m");
    println!("cargo:rerun-if-changed=src/platform/inline_html_keyboard.inc");
    println!("cargo:rerun-if-changed=src/platform/inline_html_accessibility.inc");
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

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/macos_audio_capture.m")
            .flag("-fobjc-arc")
            .flag("-mmacosx-version-min=11.0")
            .flag("-Wno-deprecated-declarations")
            .compile("voice_elf_macos_audio_capture");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,ScreenCaptureKit");
        println!("cargo:rustc-link-lib=framework=CoreMedia");
        println!("cargo:rustc-link-lib=framework=CoreAudio");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rerun-if-changed=src/macos_audio_capture.m");
    }
    // The frontend is embedded by RustEmbed rather than Tauri's frontendDist copier.
    // Explicitly invalidate Cargo after beforeBuildCommand refreshes web/dist.
    println!("cargo:rerun-if-changed=../dist");
    tauri_build::build()
}

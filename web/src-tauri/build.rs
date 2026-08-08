fn main() {
    // The frontend is embedded by RustEmbed rather than Tauri's frontendDist copier.
    // Explicitly invalidate Cargo after beforeBuildCommand refreshes web/dist.
    println!("cargo:rerun-if-changed=../dist");
    tauri_build::build()
}

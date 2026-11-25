fn main() {
    // Tauri embeds the Windows icon into the executable during build. Keep
    // Cargo's build script invalidation tied to every shipped icon layer so a
    // regenerated taskbar/tray/installer icon cannot leave a stale resource
    // inside target/release.
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=icons/32x32.png");
    println!("cargo:rerun-if-changed=icons/128x128.png");
    println!("cargo:rerun-if-changed=icons/128x128@2x.png");
    // Tauri embeds frontendDist in the executable. Track both the entry point
    // and its generated asset directory so a changed Vite bundle cannot leave
    // an older interface silently embedded in the next release build.
    println!("cargo:rerun-if-changed=../dist/index.html");
    println!("cargo:rerun-if-changed=../dist/assets");
    tauri_build::build()
}

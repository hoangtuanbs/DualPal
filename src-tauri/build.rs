fn main() {
    // Embed a changed app icon in the exe; tauri-build doesn't watch it.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}

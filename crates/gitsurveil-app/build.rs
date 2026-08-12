//! Tauri's build script: generates the context (config, icons, capabilities)
//! that `tauri::generate_context!()` expands at compile time.

fn main() {
    tauri_build::build()
}

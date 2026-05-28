// `windows_subsystem = "windows"` keeps a release build from popping a console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running qcast tauri app");
}

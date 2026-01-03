#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;
mod types;

fn main() {
    tauri::Builder::default()
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::open_pdf_file,
            commands::close_document,
            commands::get_document_outline,
            commands::get_page_sizes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

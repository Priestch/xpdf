#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .manage(crate::AppState::new())
        .invoke_handler(tauri::generate_handler![
            crate::commands::open_pdf_file,
            crate::commands::close_document,
            crate::commands::get_document_outline,
            crate::commands::get_page_sizes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

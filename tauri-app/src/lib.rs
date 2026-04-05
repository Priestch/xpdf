mod commands;
mod state;
mod types;

pub use state::AppState;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::open_pdf_file,
            commands::close_document,
            commands::extract_text_from_page,
            commands::get_document_outline,
            commands::get_page_sizes,
            commands::render_page,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

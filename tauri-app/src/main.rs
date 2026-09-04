#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;
mod types;

fn main() {
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
            commands::add_highlight,
            commands::move_page,
            commands::delete_annotation,
            commands::edit_annotation,
            commands::undo_edit,
            commands::redo_edit,
            commands::get_editor_state,
            commands::save_pdf_as,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

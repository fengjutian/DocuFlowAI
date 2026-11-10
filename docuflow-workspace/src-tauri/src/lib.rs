// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

/// Default address of the DocuFlow lab server.
///
/// The desktop client must know where the backend lives; `DOCUFLOW_API_URL` lets a launcher seed it
/// without the user retyping the address. The value is only a default — the client can override it.
#[tauri::command]
fn default_api_base_url() -> Option<String> {
    std::env::var("DOCUFLOW_API_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Default bearer token for the lab server, when it is started with `DOCUFLOW_API_TOKEN`.
#[tauri::command]
fn default_api_token() -> Option<String> {
    std::env::var("DOCUFLOW_API_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![default_api_base_url, default_api_token])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

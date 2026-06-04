mod source_engine;

use source_engine::*;
use std::sync::{Arc, OnceLock};
use tauri::State;

struct AppState {
    engine: Arc<SourceEngine>,
}

static GLOBAL_ENGINE: OnceLock<Arc<SourceEngine>> = OnceLock::new();

fn get_engine() -> &'static Arc<SourceEngine> {
    GLOBAL_ENGINE.get().expect("Engine not initialized")
}

#[tauri::command]
fn get_sources(state: State<AppState>) -> Vec<SourceInfo> {
    state.engine.get_sources()
}

#[tauri::command]
fn get_active_source(state: State<AppState>) -> Option<SourceInfo> {
    state.engine.get_active_source()
}

#[tauri::command]
fn switch_source(state: State<AppState>, id: usize) -> bool {
    state.engine.switch_source(id)
}

#[tauri::command]
fn score_source(state: State<AppState>, source_id: usize, found: bool) {
    state.engine.score_source(source_id, found);
}

#[tauri::command]
async fn search_music(keyword: String) -> Result<SearchResponse, String> {
    let engine = get_engine();
    let kw = keyword.clone();
    Ok(engine.search(&kw).await)
}

#[tauri::command]
async fn get_hot_keywords(limit: Option<usize>) -> Vec<HotItem> {
    let engine = get_engine();
    engine.hot_keywords(limit.unwrap_or(30)).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    println!("Starting Miku Tunes...");

    // Resolve path to 音源 directory
    let cwd = std::env::current_dir().unwrap_or_default();
    let source_dir = if cwd.join("音源").exists() {
        cwd.join("音源")
    } else if cwd.parent().map(|p| p.join("音源")).map_or(false, |p| p.exists()) {
        cwd.parent().unwrap().join("音源")
    } else if cwd.join("../音源").exists() {
        cwd.join("../音源")
    } else {
        cwd
    };

    println!("Source dir: {:?}", source_dir);
    let engine = Arc::new(SourceEngine::new(source_dir));
    let _ = GLOBAL_ENGINE.set(engine.clone());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState { engine })
        .invoke_handler(tauri::generate_handler![
            greet,
            get_sources,
            get_active_source,
            switch_source,
            score_source,
            search_music,
            get_hot_keywords,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

mod source_engine;

use source_engine::*;
use std::sync::{Arc, OnceLock};
use tauri::{Emitter, State};

struct AppState {
    engine: Arc<SourceEngine>,
}

static GLOBAL_ENGINE: OnceLock<Arc<SourceEngine>> = OnceLock::new();

fn get_engine() -> &'static Arc<SourceEngine> {
    GLOBAL_ENGINE.get().expect("Engine not initialized")
}

#[tauri::command]
fn get_sources(state: State<AppState>) -> Vec<SourceInfo> {
    let sources = state.engine.get_sources();
    println!("get_sources called, returning {} sources", sources.len());
    sources
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
fn set_source_enabled(state: State<AppState>, source_id: usize, enabled: bool) -> Vec<SourceInfo> {
    state.engine.set_source_enabled(source_id, enabled)
}

#[tauri::command]
fn move_source(state: State<AppState>, source_id: usize, action: String) -> Vec<SourceInfo> {
    state.engine.move_source(source_id, &action)
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

#[derive(Clone, serde::Serialize)]
struct SearchBatchEvent {
    token: u64,
    results: Vec<SongResult>,
    from_source: Option<String>,
}

#[tauri::command]
async fn search_music_stream(
    app: tauri::AppHandle,
    keyword: String,
    token: u64,
) -> Result<SearchResponse, String> {
    let engine = get_engine();
    let kw = keyword.clone();
    Ok(engine
        .search_with_batches(&kw, |batch| {
            let _ = app.emit(
                "search_music_batch",
                SearchBatchEvent {
                    token,
                    results: batch.results,
                    from_source: batch.from_source,
                },
            );
        })
        .await)
}

#[tauri::command]
async fn search_music_more(keyword: String, offset: usize) -> Result<SearchResponse, String> {
    let engine = get_engine();
    Ok(engine.search_more(&keyword, offset).await)
}

#[tauri::command]
async fn get_hot_keywords(limit: Option<usize>) -> Vec<HotItem> {
    let engine = get_engine();
    println!("get_hot_keywords called, limit={:?}", limit);
    engine.hot_keywords(limit.unwrap_or(30)).await
}

// ─── New API Commands ───

#[tauri::command]
async fn get_rankings(source_id: usize) -> Vec<RankingCategory> {
    let engine = get_engine();
    println!("get_rankings called, source_id={}", source_id);
    engine.get_rankings(source_id).await
}

#[tauri::command]
async fn get_all_rankings(limit: Option<usize>) -> Vec<RankingCategory> {
    let engine = get_engine();
    engine.get_all_rankings(limit.unwrap_or(24)).await
}

#[tauri::command]
async fn get_ranking_songs(source_id: usize, ranking_id: String) -> Vec<SongDetail> {
    let engine = get_engine();
    println!(
        "get_ranking_songs called, source_id={}, ranking_id={}",
        source_id, ranking_id
    );
    engine.get_ranking_songs(source_id, &ranking_id).await
}

#[tauri::command]
async fn get_playlists(source_id: usize) -> Vec<PlaylistInfo> {
    let engine = get_engine();
    println!("get_playlists called, source_id={}", source_id);
    engine.get_playlists(source_id).await
}

#[tauri::command]
async fn get_all_playlists(limit: Option<usize>) -> Vec<PlaylistInfo> {
    let engine = get_engine();
    engine.get_all_playlists(limit.unwrap_or(30)).await
}

#[tauri::command]
async fn get_playlist_songs(source_id: usize, playlist_id: String) -> Vec<SongDetail> {
    let engine = get_engine();
    println!(
        "get_playlist_songs called, source_id={}, playlist_id={}",
        source_id, playlist_id
    );
    engine.get_playlist_songs(source_id, &playlist_id).await
}

#[tauri::command]
async fn get_song_url(
    source_id: usize,
    song_id: String,
    platform: String,
) -> Result<SongUrlResult, String> {
    let engine = get_engine();
    println!(
        "get_song_url called, source_id={}, song_id={}, platform={}",
        source_id, song_id, platform
    );
    engine
        .get_song_url(source_id, &song_id, &platform)
        .await
        .ok_or_else(|| {
            format!(
                "获取播放链接失败：source_id={}, song_id={}, platform={}（所有音源接口均不可用）",
                source_id, song_id, platform
            )
        })
}

#[tauri::command]
async fn get_song_info(
    source_id: usize,
    song_id: String,
    platform: String,
) -> Option<SongInfoResult> {
    let engine = get_engine();
    println!(
        "get_song_info called, source_id={}, song_id={}, platform={}",
        source_id, song_id, platform
    );
    engine.get_song_info(source_id, &song_id, &platform).await
}

#[tauri::command]
async fn parse_kugou_playlist(url: String) -> Result<SharedPlaylist, String> {
    println!("parse_kugou_playlist called, url={}", url);
    parse_kugou_shared_playlist(&url).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    println!("Starting Miku Tunes...");

    // Resolve path to 音源 directory
    let cwd = std::env::current_dir().unwrap_or_default();
    let source_dir = if cwd.join("音源").exists() {
        cwd.join("音源")
    } else if cwd
        .parent()
        .map(|p| p.join("音源"))
        .map_or(false, |p| p.exists())
    {
        cwd.parent().unwrap().join("音源")
    } else if cwd.join("../音源").exists() {
        cwd.join("../音源")
    } else {
        cwd
    };

    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_source_dir = manifest_dir.parent().map(|parent| parent.join("音源"));
    let source_dir = match manifest_source_dir {
        Some(path) if path.exists() => path,
        _ => source_dir,
    };

    println!("Source dir: {:?}", source_dir);
    let engine = Arc::new(SourceEngine::new(source_dir));
    println!(
        "Engine initialized with {} sources",
        engine.get_sources().len()
    );
    let _ = GLOBAL_ENGINE.set(engine.clone());

    println!("Before tauri::Builder::run()...");
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState { engine })
        .invoke_handler(tauri::generate_handler![
            greet,
            get_sources,
            get_active_source,
            switch_source,
            set_source_enabled,
            move_source,
            score_source,
            search_music,
            search_music_stream,
            search_music_more,
            get_hot_keywords,
            get_rankings,
            get_all_rankings,
            get_ranking_songs,
            get_playlists,
            get_all_playlists,
            get_playlist_songs,
            get_song_url,
            get_song_info,
            parse_kugou_playlist,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
    println!("After tauri::Builder::run()");
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

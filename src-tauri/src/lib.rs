mod source_engine;

use source_engine::*;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{Emitter, State};

struct AppState {
    engine: Arc<SourceEngine>,
    download_dir: Mutex<PathBuf>,
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
async fn get_song_url_fallback(
    title: String,
    artist: String,
    failed_source_id: usize,
    failed_song_id: String,
    failed_platform: String,
) -> Result<SongUrlResult, String> {
    let engine = get_engine();
    let keyword = format!("{} {}", title.trim(), artist.trim())
        .trim()
        .to_string();
    if keyword.is_empty() {
        return Err("无法兜底解析：歌曲名为空".to_string());
    }
    let response = engine.search(&keyword).await;
    for song in response.results {
        if song.id == failed_song_id
            && song.source_id == failed_source_id
            && song.platform == failed_platform
        {
            continue;
        }
        if let Some(url) = engine
            .get_song_url(song.source_id, &song.id, &song.platform)
            .await
        {
            return Ok(url);
        }
    }
    Err(format!(
        "获取播放链接失败：source_id={}, song_id={}, platform={}。已尝试重新搜索其它音源，仍未找到可播放链接，请检查当前网络或稍后重试。",
        failed_source_id, failed_song_id, failed_platform
    ))
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

async fn download_song_to_dir(
    dir: PathBuf,
    url: String,
    title: String,
    artist: String,
    format: String,
) -> Result<String, String> {
    if url.trim().is_empty() {
        return Err("播放链接为空，无法下载".to_string());
    }

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|err| err.to_string())?;
    let bytes = client
        .get(&url)
        .send()
        .await
        .map_err(|err| format!("下载请求失败: {err}"))?
        .bytes()
        .await
        .map_err(|err| format!("读取下载内容失败: {err}"))?;

    std::fs::create_dir_all(&dir).map_err(|err| format!("创建下载目录失败: {err}"))?;
    let extension = if format.trim().is_empty() {
        "mp3"
    } else {
        format.trim()
    };
    let file_name = format!(
        "{} - {}.{}",
        sanitize_file_name(&title),
        sanitize_file_name(&artist),
        sanitize_file_name(extension)
    );
    let path = dir.join(file_name);
    let mut file = std::fs::File::create(&path).map_err(|err| format!("创建文件失败: {err}"))?;
    file.write_all(&bytes)
        .map_err(|err| format!("写入文件失败: {err}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
async fn download_song(
    state: State<'_, AppState>,
    url: String,
    title: String,
    artist: String,
    format: String,
) -> Result<String, String> {
    let dir = state
        .download_dir
        .lock()
        .map_err(|_| "下载目录状态被占用".to_string())?
        .clone();
    download_song_to_dir(dir, url, title, artist, format).await
}

#[tauri::command]
async fn download_song_by_id(
    state: State<'_, AppState>,
    source_id: usize,
    song_id: String,
    platform: String,
    title: String,
    artist: String,
) -> Result<String, String> {
    let engine = get_engine();
    let song = engine
        .get_song_url(source_id, &song_id, &platform)
        .await
        .ok_or_else(|| "获取播放链接失败，无法下载".to_string())?;
    let dir = state
        .download_dir
        .lock()
        .map_err(|_| "下载目录状态被占用".to_string())?
        .clone();
    download_song_to_dir(dir, song.url, title, artist, song.format).await
}

#[tauri::command]
fn get_download_dir(state: State<'_, AppState>) -> Result<String, String> {
    let dir = state
        .download_dir
        .lock()
        .map_err(|_| "下载目录状态被占用".to_string())?
        .clone();
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
fn set_download_dir(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("下载目录不能为空".to_string());
    }
    let dir = PathBuf::from(trimmed);
    std::fs::create_dir_all(&dir).map_err(|err| format!("创建下载目录失败: {err}"))?;
    {
        let mut current = state
            .download_dir
            .lock()
            .map_err(|_| "下载目录状态被占用".to_string())?;
        *current = dir.clone();
    }
    save_download_dir(&dir)?;
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
fn list_windows_drives() -> Vec<String> {
    #[cfg(not(target_os = "windows"))]
    {
        return Vec::new();
    }

    #[cfg(target_os = "windows")]
    ('A'..='Z')
        .filter_map(|letter| {
            let drive = format!("{letter}:\\");
            std::path::Path::new(&drive).exists().then_some(drive)
        })
        .collect()
}

#[tauri::command]
async fn parse_kugou_playlist(url: String) -> Result<SharedPlaylist, String> {
    println!("parse_kugou_playlist called, url={}", url);
    parse_kugou_shared_playlist(&url).await
}

fn default_download_dir() -> PathBuf {
    home_dir().join("Downloads").join("MikuTunes")
}

fn app_cache_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("AppData").join("Local"))
            .join("com.lin.music-tauri");
    }

    #[cfg(target_os = "macos")]
    {
        return home_dir()
            .join("Library")
            .join("Caches")
            .join("com.lin.music-tauri");
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        return std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".cache"))
            .join("com.lin.music-tauri");
    }
}

fn settings_path() -> PathBuf {
    app_config_base_dir()
        .join("MikuTunes")
        .join("settings.json")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

#[cfg(target_os = "windows")]
fn app_config_base_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join("AppData").join("Roaming"))
}

#[cfg(target_os = "macos")]
fn app_config_base_dir() -> PathBuf {
    home_dir().join("Library").join("Application Support")
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn app_config_base_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
}

fn load_download_dir() -> PathBuf {
    let path = settings_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return default_download_dir();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return default_download_dir();
    };
    value
        .get("download_dir")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_download_dir)
}

fn save_download_dir(dir: &PathBuf) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建设置目录失败: {err}"))?;
    }
    let value = serde_json::json!({
        "download_dir": dir.to_string_lossy(),
    });
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )
    .map_err(|err| format!("保存设置失败: {err}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    println!("Starting Miku Tunes...");

    let source_dir = resolve_source_dir();
    let audio_cache_dir = app_cache_dir().join("audio-cache");

    println!("Source dir: {:?}", source_dir);
    println!("Audio cache dir: {:?}", audio_cache_dir);
    let engine = Arc::new(SourceEngine::new(source_dir, audio_cache_dir));
    println!(
        "Engine initialized with {} sources",
        engine.get_sources().len()
    );
    let _ = GLOBAL_ENGINE.set(engine.clone());
    let download_dir = load_download_dir();

    println!("Before tauri::Builder::run()...");
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            engine,
            download_dir: Mutex::new(download_dir),
        })
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
            get_song_url_fallback,
            get_song_info,
            get_download_dir,
            set_download_dir,
            list_windows_drives,
            download_song,
            download_song_by_id,
            parse_kugou_playlist,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
    println!("After tauri::Builder::run()");
}

fn sanitize_file_name(value: &str) -> String {
    let mut out = value
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => ch,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();
    if out.is_empty() {
        out = "music".to_string();
    }
    out
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

fn resolve_source_dir() -> std::path::PathBuf {
    let mut candidates = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("音源"));
            candidates.push(exe_dir.join("resources").join("音源"));
            candidates.push(exe_dir.join("Resources").join("音源"));
            candidates.push(exe_dir.join("_up_").join("resources").join("音源"));
            candidates.push(exe_dir.join("..").join("resources").join("音源"));
            candidates.push(exe_dir.join("..").join("Resources").join("音源"));
            candidates.push(exe_dir.join("..").join("..").join("Resources").join("音源"));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("音源"));
        candidates.push(cwd.join("resources").join("音源"));
        candidates.push(cwd.join("..").join("音源"));
    }

    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(parent) = manifest_dir.parent() {
        candidates.push(parent.join("音源"));
    }

    for path in candidates {
        if path.exists() {
            return path;
        }
    }

    std::env::current_dir().unwrap_or_default()
}

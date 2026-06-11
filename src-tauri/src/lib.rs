mod source_engine;

use futures_util::StreamExt;
use source_engine::*;
use std::io::{Read, Write};
use std::path::PathBuf;
#[cfg(not(target_os = "android"))]
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::AsyncWriteExt;
use tauri::{Emitter, Manager, State};

struct AppState {
    engine: Arc<SourceEngine>,
    download_dir: Mutex<PathBuf>,
}

#[derive(Clone, serde::Serialize)]
struct DownloadProgressEvent {
    title: String,
    artist: String,
    progress: f64,
    status: String,
    message: String,
    path: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct FavoritesData {
    songs: Vec<FavoriteSong>,
    playlists: Vec<FavoritePlaylist>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FavoriteSong {
    id: String,
    title: String,
    artist: String,
    album: Option<String>,
    cover_url: Option<String>,
    duration: Option<f64>,
    source_id: usize,
    source: String,
    platform: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FavoritePlaylist {
    id: String,
    name: String,
    cover: Option<String>,
    song_count: Option<usize>,
    source_id: usize,
    source_name: String,
    external_url: Option<String>,
}

#[derive(Clone, serde::Serialize)]
struct ThemeBackground {
    id: &'static str,
    path: String,
}

#[derive(Clone, serde::Serialize)]
struct ThemeIcon {
    theme_id: String,
    path: String,
}

static GLOBAL_ENGINE: OnceLock<Arc<SourceEngine>> = OnceLock::new();
static GLOBAL_APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static GLOBAL_APP_CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

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
    engine.get_all_rankings(limit.unwrap_or(80)).await
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
    engine.get_all_playlists(limit.unwrap_or(200)).await
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

fn emit_download_event(
    app: &tauri::AppHandle,
    title: &str,
    artist: &str,
    progress: f64,
    status: &str,
    message: impl Into<String>,
    path: Option<String>,
) {
    let _ = app.emit(
        "download_progress",
        DownloadProgressEvent {
            title: title.to_string(),
            artist: artist.to_string(),
            progress: progress.clamp(0.0, 1.0),
            status: status.to_string(),
            message: message.into(),
            path,
        },
    );
}

fn normalized_extension(format: &str) -> String {
    let value = format
        .trim()
        .trim_start_matches('.')
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    if value.is_empty() {
        "mp3".to_string()
    } else {
        value
    }
}

fn target_download_path(
    dir: &std::path::Path,
    title: &str,
    artist: &str,
    format: &str,
) -> PathBuf {
    let extension = normalized_extension(format);
    let file_name = format!(
        "{} - {}.{}",
        sanitize_file_name(title),
        sanitize_file_name(artist),
        sanitize_file_name(&extension)
    );
    dir.join(file_name)
}

fn resolve_local_audio_path(url: &str) -> Option<PathBuf> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("http://asset.localhost/") {
        let encoded = trimmed.trim_start_matches("http://asset.localhost/");
        let decoded = urlencoding::decode(encoded).ok()?.to_string();
        return Some(PathBuf::from(decoded));
    }
    if trimmed.starts_with("https://asset.localhost/") {
        let encoded = trimmed.trim_start_matches("https://asset.localhost/");
        let decoded = urlencoding::decode(encoded).ok()?.to_string();
        return Some(PathBuf::from(decoded));
    }
    if trimmed.starts_with("asset://localhost/") {
        let encoded = trimmed.trim_start_matches("asset://localhost/");
        let decoded = urlencoding::decode(encoded).ok()?.to_string();
        return Some(PathBuf::from(decoded));
    }
    if trimmed.starts_with("file://localhost/") {
        let encoded = trimmed.trim_start_matches("file://localhost/");
        let decoded = urlencoding::decode(encoded).ok()?.to_string();
        return Some(PathBuf::from(decoded));
    }
    if trimmed.starts_with("file:///") {
        let encoded = trimmed.trim_start_matches("file://");
        let decoded = urlencoding::decode(encoded).ok()?.to_string();
        return Some(PathBuf::from(decoded));
    }
    if trimmed.starts_with('/')
        || trimmed.starts_with("\\\\")
        || trimmed.contains(":\\")
    {
        return Some(PathBuf::from(trimmed));
    }
    None
}

fn copy_local_file_with_progress(
    app: &tauri::AppHandle,
    source_path: &std::path::Path,
    target_path: &std::path::Path,
    title: &str,
    artist: &str,
) -> Result<(), String> {
    let total = std::fs::metadata(source_path)
        .map(|meta| meta.len())
        .map_err(|err| format!("读取缓存文件信息失败: {err}"))?;
    let mut reader =
        std::fs::File::open(source_path).map_err(|err| format!("打开缓存文件失败: {err}"))?;
    let mut writer =
        std::fs::File::create(target_path).map_err(|err| format!("创建下载文件失败: {err}"))?;
    let mut downloaded = 0u64;
    let mut buffer = vec![0u8; 256 * 1024];

    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|err| format!("读取缓存文件失败: {err}"))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|err| format!("写入下载文件失败: {err}"))?;
        downloaded = downloaded.saturating_add(read as u64);
        let progress = if total > 0 {
            downloaded as f64 / total as f64
        } else {
            1.0
        };
        emit_download_event(
            app,
            title,
            artist,
            progress,
            "progress",
            format!("正在下载《{}》", title),
            None,
        );
    }

    Ok(())
}

async fn download_remote_file_with_progress(
    app: &tauri::AppHandle,
    url: &str,
    target_path: &std::path::Path,
    title: &str,
    artist: &str,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|err| err.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| format!("下载请求失败: {err}"))?
        .error_for_status()
        .map_err(|err| format!("下载请求失败: {err}"))?;
    let total = response.content_length();
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(target_path)
        .await
        .map_err(|err| format!("创建下载文件失败: {err}"))?;
    let mut downloaded = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| format!("读取下载内容失败: {err}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|err| format!("写入下载文件失败: {err}"))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        let progress = total
            .map(|value| downloaded as f64 / value.max(1) as f64)
            .unwrap_or(0.0);
        emit_download_event(
            app,
            title,
            artist,
            progress,
            "progress",
            format!("正在下载《{}》", title),
            None,
        );
    }

    file.flush()
        .await
        .map_err(|err| format!("写入下载文件失败: {err}"))?;
    Ok(())
}

async fn download_song_to_dir(
    app: &tauri::AppHandle,
    dir: PathBuf,
    url: String,
    title: String,
    artist: String,
    format: String,
) -> Result<String, String> {
    if url.trim().is_empty() {
        return Err("播放链接为空，无法下载".to_string());
    }

    let target_dir = match std::fs::create_dir_all(&dir) {
        Ok(_) => dir,
        Err(primary_error) => {
            let fallback = resolve_download_dir_for_app(app);
            std::fs::create_dir_all(&fallback).map_err(|fallback_error| {
                format!(
                    "创建下载目录失败: {primary_error}；回退到应用目录也失败: {fallback_error}"
                )
            })?;
            fallback
        }
    };
    let path = target_download_path(&target_dir, &title, &artist, &format);
    emit_download_event(
        app,
        &title,
        &artist,
        0.0,
        "started",
        format!("开始下载《{}》", title),
        None,
    );

    let result = if let Some(local_path) = resolve_local_audio_path(&url) {
        copy_local_file_with_progress(app, &local_path, &path, &title, &artist)
    } else {
        download_remote_file_with_progress(app, &url, &path, &title, &artist).await
    };

    if let Err(error) = result {
        let _ = std::fs::remove_file(&path);
        emit_download_event(
            app,
            &title,
            &artist,
            0.0,
            "error",
            error.clone(),
            None,
        );
        return Err(error);
    }

    let saved = path.to_string_lossy().to_string();
    emit_download_event(
        app,
        &title,
        &artist,
        1.0,
        "success",
        format!("下载成功：{}", path.file_name().and_then(|name| name.to_str()).unwrap_or("音频文件")),
        Some(saved.clone()),
    );
    Ok(saved)
}

#[tauri::command]
async fn download_song(
    app: tauri::AppHandle,
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
    download_song_to_dir(&app, dir, url, title, artist, format).await
}

#[tauri::command]
async fn download_song_by_id(
    app: tauri::AppHandle,
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
    download_song_to_dir(&app, dir, song.url, title, artist, song.format).await
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
fn load_favorites() -> FavoritesData {
    load_favorites_data()
}

#[tauri::command]
fn save_favorites(data: FavoritesData) -> Result<String, String> {
    save_favorites_data(&data).map(|path| path.to_string_lossy().to_string())
}

#[tauri::command]
fn get_favorites_path() -> String {
    favorites_storage_path().to_string_lossy().to_string()
}

#[tauri::command]
fn pick_download_dir(state: State<'_, AppState>) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        let current = state
            .download_dir
            .lock()
            .map_err(|_| "下载目录状态被占用".to_string())?
            .clone();
        return Err(format!(
            "Android 当前使用应用内部下载目录：{}",
            current.to_string_lossy()
        ));
    }

    #[cfg(not(target_os = "android"))]
    {
    let current = state
        .download_dir
        .lock()
        .map_err(|_| "下载目录状态被占用".to_string())?
        .clone();
    let selected = rfd::FileDialog::new()
        .set_directory(current)
        .pick_folder();
    let Some(dir) = selected else {
        return Ok(None);
    };
    std::fs::create_dir_all(&dir).map_err(|err| format!("创建下载目录失败: {err}"))?;
    {
        let mut download_dir = state
            .download_dir
            .lock()
            .map_err(|_| "下载目录状态被占用".to_string())?;
        *download_dir = dir.clone();
    }
    save_download_dir(&dir)?;
    Ok(Some(dir.to_string_lossy().to_string()))
    }
}

#[tauri::command]
fn reveal_in_folder(path: String) -> Result<(), String> {
    let target = PathBuf::from(path.trim());
    if !target.exists() {
        return Err("目标文件或目录不存在".to_string());
    }

    #[cfg(target_os = "android")]
    {
        let _ = target;
        return Err("Android 版本暂不支持直接打开文件夹，请在系统文件管理器中查看应用目录".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let status = if target.is_dir() {
            Command::new("open").arg(&target).status()
        } else {
            Command::new("open").arg("-R").arg(&target).status()
        }
        .map_err(|err| format!("打开 Finder 失败: {err}"))?;
        if status.success() {
            return Ok(());
        }
        return Err("打开 Finder 失败".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let status = if target.is_dir() {
            Command::new("explorer").arg(&target).status()
        } else {
            Command::new("explorer")
                .arg("/select,")
                .arg(&target)
                .status()
        }
        .map_err(|err| format!("打开文件夹失败: {err}"))?;
        if status.success() {
            return Ok(());
        }
        return Err("打开文件夹失败".to_string());
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows"), not(target_os = "android")))]
    {
        let open_target = if target.is_dir() {
            target.clone()
        } else {
            target
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or(target.clone())
        };
        let status = Command::new("xdg-open")
            .arg(&open_target)
            .status()
            .map_err(|err| format!("打开文件夹失败: {err}"))?;
        if status.success() {
            return Ok(());
        }
        return Err("打开文件夹失败".to_string());
    }
}

#[tauri::command]
async fn pick_theme_image(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let result = app
        .dialog()
        .file()
        .add_filter("Image", &["png", "jpg", "jpeg", "webp", "gif", "bmp", "svg", "avif"])
        .blocking_pick_file()
        .map(|p| p.to_string());
    Ok(result)
}

#[tauri::command]
fn read_theme_image_data_url(path: String) -> Result<String, String> {
    use base64::Engine;

    let path = PathBuf::from(path.trim());
    if !path.exists() {
        return Err("图片文件不存在".to_string());
    }

    let bytes = std::fs::read(&path).map_err(|err| format!("读取图片失败: {err}"))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("图片过大，请选择 8MB 以内的图片".to_string());
    }

    let mime = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

#[tauri::command]
fn get_theme_backgrounds() -> Vec<ThemeBackground> {
    let dir = resolve_bgimg_dir();
    [
        ("miku", "MIKUNT.jpg"),
        ("kuromi", "kuluomi.webp"),
        ("bamboo", "zhu.webp"),
        ("newyear", "newyear.webp"),
    ]
    .into_iter()
    .filter_map(|(id, file)| {
        let path = dir.join(file);
        path.exists().then(|| ThemeBackground {
            id,
            path: path.to_string_lossy().to_string(),
        })
    })
    .collect()
}

#[tauri::command]
fn get_theme_icons() -> Vec<ThemeIcon> {
    let dir = resolve_bgimg_dir();
    let folders = [
        ("miku", "chuyin"),
        ("kuromi", "kuluomi"),
        ("bamboo", "zhu"),
        ("newyear", "newyear"),
    ];
    let mut icons = Vec::new();
    for (theme_id, folder) in folders {
        let folder_path = dir.join(folder);
        let Ok(entries) = std::fs::read_dir(folder_path) else {
            continue;
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
                    return false;
                };
                let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
                    return false;
                };
                name.to_ascii_lowercase().starts_with("icon")
                    && matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg" | "avif"
                    )
            })
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| name.trim_start_matches("icon").parse::<usize>().ok())
                .unwrap_or(usize::MAX)
        });
        icons.extend(paths.into_iter().map(|path| ThemeIcon {
            theme_id: theme_id.to_string(),
            path: path.to_string_lossy().to_string(),
        }));
    }
    icons
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

#[cfg(not(target_os = "android"))]
fn default_download_dir() -> PathBuf {
    home_dir().join("Downloads").join("MikuTunes")
}

fn app_cache_dir() -> PathBuf {
    #[cfg(target_os = "android")]
    {
        if let Some(path) = GLOBAL_APP_CACHE_DIR.get() {
            return path.clone();
        }
    }

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
    #[cfg(target_os = "android")]
    {
        if let Some(path) = GLOBAL_APP_DATA_DIR.get() {
            return path.clone();
        }
    }

    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
}

fn resolve_managed_data_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| home_dir().join(".config").join("com.lin.music-tauri"))
}

fn resolve_managed_cache_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_cache_dir()
        .unwrap_or_else(|_| app_cache_dir())
}

fn resolve_download_dir_for_app(app: &tauri::AppHandle) -> PathBuf {
    #[cfg(target_os = "android")]
    {
        let dir = resolve_managed_data_dir(app).join("downloads");
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }

    #[cfg(not(target_os = "android"))]
    {
        let configured = load_download_dir();
        if std::fs::create_dir_all(&configured).is_ok() {
            return configured;
        }

        let dir = app
            .path()
            .download_dir()
            .map(|path| path.join("MikuTunes"))
            .unwrap_or_else(|_| default_download_dir());
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}

#[cfg(not(target_os = "android"))]
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

fn favorites_file_name() -> &'static str {
    "favorites.json"
}

fn portable_data_dir() -> PathBuf {
    let app_name = "MikuTunesData";

    if let Ok(exe) = std::env::current_exe() {
        let mut cursor = exe.parent().map(std::path::Path::to_path_buf);
        while let Some(path) = cursor {
            if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("app"))
            {
                if let Some(parent) = path.parent() {
                    return parent.join(app_name);
                }
            }
            cursor = path.parent().map(std::path::Path::to_path_buf);
        }

        if let Some(parent) = exe.parent() {
            return parent.join(app_name);
        }
    }

    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(app_name)
}

fn fallback_data_dir() -> PathBuf {
    app_config_base_dir().join("MikuTunes")
}

fn portable_favorites_path() -> PathBuf {
    portable_data_dir().join(favorites_file_name())
}

fn fallback_favorites_path() -> PathBuf {
    fallback_data_dir().join(favorites_file_name())
}

fn ensure_parent(path: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建数据目录失败: {err}"))?;
    }
    Ok(())
}

fn write_favorites_data(path: &std::path::Path, data: &FavoritesData) -> Result<(), String> {
    ensure_parent(path)?;
    let raw = serde_json::to_string_pretty(data).map_err(|err| format!("序列化收藏失败: {err}"))?;
    std::fs::write(path, raw).map_err(|err| format!("保存收藏失败: {err}"))
}

fn read_favorites_data(path: &std::path::Path) -> Option<FavoritesData> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<FavoritesData>(&raw).ok()
}

fn favorites_storage_path() -> PathBuf {
    let portable = portable_favorites_path();
    if portable.exists() {
        return portable;
    }
    let fallback = fallback_favorites_path();
    if fallback.exists() {
        return fallback;
    }
    portable
}

fn load_favorites_data() -> FavoritesData {
    read_favorites_data(&portable_favorites_path())
        .or_else(|| read_favorites_data(&fallback_favorites_path()))
        .unwrap_or_default()
}

fn save_favorites_data(data: &FavoritesData) -> Result<PathBuf, String> {
    let portable = portable_favorites_path();
    match write_favorites_data(&portable, data) {
        Ok(_) => Ok(portable),
        Err(primary_error) => {
            let fallback = fallback_favorites_path();
            write_favorites_data(&fallback, data).map_err(|fallback_error| {
                format!(
                    "保存收藏失败：同级目录写入失败（{}）；应用数据目录写入也失败（{}）",
                    primary_error, fallback_error
                )
            })?;
            Ok(fallback)
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    println!("Starting Miku Tunes...");

    println!("Before tauri::Builder::run()...");
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_handle = app.handle().clone();
            let source_dir = resolve_source_dir();
            let data_dir = resolve_managed_data_dir(&app_handle);
            let cache_dir = resolve_managed_cache_dir(&app_handle);
            let audio_cache_dir = cache_dir.join("audio-cache");

            let _ = std::fs::create_dir_all(&data_dir);
            let _ = std::fs::create_dir_all(&audio_cache_dir);
            let _ = GLOBAL_APP_DATA_DIR.set(data_dir);
            let _ = GLOBAL_APP_CACHE_DIR.set(cache_dir);

            println!("Source dir: {:?}", source_dir);
            println!("Audio cache dir: {:?}", audio_cache_dir);

            let engine = Arc::new(SourceEngine::new(source_dir, audio_cache_dir));
            println!(
                "Engine initialized with {} sources",
                engine.get_sources().len()
            );
            let _ = GLOBAL_ENGINE.set(engine.clone());

            let download_dir = resolve_download_dir_for_app(&app_handle);
            let _ = std::fs::create_dir_all(&download_dir);
            if cfg!(target_os = "android") {
                let _ = save_download_dir(&download_dir);
            }

            app.manage(AppState {
                engine,
                download_dir: Mutex::new(download_dir),
            });
            Ok(())
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
            load_favorites,
            save_favorites,
            get_favorites_path,
            pick_download_dir,
            reveal_in_folder,
            pick_theme_image,
            read_theme_image_data_url,
            get_theme_backgrounds,
            get_theme_icons,
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

fn resolve_bgimg_dir() -> std::path::PathBuf {
    let mut candidates = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("bgimg"));
            candidates.push(exe_dir.join("resources").join("bgimg"));
            candidates.push(exe_dir.join("_up_").join("resources").join("bgimg"));
            candidates.push(exe_dir.join("..").join("bgimg"));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("bgimg"));
        candidates.push(cwd.join("resources").join("bgimg"));
        candidates.push(cwd.join("..").join("bgimg"));
    }

    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(parent) = manifest_dir.parent() {
        candidates.push(parent.join("bgimg"));
    }

    for path in candidates {
        if path.exists() {
            return path;
        }
    }

    std::env::current_dir().unwrap_or_default().join("bgimg")
}

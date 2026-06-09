use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_SEARCH_CONCURRENT: usize = 6;

// ─── Source descriptor ───
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: usize,
    pub name: String,
    pub file_name: String,
    pub score: i32,
    pub enabled: bool,
}

/// Parsed from each JS source file
#[derive(Debug, Clone)]
struct SourceMeta {
    name: String,
    api_url: String,
    api_key: String,
    platforms: Vec<String>, // e.g. ["kw", "kg", "tx", "wy", "mg"]
    adapter: Option<NativeSourceAdapter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeSourceAdapter {
    Xiaowo,
}

// ─── Search result ───
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongResult {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub source: String,
    pub source_id: usize,
    pub platform: String,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub duration: Option<f64>,
    pub quality: Option<String>,
    pub score: i32,
}

// ─── Search response ───
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SongResult>,
    pub total: usize,
    pub from_source: Option<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchBatch {
    pub results: Vec<SongResult>,
    pub from_source: Option<String>,
}

// ─── Hot keyword result ───
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotItem {
    pub title: String,
    pub source: String,
    pub source_id: usize,
}

// ─── Ranking / Playlist types ───
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingCategory {
    pub id: String,
    pub name: String,
    pub source_id: usize,
    pub source_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistInfo {
    pub id: String,
    pub name: String,
    pub cover: Option<String>,
    pub song_count: Option<usize>,
    pub source_id: usize,
    pub source_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongDetail {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub cover_url: Option<String>,
    pub duration: Option<f64>,
    pub source_id: usize,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongUrlResult {
    pub url: String,
    pub quality: String,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongInfoResult {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub lyrics: Option<String>,
    pub duration: Option<f64>,
    pub platform: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SharedPlaylist {
    pub playlist: PlaylistInfo,
    pub songs: Vec<SongDetail>,
    pub external_url: String,
    pub note: Option<String>,
}

// ─── Engine state ───
pub struct SourceEngine {
    sources: Mutex<Vec<SourceInfo>>,
    meta_map: Mutex<HashMap<usize, SourceMeta>>,
    active_source: Mutex<usize>,
    http_client: reqwest::Client,
    source_dir: PathBuf,
    audio_cache_dir: PathBuf,
}

impl SourceEngine {
    pub fn new(source_dir: PathBuf) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            .connect_timeout(Duration::from_secs(2))
            .danger_accept_invalid_certs(true)
            .user_agent("MikuTunes/1.0")
            .build()
            .unwrap_or_default();

        let audio_cache_dir = std::env::temp_dir().join("music-tauri-audio-cache");
        if let Err(error) = fs::create_dir_all(&audio_cache_dir) {
            println!(
                "Failed to create audio cache dir {:?}: {}",
                audio_cache_dir, error
            );
        }

        let engine = Self {
            sources: Mutex::new(Vec::new()),
            meta_map: Mutex::new(HashMap::new()),
            active_source: Mutex::new(0),
            http_client,
            source_dir,
            audio_cache_dir,
        };
        engine.scan_sources();
        engine
    }

    /// Scan 音源 directory, parse each JS file for metadata
    pub fn scan_sources(&self) {
        let mut scanned = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.source_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "js") {
                    let file_name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    // Parse JS file for metadata
                    let (name, meta) = parse_source_file(&path, 0, &file_name);
                    scanned.push((file_name, name, meta));
                }
            }
        }

        scanned.sort_by(|left, right| left.0.cmp(&right.0));

        let mut sources = Vec::with_capacity(scanned.len());
        let mut meta_map = HashMap::new();
        for (id, (file_name, name, meta)) in scanned.into_iter().enumerate() {
            sources.push(SourceInfo {
                id,
                name,
                score: 0,
                enabled: true,
                file_name,
            });
            if let Some(meta) = meta {
                meta_map.insert(id, meta);
            }
        }

        if let Ok(mut current) = self.sources.lock() {
            *current = sources;
        }
        if let Ok(mut map) = self.meta_map.lock() {
            *map = meta_map;
        }
    }

    pub fn get_sources(&self) -> Vec<SourceInfo> {
        self.sources.lock().unwrap().clone()
    }

    pub fn get_active_source(&self) -> Option<SourceInfo> {
        let active_id = *self.active_source.lock().unwrap();
        let sources = self.sources.lock().unwrap();
        sources.iter().find(|s| s.id == active_id).cloned()
    }

    pub fn switch_source(&self, id: usize) -> bool {
        let sources = self.sources.lock().unwrap();
        if sources.iter().any(|s| s.id == id) {
            *self.active_source.lock().unwrap() = id;
            true
        } else {
            false
        }
    }

    pub fn set_source_enabled(&self, source_id: usize, enabled: bool) -> Vec<SourceInfo> {
        if let Ok(mut sources) = self.sources.lock() {
            if let Some(source) = sources.iter_mut().find(|s| s.id == source_id) {
                source.enabled = enabled;
            }
            if enabled {
                *self.active_source.lock().unwrap() = source_id;
            }
            return sources.clone();
        }
        Vec::new()
    }

    pub fn move_source(&self, source_id: usize, action: &str) -> Vec<SourceInfo> {
        if let Ok(mut sources) = self.sources.lock() {
            let Some(index) = sources.iter().position(|s| s.id == source_id) else {
                return sources.clone();
            };
            match action {
                "up" if index > 0 => sources.swap(index, index - 1),
                "down" if index + 1 < sources.len() => sources.swap(index, index + 1),
                "top" if index > 0 => {
                    let source = sources.remove(index);
                    sources.insert(0, source);
                }
                _ => {}
            }
            return sources.clone();
        }
        Vec::new()
    }

    pub fn score_source(&self, source_id: usize, found: bool) {
        if let Ok(mut sources) = self.sources.lock() {
            if let Some(source) = sources.iter_mut().find(|s| s.id == source_id) {
                if found {
                    source.score += 1;
                } else {
                    source.score -= 1;
                }
            }
        }
    }

    pub async fn search(&self, keyword: &str) -> SearchResponse {
        self.search_with_batches(keyword, |_| {}).await
    }

    pub async fn search_more(&self, keyword: &str, offset: usize) -> SearchResponse {
        let start = Instant::now();
        let sources = self
            .sources
            .lock()
            .unwrap()
            .clone()
            .into_iter()
            .filter(|source| source.enabled)
            .collect::<Vec<_>>();
        let meta_map = self.meta_map.lock().unwrap().clone();
        let client = self.http_client.clone();
        let page_start = (offset / 100).max(3);
        let mut all_results = Vec::new();

        for source in sources.into_iter().filter(|source| {
            source.name.eq_ignore_ascii_case("xiaowo")
                || source.file_name.eq_ignore_ascii_case("xiaowo")
        }) {
            let Some(meta) = meta_map.get(&source.id).cloned() else {
                continue;
            };
            let results = search_via_xiaowo_pages(&client, &meta, keyword, page_start, 3).await;
            for mut item in results {
                item.source_id = source.id;
                item.score = source.score;
                all_results.push(item);
            }
        }

        SearchResponse {
            total: all_results.len(),
            results: all_results,
            from_source: Some("xiaowo".to_string()),
            elapsed_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// Search music from ALL sources and report each source batch as soon as it finishes.
    pub async fn search_with_batches<F>(&self, keyword: &str, mut on_batch: F) -> SearchResponse
    where
        F: FnMut(SearchBatch),
    {
        println!("Searching for keyword: '{}'", keyword);
        let start = Instant::now();
        let sources = {
            self.sources
                .lock()
                .unwrap()
                .clone()
                .into_iter()
                .filter(|s| s.enabled)
                .collect::<Vec<_>>()
        };
        let meta_map = self.meta_map.lock().unwrap().clone();
        let client = self.http_client.clone();
        let kw = keyword.to_string();

        let mut all_results = Vec::new();
        let mut found_source: Option<String> = None;

        let mut priority_sources = sources
            .iter()
            .filter(|source| {
                source.name.eq_ignore_ascii_case("xiaowo")
                    || source.file_name.eq_ignore_ascii_case("xiaowo")
            })
            .cloned()
            .collect::<Vec<_>>();
        priority_sources.sort_by(|a, b| {
            let a_xiaowo =
                a.name.eq_ignore_ascii_case("xiaowo") || a.file_name.eq_ignore_ascii_case("xiaowo");
            let b_xiaowo =
                b.name.eq_ignore_ascii_case("xiaowo") || b.file_name.eq_ignore_ascii_case("xiaowo");
            b_xiaowo.cmp(&a_xiaowo).then_with(|| b.score.cmp(&a.score))
        });

        let mut searched_source_ids = Vec::new();
        for source in priority_sources {
            let Some(meta) = meta_map.get(&source.id).cloned() else {
                continue;
            };
            searched_source_ids.push(source.id);
            let search_future = async {
                match meta.adapter {
                    Some(NativeSourceAdapter::Xiaowo) => {
                        search_via_xiaowo(&client, &meta, &kw).await
                    }
                    None => search_via_lxmusic(&client, &meta, &kw).await,
                }
            };
            let results = tokio::time::timeout(Duration::from_secs(5), search_future)
                .await
                .unwrap_or_default();
            println!(
                "Priority source {} found {} results for '{}'",
                source.name,
                results.len(),
                keyword
            );
            if !results.is_empty() {
                found_source = Some(source.name.clone());
                let mut batch_results = Vec::new();
                for mut r in results {
                    r.source_id = source.id;
                    r.score = source.score;
                    all_results.push(r.clone());
                    batch_results.push(r);
                }
                on_batch(SearchBatch {
                    results: batch_results,
                    from_source: Some(source.name.clone()),
                });
            }
        }

        let mut remaining_sources = sources
            .into_iter()
            .filter(|source| !searched_source_ids.contains(&source.id))
            .collect::<Vec<_>>();
        remaining_sources.sort_by(|a, b| b.score.cmp(&a.score));

        for batch in remaining_sources.chunks(MAX_SEARCH_CONCURRENT) {
            let mut tasks = FuturesUnordered::new();
            for source in batch.iter().cloned() {
                let Some(meta) = meta_map.get(&source.id).cloned() else {
                    continue;
                };
                let client = client.clone();
                let kw = kw.clone();
                tasks.push(tokio::spawn(async move {
                    let search_future = async {
                        match meta.adapter {
                            Some(NativeSourceAdapter::Xiaowo) => {
                                search_via_xiaowo(&client, &meta, &kw).await
                            }
                            None => search_via_lxmusic(&client, &meta, &kw).await,
                        }
                    };
                    let results = tokio::time::timeout(Duration::from_secs(5), search_future)
                        .await
                        .unwrap_or_default();
                    (source, results)
                }));
            }

            while let Some(task_result) = tasks.next().await {
                match task_result {
                    Ok((source, results)) => {
                        println!(
                            "Source {} found {} results for '{}'",
                            source.name,
                            results.len(),
                            keyword
                        );
                        if !results.is_empty() && found_source.is_none() {
                            found_source = Some(source.name.clone());
                        }
                        let mut batch_results = Vec::new();
                        for mut r in results {
                            r.source_id = source.id;
                            r.score = source.score;
                            all_results.push(r.clone());
                            batch_results.push(r);
                        }
                        if !batch_results.is_empty() {
                            on_batch(SearchBatch {
                                results: batch_results,
                                from_source: Some(source.name.clone()),
                            });
                        }
                    }
                    Err(e) => {
                        println!("Search task error: {:?}", e);
                    }
                }
            }
        }

        println!("Search complete: {} total results", all_results.len());
        SearchResponse {
            total: all_results.len(),
            results: all_results,
            from_source: found_source,
            elapsed_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// Fetch hot search keywords from sources
    pub async fn hot_keywords(&self, limit: usize) -> Vec<HotItem> {
        let meta_map = self.meta_map.lock().unwrap().clone();
        println!(
            "hot_keywords called, meta_map has {} entries",
            meta_map.len()
        );
        let client = self.http_client.clone();

        let tasks: Vec<_> = meta_map
            .into_iter()
            .filter_map(|(id, meta)| {
                if meta.adapter.is_some() || meta.api_url.is_empty() {
                    return None;
                }
                let client = client.clone();
                Some(tokio::spawn(async move {
                    let items = hot_via_lxmusic(&client, &meta).await;
                    (id, meta.name, items)
                }))
            })
            .collect();

        let mut results = Vec::new();
        for task in tasks {
            // Add per-task timeout so slow sources don't block everything
            match tokio::time::timeout(std::time::Duration::from_secs(6), task).await {
                Ok(Ok((id, name, items))) => {
                    println!("Source {}: found {} hot keywords", name, items.len());
                    for item in items {
                        results.push(HotItem {
                            title: item,
                            source: name.clone(),
                            source_id: id,
                        });
                    }
                }
                Ok(Err(e)) => {
                    println!("Hot keywords task error: {:?}", e);
                }
                Err(_) => {
                    println!("Hot keywords task timeout");
                }
            }
            // Return early if we have enough results
            if results.len() >= limit * 2 {
                break;
            }
        }

        results.truncate(limit);
        println!("Returning {} hot keywords", results.len());
        results
    }

    /// Fetch rankings from a specific source
    pub async fn get_rankings(&self, source_id: usize) -> Vec<RankingCategory> {
        let meta_map = self.meta_map.lock().unwrap().clone();
        let meta = match meta_map.get(&source_id) {
            Some(m) => m.clone(),
            None => return vec![],
        };
        let client = self.http_client.clone();
        match meta.adapter {
            Some(NativeSourceAdapter::Xiaowo) => {
                fetch_xiaowo_rankings(&client, source_id, &meta.name).await
            }
            None => fetch_rankings(&client, &meta, source_id).await,
        }
    }

    pub async fn get_all_rankings(&self, limit: usize) -> Vec<RankingCategory> {
        let sources = self
            .sources
            .lock()
            .unwrap()
            .clone()
            .into_iter()
            .filter(|source| source.enabled)
            .collect::<Vec<_>>();
        let meta_map = self.meta_map.lock().unwrap().clone();
        let client = self.http_client.clone();
        let mut tasks = FuturesUnordered::new();
        for source in sources {
            let Some(meta) = meta_map.get(&source.id).cloned() else {
                continue;
            };
            let client = client.clone();
            tasks.push(tokio::spawn(async move {
                match meta.adapter {
                    Some(NativeSourceAdapter::Xiaowo) => {
                        fetch_xiaowo_rankings(&client, source.id, &meta.name).await
                    }
                    None => fetch_rankings(&client, &meta, source.id).await,
                }
            }));
        }

        let mut out = Vec::new();
        while let Some(result) = tasks.next().await {
            if let Ok(items) = result {
                out.extend(items);
            }
            if out.len() >= limit {
                break;
            }
        }
        out.truncate(limit);
        out
    }

    /// Fetch songs in a ranking
    pub async fn get_ranking_songs(&self, source_id: usize, ranking_id: &str) -> Vec<SongDetail> {
        let meta_map = self.meta_map.lock().unwrap().clone();
        let meta = match meta_map.get(&source_id) {
            Some(m) => m.clone(),
            None => return vec![],
        };
        let client = self.http_client.clone();
        match meta.adapter {
            Some(NativeSourceAdapter::Xiaowo) => {
                fetch_xiaowo_ranking_songs(&client, source_id, ranking_id).await
            }
            None => fetch_ranking_songs(&client, &meta, source_id, ranking_id).await,
        }
    }

    /// Fetch playlists from a specific source
    pub async fn get_playlists(&self, source_id: usize) -> Vec<PlaylistInfo> {
        let meta_map = self.meta_map.lock().unwrap().clone();
        let meta = match meta_map.get(&source_id) {
            Some(m) => m.clone(),
            None => return vec![],
        };
        let client = self.http_client.clone();
        match meta.adapter {
            Some(NativeSourceAdapter::Xiaowo) => {
                fetch_xiaowo_playlists(&client, source_id, &meta.name).await
            }
            None => fetch_playlists(&client, &meta, source_id).await,
        }
    }

    pub async fn get_all_playlists(&self, limit: usize) -> Vec<PlaylistInfo> {
        let sources = self
            .sources
            .lock()
            .unwrap()
            .clone()
            .into_iter()
            .filter(|source| source.enabled)
            .collect::<Vec<_>>();
        let meta_map = self.meta_map.lock().unwrap().clone();
        let client = self.http_client.clone();
        let mut tasks = FuturesUnordered::new();
        for source in sources {
            let Some(meta) = meta_map.get(&source.id).cloned() else {
                continue;
            };
            let client = client.clone();
            tasks.push(tokio::spawn(async move {
                match meta.adapter {
                    Some(NativeSourceAdapter::Xiaowo) => {
                        fetch_xiaowo_playlists(&client, source.id, &meta.name).await
                    }
                    None => fetch_playlists(&client, &meta, source.id).await,
                }
            }));
        }

        let mut out = Vec::new();
        while let Some(result) = tasks.next().await {
            if let Ok(items) = result {
                out.extend(items);
            }
            if out.len() >= limit {
                break;
            }
        }
        out.truncate(limit);
        out
    }

    /// Fetch songs in a playlist
    pub async fn get_playlist_songs(&self, source_id: usize, playlist_id: &str) -> Vec<SongDetail> {
        let meta_map = self.meta_map.lock().unwrap().clone();
        let meta = match meta_map.get(&source_id) {
            Some(m) => m.clone(),
            None => return vec![],
        };
        let client = self.http_client.clone();
        match meta.adapter {
            Some(NativeSourceAdapter::Xiaowo) => {
                fetch_xiaowo_playlist_songs(&client, source_id, playlist_id).await
            }
            None => fetch_playlist_songs(&client, &meta, source_id, playlist_id).await,
        }
    }

    /// Get playable URL for a song
    pub async fn get_song_url(
        &self,
        source_id: usize,
        song_id: &str,
        platform: &str,
    ) -> Option<SongUrlResult> {
        let meta_map = self.meta_map.lock().unwrap().clone();
        let meta = match meta_map.get(&source_id) {
            Some(m) => m.clone(),
            None => return None,
        };
        let client = self.http_client.clone();
        let direct = match meta.adapter {
            Some(NativeSourceAdapter::Xiaowo) => fetch_xiaowo_song_url(&client, song_id).await,
            None => fetch_song_url(&client, &meta, platform, song_id).await,
        }?;
        materialize_song_url(&client, &self.audio_cache_dir, direct).await
    }

    /// Get detailed song info (including lyrics)
    pub async fn get_song_info(
        &self,
        source_id: usize,
        song_id: &str,
        platform: &str,
    ) -> Option<SongInfoResult> {
        let meta_map = self.meta_map.lock().unwrap().clone();
        let meta = match meta_map.get(&source_id) {
            Some(m) => m.clone(),
            None => return None,
        };
        let client = self.http_client.clone();
        match meta.adapter {
            Some(NativeSourceAdapter::Xiaowo) => fetch_xiaowo_song_info(&client, song_id).await,
            None => fetch_song_info(&client, &meta, source_id, platform, song_id).await,
        }
    }
}

// ─── Parse JS source file ───

fn parse_source_file(
    path: &std::path::Path,
    _id: usize,
    file_name: &str,
) -> (String, Option<SourceMeta>) {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            println!("Failed to read source file {:?}: {:?}", path, e);
            return (file_name.to_string(), None);
        }
    };

    let name = extract_source_name(&content).unwrap_or_else(|| file_name.to_string());
    let api_url = extract_js_var(&content, "API_URL").map(|s| s.trim_matches('"').to_string());
    let api_key = extract_js_var(&content, "API_KEY").map(|s| s.trim_matches('"').to_string());
    let quality_str = extract_js_var(&content, "MUSIC_QUALITY");

    let platforms = quality_str
        .and_then(|s| {
            // Parse JSON object like {"kw":["128k"],"kg":["128k"],...}
            serde_json::from_str::<HashMap<String, serde_json::Value>>(&s)
                .ok()
                .map(|map| map.keys().cloned().collect::<Vec<_>>())
        })
        .unwrap_or_else(|| vec!["kw".to_string()]); // default fallback

    if file_name.eq_ignore_ascii_case("xiaowo") {
        println!("Parsed source '{}': native adapter=Xiaowo", name);
        return (
            name.clone(),
            Some(SourceMeta {
                name,
                api_url: String::new(),
                api_key: String::new(),
                platforms: vec!["kw".to_string()],
                adapter: Some(NativeSourceAdapter::Xiaowo),
            }),
        );
    }

    let meta = api_url.map(|url| {
        println!(
            "Parsed source '{}': API_URL={}, platforms={:?}",
            name, url, platforms
        );
        SourceMeta {
            name: name.clone(),
            api_url: url,
            api_key: api_key.unwrap_or_default(),
            platforms,
            adapter: None,
        }
    });

    (name, meta)
}

/// Extract a JS variable value (const/let/var NAME = value)
fn extract_js_var(content: &str, var_name: &str) -> Option<String> {
    // Try patterns like:
    // const API_URL = "value"
    // const API_URL = 'value'
    // const API_URL = `value`
    // var API_URL = "value"
    // let API_URL = "value"
    // Also JSON.parse('{"key":...}') for MUSIC_QUALITY

    for line in content.lines() {
        let trimmed = line.trim();
        // Skip comments
        if trimmed.starts_with("//") || trimmed.starts_with("*") {
            continue;
        }

        // Pattern: const/let/var API_URL = ...
        let keyword = format!("{} =", var_name);
        if let Some(idx) = trimmed.find(&keyword) {
            let rest = trimmed[idx + keyword.len()..].trim();
            // Handle JSON.parse('...')
            if rest.starts_with("JSON.parse(") {
                if let Some(start) = rest.find('\'') {
                    if let Some(end) = rest[start + 1..].find('\'') {
                        let inner = &rest[start + 1..start + 1 + end];
                        // Unescape
                        return Some(inner.replace("\\'", "'"));
                    }
                }
            }
            // Handle quoted strings: "value" or 'value' or `value`
            for quote in ['"', '\'', '`'].iter() {
                if rest.starts_with(*quote) {
                    let end = rest[1..].find(*quote)?;
                    return Some(rest[1..1 + end].to_string());
                }
            }
            // Handle JSON.parse(`...`)
            if rest.contains("JSON.parse(`") {
                if let Some(start) = rest.find('`') {
                    if let Some(end) = rest[start + 1..].find('`') {
                        return Some(rest[start + 1..start + 1 + end].to_string());
                    }
                }
            }
            // No quotes -> raw value
            let val = rest
                .split(&[' ', ';', ',', '\n', '\r'][..])
                .next()
                .unwrap_or(rest)
                .to_string();
            if !val.is_empty() && val != "=" {
                return Some(val);
            }
        }
    }
    None
}

/// Extract @name from JS comment header
fn extract_source_name(content: &str) -> Option<String> {
    for line in content.lines().take(30) {
        let trimmed = line.trim();
        // @name xxx
        if let Some(idx) = trimmed.find("@name") {
            let rest = trimmed[idx + 5..].trim();
            if !rest.is_empty() {
                return Some(rest.trim_matches('*').trim().to_string());
            }
        }
    }
    None
}

// ─── LxMusic API calls ───

/// Search via LxMusic API Server format: GET {API_URL}/api/v1/search?keyword=xxx&source=xxx
async fn search_via_lxmusic(
    client: &reqwest::Client,
    meta: &SourceMeta,
    keyword: &str,
) -> Vec<SongResult> {
    let mut results = Vec::new();
    let base = meta.api_url.trim_end_matches('/');
    let platforms = preferred_search_platforms(&meta.platforms);

    for platform in &platforms {
        // Use correct format: /api/v1/search?keyword=xxx&source=xxx
        let url = format!("{}/api/v1/search", base);

        println!(
            "Searching via GET URL: {}?keyword={}&source={}",
            url, keyword, platform
        );

        let mut get_request = client
            .get(&url)
            .query(&[("keyword", keyword), ("source", platform.as_str())])
            .timeout(Duration::from_secs(3));
        if !meta.api_key.is_empty() {
            get_request = get_request.header("X-Request-Key", meta.api_key.as_str());
        }

        match get_request.send().await {
            Ok(response) => {
                let status = response.status();
                if !status.is_success() {
                    println!(
                        "GET search failed: source={}, platform={}, status={}",
                        meta.name, platform, status
                    );
                } else if let Ok(json) = response.json::<serde_json::Value>().await {
                    let songs = extract_song_items(&json);

                    println!("Got {} results from GET", songs.len());

                    for item in songs {
                        let id = extract_song_id(&item);
                        let title = extract_song_title(&item);
                        let artist = extract_song_artist(&item);
                        if !title.is_empty() {
                            let album = item
                                .get("album")
                                .or_else(|| item.get("album_name"))
                                .map(json_value_to_string)
                                .filter(|v| !v.is_empty());
                            let duration = item
                                .get("duration")
                                .or_else(|| item.get("dt"))
                                .and_then(json_value_to_f64)
                                .map(normalize_duration_secs);
                            let cover_url = extract_cover_url(&item);
                            let quality = extract_quality_label(&item);
                            results.push(SongResult {
                                id,
                                title,
                                artist,
                                source: format!("{}-{}", meta.name, platform),
                                source_id: 0,
                                platform: platform.clone(),
                                album,
                                cover_url,
                                duration,
                                quality,
                                score: 0,
                            });
                        }
                    }
                } else {
                    println!(
                        "GET search JSON parse failed: source={}, platform={}",
                        meta.name, platform
                    );
                }
            }
            Err(error) => {
                println!(
                    "GET search request failed: source={}, platform={}, error={}",
                    meta.name, platform, error
                );
            }
        }

        // Try fallback POST format
        if results.is_empty() {
            let body = serde_json::json!({
                "keyword": keyword,
                "source": platform,
                "limit": 5,
                "key": meta.api_key,
            });

            let mut post_request = client
                .post(format!("{}/search", base))
                .json(&body)
                .timeout(Duration::from_secs(3));
            if !meta.api_key.is_empty() {
                post_request = post_request.header("X-Request-Key", meta.api_key.as_str());
            }

            match post_request.send().await {
                Ok(response) => {
                    let status = response.status();
                    if !status.is_success() {
                        println!(
                            "POST search failed: source={}, platform={}, status={}",
                            meta.name, platform, status
                        );
                    } else if let Ok(json) = response.json::<serde_json::Value>().await {
                        let songs = extract_song_items(&json);

                        for item in songs {
                            let id = extract_song_id(&item);
                            let title = extract_song_title(&item);
                            let artist = extract_song_artist(&item);
                            if !title.is_empty() {
                                let album = item
                                    .get("album")
                                    .or_else(|| item.get("album_name"))
                                    .map(json_value_to_string)
                                    .filter(|v| !v.is_empty());
                                let duration = item
                                    .get("duration")
                                    .or_else(|| item.get("dt"))
                                    .and_then(json_value_to_f64)
                                    .map(normalize_duration_secs);
                                let cover_url = extract_cover_url(&item);
                                let quality = extract_quality_label(&item);
                                results.push(SongResult {
                                    id,
                                    title,
                                    artist,
                                    source: format!("{}-{}", meta.name, platform),
                                    source_id: 0,
                                    platform: platform.clone(),
                                    album,
                                    cover_url,
                                    duration,
                                    quality,
                                    score: 0,
                                });
                            }
                        }
                    } else {
                        println!(
                            "POST search JSON parse failed: source={}, platform={}",
                            meta.name, platform
                        );
                    }
                }
                Err(error) => {
                    println!(
                        "POST search request failed: source={}, platform={}, error={}",
                        meta.name, platform, error
                    );
                }
            }
        }

        if !results.is_empty() {
            break; // Stop after first successful platform
        }
    }
    results
}

fn preferred_search_platforms(platforms: &[String]) -> Vec<String> {
    let mut preferred = Vec::new();
    for key in ["kw", "tx", "wy", "kg", "mg"] {
        if let Some(platform) = platforms.iter().find(|platform| platform.as_str() == key) {
            preferred.push(platform.clone());
        }
    }

    for platform in platforms {
        if !preferred.iter().any(|item| item == platform) {
            preferred.push(platform.clone());
        }
    }

    preferred.into_iter().take(2).collect()
}

async fn search_via_xiaowo(
    client: &reqwest::Client,
    meta: &SourceMeta,
    keyword: &str,
) -> Vec<SongResult> {
    search_via_xiaowo_pages(client, meta, keyword, 0, 3).await
}

async fn search_via_xiaowo_pages(
    client: &reqwest::Client,
    meta: &SourceMeta,
    keyword: &str,
    page_start: usize,
    page_count: usize,
) -> Vec<SongResult> {
    println!("Searching via Xiaowo adapter: keyword={}", keyword);
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for page in page_start..page_start + page_count {
        let response = client
            .get("http://search.kuwo.cn/r.s")
            .query(&[
                ("client", "kt"),
                ("all", keyword),
                ("pn", &page.to_string()),
                ("rn", "100"),
                ("uid", "2574109560"),
                ("ver", "kwplayer_ar_8.5.4.2"),
                ("vipver", "1"),
                ("ft", "music"),
                ("cluster", "0"),
                ("strategy", "2012"),
                ("encoding", "utf8"),
                ("rformat", "json"),
                ("vermerge", "1"),
                ("mobi", "1"),
            ])
            .timeout(Duration::from_secs(4))
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                println!("Xiaowo search request failed on page {}: {}", page, error);
                break;
            }
        };

        let Ok(json) = response.json::<serde_json::Value>().await else {
            println!("Xiaowo search JSON parse failed on page {}", page);
            break;
        };

        let Some(items) = json.get("abslist").and_then(|value| value.as_array()) else {
            break;
        };

        if items.is_empty() {
            break;
        }

        for item in items {
            let raw_id = item
                .get("MUSICRID")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let id = raw_id.strip_prefix("MUSIC_").unwrap_or(raw_id).to_string();
            let title = item
                .get("NAME")
                .map(json_value_to_string)
                .unwrap_or_default();
            if id.is_empty() || title.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            let artist = item
                .get("ARTIST")
                .map(json_value_to_string)
                .unwrap_or_default();
            let album = item
                .get("ALBUM")
                .map(json_value_to_string)
                .filter(|value| !value.is_empty());
            let cover_url = extract_cover_url(item);
            let quality = extract_quality_label(item).or_else(|| Some("320k".to_string()));
            results.push(SongResult {
                id,
                title,
                artist,
                source: meta.name.clone(),
                source_id: 0,
                platform: "kw".to_string(),
                album,
                cover_url,
                duration: item.get("DURATION").and_then(json_value_to_f64),
                quality,
                score: 0,
            });
        }

        if items.len() < 100 {
            break;
        }
    }

    results
}

async fn fetch_xiaowo_song_url(client: &reqwest::Client, song_id: &str) -> Option<SongUrlResult> {
    println!("Fetching Xiaowo song URL, song_id={}", song_id);

    // ── Attempt 1: lxmusicapi.onrender.com ──
    // (Render free tier cold-starts can take 20-30s, so use a longer timeout)
    println!("  [1/2] Trying lxmusicapi.onrender.com ...");
    let lx_result = client
        .get(format!(
            "https://lxmusicapi.onrender.com/url/kw/{song_id}/320k"
        ))
        .header("X-Request-Key", "share-v3")
        .timeout(Duration::from_secs(15))
        .send()
        .await;

    match &lx_result {
        Ok(resp) => println!("  lxmusicapi HTTP status: {}", resp.status()),
        Err(e) => println!("  lxmusicapi request failed: {:?}", e),
    }

    if let Ok(response) = lx_result {
        match response.json::<serde_json::Value>().await {
            Ok(json) => {
                if let Some(url) = json.get("url").and_then(|value| value.as_str()) {
                    if !url.is_empty() {
                        println!("  lxmusicapi success, url={}", url);
                        return Some(SongUrlResult {
                            url: url.to_string(),
                            quality: "320k".to_string(),
                            format: infer_audio_format(url, None),
                        });
                    }
                }
                println!("  lxmusicapi response has no valid url");
            }
            Err(e) => println!("  lxmusicapi JSON parse failed: {:?}", e),
        }
    }

    // ── Attempt 2: nmobi.kuwo.cn (old Kuwo mobile API) ──
    println!("  [2/2] Falling back to nmobi.kuwo.cn ...");
    let nmobi_result = client
        .get("http://nmobi.kuwo.cn/mobi.s")
        .query(&[
            ("f", "web"),
            ("source", "kwplayer_ar_1.1.9_oppo_118980_320.apk"),
            ("type", "convert_url_with_sign"),
            ("rid", song_id),
            ("br", "320kmp3"),
        ])
        .header("User-Agent", "okhttp/4.10.0")
        .timeout(Duration::from_secs(8))
        .send()
        .await;

    match &nmobi_result {
        Ok(resp) => println!("  nmobi HTTP status: {}", resp.status()),
        Err(e) => println!("  nmobi request failed: {:?}", e),
    }

    let response = nmobi_result.ok()?;

    let json: serde_json::Value = response.json().await.ok()?;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    println!("  nmobi response code={}", code);

    if code != 200 {
        println!("  nmobi API returned error code, msg={:?}", json.get("msg"));
        return None;
    }

    let url = json
        .get("data")
        .and_then(|value| value.get("url"))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .replace("http://", "https://");

    if url.is_empty() {
        println!("  nmobi response has no url in data");
        return None;
    }

    println!("  nmobi success, url={}", url);
    Some(SongUrlResult {
        format: infer_audio_format(&url, None),
        url,
        quality: "320k".to_string(),
    })
}

async fn materialize_song_url(
    _client: &reqwest::Client,
    _cache_dir: &std::path::Path,
    song: SongUrlResult,
) -> Option<SongUrlResult> {
    if song.url.is_empty() {
        return None;
    }
    if song.url.starts_with("data:") {
        return Some(song);
    }
    Some(song)
}

async fn fetch_xiaowo_song_info(client: &reqwest::Client, song_id: &str) -> Option<SongInfoResult> {
    let response = client
        .get("http://m.kuwo.cn/newh5/singles/songinfoandlrc")
        .query(&[("musicId", song_id), ("httpStatus", "1")])
        .header(
            "User-Agent",
            "Mozilla/5.0 (Linux; Android 10) AppleWebKit/537.36 Chrome/88 Mobile Safari/537.36",
        )
        .header("Referer", "http://m.kuwo.cn/")
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?;

    let json = response.json::<serde_json::Value>().await.ok()?;
    let data = json.get("data").unwrap_or(&json);
    let song = data
        .get("songinfo")
        .or_else(|| data.get("songInfo"))
        .or_else(|| data.get("musicInfo"))
        .or_else(|| data.get("song"))
        .unwrap_or(data);
    let title = song
        .get("songName")
        .or_else(|| song.get("songname"))
        .or_else(|| song.get("name"))
        .or_else(|| song.get("title"))
        .map(json_value_to_string)
        .unwrap_or_default();
    let title = if title.is_empty() {
        song_id.to_string()
    } else {
        title
    };

    let cover_url = song
        .get("pic")
        .or_else(|| song.get("pic120"))
        .or_else(|| song.get("pic300"))
        .or_else(|| song.get("albumPic"))
        .or_else(|| song.get("albumpic"))
        .or_else(|| song.get("cover"))
        .or_else(|| song.get("image"))
        .map(json_value_to_string)
        .map(normalize_cover_url)
        .filter(|value| !value.is_empty());
    let cover_url = match cover_url {
        Some(value) => Some(value),
        None => fetch_kuwo_cover(client, song_id).await,
    };

    let lyrics = data
        .get("lrclist")
        .or_else(|| data.get("lrcList"))
        .or_else(|| data.get("lyrics"))
        .and_then(|value| {
            if let Some(text) = value.as_str() {
                return Some(text.to_string());
            }
            value.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let text = item
                            .get("lineLyric")
                            .or_else(|| item.get("lyric"))
                            .or_else(|| item.get("text"))
                            .and_then(|value| value.as_str())?
                            .trim();
                        if text.is_empty() {
                            return None;
                        }
                        let time = item
                            .get("time")
                            .or_else(|| item.get("lineTime"))
                            .or_else(|| item.get("startTime"))
                            .map(json_value_to_string)
                            .unwrap_or_default();
                        if time.contains(':') {
                            Some(format!("[{}]{}", time.trim_matches(['[', ']']), text))
                        } else if let Ok(seconds) = time.parse::<f64>() {
                            let minutes = (seconds / 60.0).floor() as u32;
                            let secs = seconds - (minutes as f64 * 60.0);
                            Some(format!("[{:02}:{:05.2}]{}", minutes, secs, text))
                        } else {
                            Some(text.to_string())
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        })
        .or_else(|| {
            data.get("lrc")
                .or_else(|| data.get("lyric"))
                .or_else(|| data.get("lyricText"))
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
        .filter(|value| !value.is_empty());

    Some(SongInfoResult {
        id: song_id.to_string(),
        title,
        artist: song
            .get("artist")
            .or_else(|| song.get("singer"))
            .or_else(|| song.get("artistName"))
            .map(json_value_to_string)
            .unwrap_or_default(),
        album: song
            .get("album")
            .or_else(|| song.get("albumName"))
            .or_else(|| song.get("albumname"))
            .map(json_value_to_string)
            .filter(|value| !value.is_empty()),
        cover_url,
        lyrics,
        duration: None,
        platform: "kw".to_string(),
    })
}

async fn fetch_xiaowo_rankings(
    client: &reqwest::Client,
    source_id: usize,
    source_name: &str,
) -> Vec<RankingCategory> {
    let response = client
        .get("http://wapi.kuwo.cn/api/pc/bang/list")
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    let Ok(response) = response else {
        return Vec::new();
    };
    let Ok(json) = response.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(groups) = json.get("child").and_then(|value| value.as_array()) {
        for group in groups {
            if let Some(items) = group.get("child").and_then(|value| value.as_array()) {
                for item in items {
                    let id = item
                        .get("sourceid")
                        .or_else(|| item.get("id"))
                        .map(json_value_to_string)
                        .unwrap_or_default();
                    let name = item
                        .get("name")
                        .or_else(|| item.get("disname"))
                        .map(json_value_to_string)
                        .unwrap_or_default();
                    if !id.is_empty() && !name.is_empty() {
                        out.push(RankingCategory {
                            id,
                            name,
                            source_id,
                            source_name: source_name.to_string(),
                        });
                    }
                }
            }
        }
    }
    out
}

async fn fetch_xiaowo_ranking_songs(
    client: &reqwest::Client,
    source_id: usize,
    ranking_id: &str,
) -> Vec<SongDetail> {
    let response = client
        .get("http://kbangserver.kuwo.cn/ksong.s")
        .query(&[
            ("from", "pc"),
            ("fmt", "json"),
            ("pn", "0"),
            ("rn", "80"),
            ("type", "bang"),
            ("data", "content"),
            ("id", ranking_id),
            ("show_copyright_off", "0"),
            ("pcmp4", "1"),
            ("isbang", "1"),
            ("userid", "0"),
            ("httpStatus", "1"),
        ])
        .timeout(Duration::from_secs(6))
        .send()
        .await;
    let Ok(response) = response else {
        return Vec::new();
    };
    let Ok(json) = response.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    json.get("musiclist")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| xiaowo_song_detail_from_item(client, item, source_id))
                .collect()
        })
        .unwrap_or_default()
}

async fn fetch_xiaowo_playlists(
    client: &reqwest::Client,
    source_id: usize,
    source_name: &str,
) -> Vec<PlaylistInfo> {
    let response = client
        .get("https://wapi.kuwo.cn/api/pc/classify/playlist/getRcmPlayList")
        .query(&[
            ("loginUid", "0"),
            ("loginSid", "0"),
            ("appUid", "76039576"),
            ("pn", "0"),
            ("rn", "30"),
            ("order", "hot"),
        ])
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    let Ok(response) = response else {
        return Vec::new();
    };
    let Ok(json) = response.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    let items = json
        .get("data")
        .and_then(|value| value.get("data"))
        .or_else(|| json.get("data"))
        .and_then(|value| value.as_array());
    items
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let id = item.get("id").map(json_value_to_string).unwrap_or_default();
                    let name = item
                        .get("name")
                        .or_else(|| item.get("title"))
                        .map(json_value_to_string)
                        .unwrap_or_default();
                    if id.is_empty() || name.is_empty() {
                        return None;
                    }
                    Some(PlaylistInfo {
                        id,
                        name,
                        cover: item
                            .get("img")
                            .or_else(|| item.get("pic"))
                            .or_else(|| item.get("cover"))
                            .map(json_value_to_string)
                            .map(normalize_cover_url)
                            .filter(|value| !value.is_empty()),
                        song_count: None,
                        source_id,
                        source_name: source_name.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn fetch_xiaowo_playlist_songs(
    client: &reqwest::Client,
    source_id: usize,
    playlist_id: &str,
) -> Vec<SongDetail> {
    let response = client
        .get("http://nplserver.kuwo.cn/pl.svc")
        .query(&[
            ("op", "getlistinfo"),
            ("pid", playlist_id),
            ("pn", "0"),
            ("rn", "100"),
            ("encode", "utf8"),
            ("keyset", "pl2012"),
            ("vipver", "MUSIC_9.1.1.2_BCS2"),
            ("newver", "1"),
        ])
        .timeout(Duration::from_secs(6))
        .send()
        .await;
    let Ok(response) = response else {
        return Vec::new();
    };
    let Ok(json) = response.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    json.get("musiclist")
        .or_else(|| json.get("songs"))
        .or_else(|| json.get("data"))
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| xiaowo_song_detail_from_item(client, item, source_id))
                .collect()
        })
        .unwrap_or_default()
}

fn xiaowo_song_detail_from_item(
    _client: &reqwest::Client,
    item: &serde_json::Value,
    source_id: usize,
) -> Option<SongDetail> {
    let id = item
        .get("id")
        .or_else(|| item.get("rid"))
        .or_else(|| item.get("musicrid"))
        .map(json_value_to_string)
        .unwrap_or_default()
        .trim_start_matches("MUSIC_")
        .to_string();
    let title = item
        .get("name")
        .or_else(|| item.get("songName"))
        .or_else(|| item.get("songname"))
        .or_else(|| item.get("title"))
        .map(json_value_to_string)
        .unwrap_or_default();
    if id.is_empty() || title.is_empty() {
        return None;
    }
    Some(SongDetail {
        id,
        title,
        artist: item
            .get("artist")
            .or_else(|| item.get("singer"))
            .or_else(|| item.get("artistName"))
            .map(json_value_to_string)
            .unwrap_or_default(),
        album: item
            .get("album")
            .or_else(|| item.get("albumName"))
            .or_else(|| item.get("albumname"))
            .map(json_value_to_string)
            .filter(|value| !value.is_empty()),
        album_id: item
            .get("albumid")
            .or_else(|| item.get("albumId"))
            .map(json_value_to_string)
            .filter(|value| !value.is_empty()),
        cover_url: extract_cover_url(item),
        duration: item
            .get("duration")
            .or_else(|| item.get("songTimeMinutes"))
            .and_then(json_value_to_f64)
            .map(|value| if value > 10000.0 { value / 1000.0 } else { value }),
        source_id,
        platform: "kw".to_string(),
    })
}

async fn fetch_kuwo_cover(client: &reqwest::Client, song_id: &str) -> Option<String> {
    let response = client
        .get("http://artistpicserver.kuwo.cn/pic.web")
        .query(&[
            ("type", "rid_pic"),
            ("rid", song_id),
            ("size", "500"),
            ("ct", "music"),
        ])
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?;
    let json = response.json::<serde_json::Value>().await.ok()?;
    json.get("url")
        .or_else(|| json.get("pic"))
        .or_else(|| json.get("data").and_then(|data| data.get("url")))
        .map(json_value_to_string)
        .map(normalize_cover_url)
        .filter(|value| !value.is_empty())
}

fn normalize_cover_url(value: String) -> String {
    let value = value.trim().to_string();
    if value.starts_with("//") {
        format!("https:{value}")
    } else if value.starts_with("http://") {
        value.replacen("http://", "https://", 1)
    } else {
        value
    }
}

fn extract_song_items(json: &serde_json::Value) -> Vec<serde_json::Value> {
    if let Some(array) = json.as_array() {
        return array.clone();
    }

    let direct = json
        .get("data")
        .or_else(|| json.get("songs"))
        .or_else(|| json.get("results"))
        .or_else(|| json.get("result"))
        .or_else(|| json.get("list"));

    if let Some(array) = direct.and_then(|value| value.as_array()) {
        return array.clone();
    }

    if let Some(object) = direct {
        for key in ["list", "songs", "results", "result", "data"] {
            if let Some(array) = object.get(key).and_then(|value| value.as_array()) {
                return array.clone();
            }
        }
    }

    Vec::new()
}

fn extract_song_id(item: &serde_json::Value) -> String {
    item.get("id")
        .or_else(|| item.get("song_id"))
        .or_else(|| item.get("rid"))
        .or_else(|| item.get("songmid"))
        .or_else(|| item.get("mid"))
        .map(json_value_to_string)
        .unwrap_or_default()
}

fn extract_song_title(item: &serde_json::Value) -> String {
    item.get("name")
        .or_else(|| item.get("title"))
        .or_else(|| item.get("songname"))
        .map(json_value_to_string)
        .unwrap_or_default()
}

fn extract_song_artist(item: &serde_json::Value) -> String {
    for key in ["singer", "artist", "author", "artists"] {
        if let Some(value) = item.get(key) {
            let text = json_value_to_string(value);
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

fn extract_cover_url(item: &serde_json::Value) -> Option<String> {
    for key in [
        "cover",
        "pic",
        "image",
        "img",
        "albumPic",
        "albumpic",
        "pic120",
        "pic300",
        "web_albumpic_short",
        "WEB_ALBUMPIC_SHORT",
        "ALBUM_PIC",
    ] {
        if let Some(value) = item.get(key) {
            let text = normalize_cover_url(json_value_to_string(value));
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn extract_quality_label(item: &serde_json::Value) -> Option<String> {
    for key in ["quality", "br", "bitrate", "rate"] {
        if let Some(value) = item.get(key) {
            let text = json_value_to_string(value);
            if !text.is_empty() {
                if let Ok(rate) = text.parse::<u32>() {
                    return Some(format_bitrate(rate));
                }
                return Some(text);
            }
        }
    }

    let best = item
        .get("relate_goods")
        .and_then(|value| value.as_array())
        .and_then(|items| {
            items
                .iter()
                .filter_map(|item| item.get("bitrate").and_then(json_value_to_f64))
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        });
    best.map(|rate| format_bitrate(rate as u32))
}

fn format_bitrate(rate: u32) -> String {
    if rate >= 1000 {
        "Lossless".to_string()
    } else if rate > 0 {
        format!("{rate}k")
    } else {
        String::new()
    }
}

fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(v) => v.clone(),
        serde_json::Value::Number(v) => v.to_string(),
        serde_json::Value::Array(values) => values
            .iter()
            .map(json_value_to_string)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        serde_json::Value::Object(map) => {
            for key in ["name", "title", "artist", "singer"] {
                if let Some(value) = map.get(key) {
                    let text = json_value_to_string(value);
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

fn json_value_to_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_u64().map(|v| v as f64))
        .or_else(|| value.as_i64().map(|v| v as f64))
        .or_else(|| value.as_str().and_then(|v| v.parse::<f64>().ok()))
}

fn normalize_duration_secs(duration: f64) -> f64 {
    if duration > 10_000.0 {
        duration / 1000.0
    } else {
        duration
    }
}

/// Fetch hot keywords via LxMusic API Server format: GET {API_URL}/api/v1/hot?source=xxx
async fn hot_via_lxmusic(client: &reqwest::Client, meta: &SourceMeta) -> Vec<String> {
    let mut items = Vec::new();
    let base = meta.api_url.trim_end_matches('/');

    // Only use the first platform for speed
    let platform = meta
        .platforms
        .first()
        .cloned()
        .unwrap_or_else(|| "kw".to_string());

    println!(
        "Trying to fetch hot keywords from {} with platform {}",
        base, platform
    );

    // Try GET with /api/v1 first
    let url = format!("{}/api/v1/hot", base);
    if let Ok(r) = client
        .get(&url)
        .query(&[("source", platform.as_str())])
        .timeout(std::time::Duration::from_secs(4))
        .send()
        .await
    {
        if let Ok(json) = r.json::<serde_json::Value>().await {
            let list = json
                .get("data")
                .or_else(|| json.get("hots"))
                .or_else(|| json.get("result"))
                .and_then(|v| v.as_array());
            if let Some(arr) = list {
                println!("Got {} hot items from /api/v1/hot GET", arr.len());
                for item in arr {
                    let keyword = item
                        .get("keyword")
                        .or_else(|| item.get("name"))
                        .or_else(|| item.get("title"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !keyword.is_empty() {
                        items.push(keyword.to_string());
                    }
                    if items.len() >= 10 {
                        return items;
                    }
                }
            }
        }
    }

    // Try POST fallback if GET returned nothing
    if items.is_empty() {
        let body = serde_json::json!({
            "source": &platform,
            "key": meta.api_key,
        });
        let resp = client
            .post(format!("{}/hot", base))
            .json(&body)
            .timeout(std::time::Duration::from_secs(4))
            .send()
            .await;

        if let Ok(r) = resp {
            if let Ok(json) = r.json::<serde_json::Value>().await {
                let list = json
                    .get("data")
                    .or_else(|| json.get("hots"))
                    .or_else(|| json.get("result"))
                    .and_then(|v| v.as_array());
                if let Some(arr) = list {
                    for item in arr {
                        let keyword = item
                            .get("keyword")
                            .or_else(|| item.get("name"))
                            .or_else(|| item.get("title"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !keyword.is_empty() {
                            items.push(keyword.to_string());
                        }
                        if items.len() >= 10 {
                            break;
                        }
                    }
                }
            }
        }
    }

    println!("Returning {} hot items", items.len());
    items
}

// ─── Rankings ───

/// Fetch ranking categories via LxMusic API Server: GET {API_URL}/api/v1/top?source=xxx
async fn fetch_rankings(
    client: &reqwest::Client,
    meta: &SourceMeta,
    source_id: usize,
) -> Vec<RankingCategory> {
    let mut results = Vec::new();
    let base = meta.api_url.trim_end_matches('/');
    let platform = meta
        .platforms
        .first()
        .cloned()
        .unwrap_or_else(|| "kw".to_string());

    // Try GET with /api/v1/top first
    let url = format!("{}/api/v1/top", base);
    if let Ok(r) = client
        .get(&url)
        .query(&[("source", platform.as_str())])
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        if let Ok(json) = r.json::<serde_json::Value>().await {
            let list = json
                .get("data")
                .or_else(|| json.get("result"))
                .and_then(|v| v.as_array());
            if let Some(arr) = list {
                for item in arr {
                    let id = item
                        .get("id")
                        .or_else(|| item.get("top_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let name = item
                        .get("name")
                        .or_else(|| item.get("title"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !id.is_empty() && !name.is_empty() {
                        results.push(RankingCategory {
                            id: id.to_string(),
                            name: name.to_string(),
                            source_id,
                            source_name: meta.name.clone(),
                        });
                    }
                }
            }
        }
    }

    // Try POST fallback if GET returned nothing
    if results.is_empty() {
        let body = serde_json::json!({
            "source": &platform,
            "key": meta.api_key,
        });

        if let Ok(r) = client
            .post(format!("{}/top", base))
            .json(&body)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            if let Ok(json) = r.json::<serde_json::Value>().await {
                let list = json
                    .get("data")
                    .or_else(|| json.get("result"))
                    .and_then(|v| v.as_array());
                if let Some(arr) = list {
                    for item in arr {
                        let id = item
                            .get("id")
                            .or_else(|| item.get("top_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let name = item
                            .get("name")
                            .or_else(|| item.get("title"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !id.is_empty() && !name.is_empty() {
                            results.push(RankingCategory {
                                id: id.to_string(),
                                name: name.to_string(),
                                source_id,
                                source_name: meta.name.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    results
}

/// Fetch songs in a ranking: GET {API_URL}/api/v1/top/songs?source=xxx&top_id=xxx
async fn fetch_ranking_songs(
    client: &reqwest::Client,
    meta: &SourceMeta,
    source_id: usize,
    ranking_id: &str,
) -> Vec<SongDetail> {
    let base = meta.api_url.trim_end_matches('/');
    let platform = meta
        .platforms
        .first()
        .cloned()
        .unwrap_or_else(|| "kw".to_string());
    let mut results = Vec::new();

    // Try GET with /api/v1/top/songs first
    let url = format!("{}/api/v1/top/songs", base);
    if let Ok(r) = client
        .get(&url)
        .query(&[("source", platform.as_str()), ("top_id", ranking_id)])
        .timeout(std::time::Duration::from_secs(6))
        .send()
        .await
    {
        if let Ok(json) = r.json::<serde_json::Value>().await {
            let songs = json
                .get("data")
                .or_else(|| json.get("songs"))
                .or_else(|| json.get("result"))
                .and_then(|v| v.as_array());
            if let Some(arr) = songs {
                for item in arr {
                    let id = item
                        .get("id")
                        .or_else(|| item.get("song_id"))
                        .or_else(|| item.get("rid"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let title = item
                        .get("name")
                        .or_else(|| item.get("title"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let artist = item
                        .get("singer")
                        .or_else(|| item.get("artist"))
                        .or_else(|| item.get("author"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !id.is_empty() && !title.is_empty() {
                        let album = item
                            .get("album")
                            .or_else(|| item.get("album_name"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let duration = item
                            .get("duration")
                            .or_else(|| item.get("dt"))
                            .and_then(|v| v.as_f64());
                        let cover = item
                            .get("cover")
                            .or_else(|| item.get("pic"))
                            .or_else(|| item.get("image"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        results.push(SongDetail {
                            id,
                            title,
                            artist,
                            album,
                            album_id: None,
                            cover_url: cover,
                            duration: duration.map(|d| d / 1000.0), // ms → sec
                            source_id,
                            platform: platform.clone(),
                        });
                    }
                }
            }
        }
    }

    // Try POST fallback if GET returned nothing
    if results.is_empty() {
        let body = serde_json::json!({
            "source": &platform,
            "top_id": ranking_id,
            "key": meta.api_key,
            "limit": 30,
        });

        if let Ok(r) = client
            .post(format!("{}/top/songs", base))
            .json(&body)
            .timeout(std::time::Duration::from_secs(6))
            .send()
            .await
        {
            if let Ok(json) = r.json::<serde_json::Value>().await {
                let songs = json
                    .get("data")
                    .or_else(|| json.get("songs"))
                    .or_else(|| json.get("result"))
                    .and_then(|v| v.as_array());
                if let Some(arr) = songs {
                    for item in arr {
                        let id = item
                            .get("id")
                            .or_else(|| item.get("song_id"))
                            .or_else(|| item.get("rid"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let title = item
                            .get("name")
                            .or_else(|| item.get("title"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let artist = item
                            .get("singer")
                            .or_else(|| item.get("artist"))
                            .or_else(|| item.get("author"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !id.is_empty() && !title.is_empty() {
                            let album = item
                                .get("album")
                                .or_else(|| item.get("album_name"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let duration = item
                                .get("duration")
                                .or_else(|| item.get("dt"))
                                .and_then(|v| v.as_f64());
                            let cover = item
                                .get("cover")
                                .or_else(|| item.get("pic"))
                                .or_else(|| item.get("image"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            results.push(SongDetail {
                                id,
                                title,
                                artist,
                                album,
                                album_id: None,
                                cover_url: cover,
                                duration: duration.map(|d| d / 1000.0), // ms → sec
                                source_id,
                                platform: platform.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    results
}

// ─── Playlists ───

/// Fetch playlists: GET {API_URL}/api/v1/playlist?source=xxx
async fn fetch_playlists(
    client: &reqwest::Client,
    meta: &SourceMeta,
    source_id: usize,
) -> Vec<PlaylistInfo> {
    let base = meta.api_url.trim_end_matches('/');
    let platform = meta
        .platforms
        .first()
        .cloned()
        .unwrap_or_else(|| "kw".to_string());
    let mut results = Vec::new();

    // Try GET with /api/v1/playlist first
    let url = format!("{}/api/v1/playlist", base);
    if let Ok(r) = client
        .get(&url)
        .query(&[("source", platform.as_str())])
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        if let Ok(json) = r.json::<serde_json::Value>().await {
            let list = json
                .get("data")
                .or_else(|| json.get("playlists"))
                .or_else(|| json.get("result"))
                .and_then(|v| v.as_array());
            if let Some(arr) = list {
                for item in arr {
                    let id = item
                        .get("id")
                        .or_else(|| item.get("playlist_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .or_else(|| item.get("title"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !id.is_empty() && !name.is_empty() {
                        let count = item
                            .get("song_count")
                            .or_else(|| item.get("count"))
                            .or_else(|| item.get("total"))
                            .and_then(|v| v.as_u64())
                            .map(|v| v as usize);
                        let cover = item
                            .get("cover")
                            .or_else(|| item.get("pic"))
                            .or_else(|| item.get("image"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        results.push(PlaylistInfo {
                            id,
                            name,
                            cover,
                            song_count: count,
                            source_id,
                            source_name: meta.name.clone(),
                        });
                    }
                }
            }
        }
    }

    // Try POST fallback if GET returned nothing
    if results.is_empty() {
        let body = serde_json::json!({
            "source": &platform,
            "key": meta.api_key,
            "limit": 30,
        });

        if let Ok(r) = client
            .post(format!("{}/playlist", base))
            .json(&body)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            if let Ok(json) = r.json::<serde_json::Value>().await {
                let list = json
                    .get("data")
                    .or_else(|| json.get("playlists"))
                    .or_else(|| json.get("result"))
                    .and_then(|v| v.as_array());
                if let Some(arr) = list {
                    for item in arr {
                        let id = item
                            .get("id")
                            .or_else(|| item.get("playlist_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = item
                            .get("name")
                            .or_else(|| item.get("title"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !id.is_empty() && !name.is_empty() {
                            let count = item
                                .get("song_count")
                                .or_else(|| item.get("count"))
                                .or_else(|| item.get("total"))
                                .and_then(|v| v.as_u64())
                                .map(|v| v as usize);
                            let cover = item
                                .get("cover")
                                .or_else(|| item.get("pic"))
                                .or_else(|| item.get("image"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            results.push(PlaylistInfo {
                                id,
                                name,
                                cover,
                                song_count: count,
                                source_id,
                                source_name: meta.name.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    results
}

/// Fetch songs in a playlist: GET {API_URL}/api/v1/playlist/songs?source=xxx&playlist_id=xxx
async fn fetch_playlist_songs(
    client: &reqwest::Client,
    meta: &SourceMeta,
    source_id: usize,
    playlist_id: &str,
) -> Vec<SongDetail> {
    let base = meta.api_url.trim_end_matches('/');
    let platform = meta
        .platforms
        .first()
        .cloned()
        .unwrap_or_else(|| "kw".to_string());
    let mut results = Vec::new();

    // Try GET with /api/v1/playlist/songs first
    let url = format!("{}/api/v1/playlist/songs", base);
    if let Ok(r) = client
        .get(&url)
        .query(&[("source", platform.as_str()), ("playlist_id", playlist_id)])
        .timeout(std::time::Duration::from_secs(6))
        .send()
        .await
    {
        if let Ok(json) = r.json::<serde_json::Value>().await {
            let songs = json
                .get("data")
                .or_else(|| json.get("songs"))
                .or_else(|| json.get("result"))
                .and_then(|v| v.as_array());
            if let Some(arr) = songs {
                for item in arr {
                    let id = item
                        .get("id")
                        .or_else(|| item.get("song_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let title = item
                        .get("name")
                        .or_else(|| item.get("title"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let artist = item
                        .get("singer")
                        .or_else(|| item.get("artist"))
                        .or_else(|| item.get("author"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !id.is_empty() && !title.is_empty() {
                        let album = item
                            .get("album")
                            .or_else(|| item.get("album_name"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let duration = item
                            .get("duration")
                            .or_else(|| item.get("dt"))
                            .and_then(|v| v.as_f64());
                        let cover = item
                            .get("cover")
                            .or_else(|| item.get("pic"))
                            .or_else(|| item.get("image"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        results.push(SongDetail {
                            id,
                            title,
                            artist,
                            album,
                            album_id: None,
                            cover_url: cover,
                            duration: duration.map(|d| d / 1000.0),
                            source_id,
                            platform: platform.clone(),
                        });
                    }
                }
            }
        }
    }

    // Try POST fallback if GET returned nothing
    if results.is_empty() {
        let body = serde_json::json!({
            "source": &platform,
            "playlist_id": playlist_id,
            "key": meta.api_key,
            "limit": 50,
        });

        if let Ok(r) = client
            .post(format!("{}/playlist/songs", base))
            .json(&body)
            .timeout(std::time::Duration::from_secs(6))
            .send()
            .await
        {
            if let Ok(json) = r.json::<serde_json::Value>().await {
                let songs = json
                    .get("data")
                    .or_else(|| json.get("songs"))
                    .or_else(|| json.get("result"))
                    .and_then(|v| v.as_array());
                if let Some(arr) = songs {
                    for item in arr {
                        let id = item
                            .get("id")
                            .or_else(|| item.get("song_id"))
                            .or_else(|| item.get("rid"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let title = item
                            .get("name")
                            .or_else(|| item.get("title"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let artist = item
                            .get("singer")
                            .or_else(|| item.get("artist"))
                            .or_else(|| item.get("author"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !id.is_empty() && !title.is_empty() {
                            let album = item
                                .get("album")
                                .or_else(|| item.get("album_name"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let duration = item
                                .get("duration")
                                .or_else(|| item.get("dt"))
                                .and_then(|v| v.as_f64());
                            let cover = item
                                .get("cover")
                                .or_else(|| item.get("pic"))
                                .or_else(|| item.get("image"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            results.push(SongDetail {
                                id,
                                title,
                                artist,
                                album,
                                album_id: None,
                                cover_url: cover,
                                duration: duration.map(|d| d / 1000.0),
                                source_id,
                                platform: platform.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    results
}

// ─── Song URL ───

/// Fetch playable URL for a song: POST {API_URL}/song or POST {API_URL}/url
async fn fetch_song_url(
    client: &reqwest::Client,
    meta: &SourceMeta,
    platform: &str,
    song_id: &str,
) -> Option<SongUrlResult> {
    let base = meta.api_url.trim_end_matches('/');
    let quality = preferred_quality_for_platform(meta, platform);

    // Try POST /url first (standard LxMusic endpoint)
    let body = serde_json::json!({
        "source": platform,
        "id": song_id,
        "quality": quality,
        "key": meta.api_key,
    });

    // Try /url endpoint
    if let Ok(r) = client
        .post(format!("{}/url", base))
        .json(&body)
        .timeout(std::time::Duration::from_secs(6))
        .send()
        .await
    {
        if let Ok(json) = r.json::<serde_json::Value>().await {
            if let Some(song) = parse_song_url_result(&json, quality) {
                return Some(song);
            }
        }
    }

    // Try /song endpoint (some sources use this for URL too)
    if let Ok(r) = client
        .post(format!("{}/song", base))
        .json(&body)
        .timeout(std::time::Duration::from_secs(6))
        .send()
        .await
    {
        if let Ok(json) = r.json::<serde_json::Value>().await {
            if let Some(song) = parse_song_url_result(&json, quality) {
                return Some(song);
            }
        }
    }

    // Try script-compatible GET /url/{source}/{song_id}/{quality}
    let path_url = format!("{}/url/{}/{}/{}", base, platform, song_id, quality);
    let mut path_request = client
        .get(&path_url)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(5));
    if !meta.api_key.is_empty() {
        path_request = path_request.header("X-Request-Key", meta.api_key.as_str());
    }
    if let Ok(r) = path_request.send().await {
        if let Ok(json) = r.json::<serde_json::Value>().await {
            if let Some(song) = parse_song_url_result(&json, quality) {
                return Some(song);
            }
        }
    }

    // Try GET fallback
    let get_url = format!(
        "{}/url?source={}&id={}&quality={}",
        base, platform, song_id, quality
    );
    let mut get_request = client
        .get(&get_url)
        .timeout(std::time::Duration::from_secs(4));
    if !meta.api_key.is_empty() {
        get_request = get_request.header("X-Request-Key", meta.api_key.as_str());
    }
    if let Ok(r) = get_request.send().await {
        if let Ok(json) = r.json::<serde_json::Value>().await {
            if let Some(song) = parse_song_url_result(&json, quality) {
                return Some(song);
            }
        }
    }

    None
}

fn preferred_quality_for_platform(meta: &SourceMeta, platform: &str) -> &'static str {
    if meta.platforms.iter().any(|item| item == platform) {
        "128k"
    } else {
        "128k"
    }
}

fn parse_song_url_result(
    json: &serde_json::Value,
    requested_quality: &str,
) -> Option<SongUrlResult> {
    if let Some(url) = json
        .get("url")
        .or_else(|| json.get("play_url"))
        .and_then(|v| v.as_str())
    {
        if !url.is_empty() {
            return Some(SongUrlResult {
                url: url.to_string(),
                quality: json
                    .get("quality")
                    .and_then(|v| v.as_str())
                    .unwrap_or(requested_quality)
                    .to_string(),
                format: infer_audio_format(url, json.get("format").and_then(|v| v.as_str())),
            });
        }
    }

    let data = json.get("data").or_else(|| json.get("result"))?;

    if let Some(url) = data.as_str() {
        if !url.is_empty() {
            return Some(SongUrlResult {
                url: url.to_string(),
                quality: requested_quality.to_string(),
                format: infer_audio_format(url, None),
            });
        }
    }

    if let Some(object) = data.as_object() {
        if let Some(url) = object
            .get("url")
            .or_else(|| object.get("play_url"))
            .or_else(|| object.get("src"))
            .and_then(|v| v.as_str())
        {
            if !url.is_empty() {
                return Some(SongUrlResult {
                    url: url.to_string(),
                    quality: object
                        .get("quality")
                        .and_then(|v| v.as_str())
                        .unwrap_or(requested_quality)
                        .to_string(),
                    format: infer_audio_format(url, object.get("format").and_then(|v| v.as_str())),
                });
            }
        }
    }

    if let Some(array) = data.as_array() {
        for item in array {
            if let Some(song) = parse_song_url_result(item, requested_quality) {
                return Some(song);
            }
        }
    }

    None
}

fn infer_audio_format(url: &str, explicit: Option<&str>) -> String {
    if let Some(format) = explicit.filter(|value| !value.is_empty()) {
        return format.to_string();
    }

    let clean = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    for extension in ["flac", "m4a", "aac", "ogg", "wav", "mp3"] {
        if clean.ends_with(&format!(".{extension}")) {
            return extension.to_string();
        }
    }
    "mp3".to_string()
}

// ─── Song Info ───

/// Fetch detailed song info including lyrics: POST {API_URL}/song
async fn fetch_song_info(
    client: &reqwest::Client,
    meta: &SourceMeta,
    _source_id: usize,
    platform: &str,
    song_id: &str,
) -> Option<SongInfoResult> {
    let base = meta.api_url.trim_end_matches('/');

    let body = serde_json::json!({
        "source": platform,
        "id": song_id,
        "key": meta.api_key,
    });

    // Try POST /song
    if let Ok(r) = client
        .post(format!("{}/song", base))
        .json(&body)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        if let Ok(json) = r.json::<serde_json::Value>().await {
            let data = json.get("data").or_else(|| json.get("result"));
            if let Some(d) = data {
                let title = d
                    .get("name")
                    .or_else(|| d.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let artist = d
                    .get("singer")
                    .or_else(|| d.get("artist"))
                    .or_else(|| d.get("author"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !title.is_empty() {
                    let album = d
                        .get("album")
                        .or_else(|| d.get("album_name"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let cover = d
                        .get("cover")
                        .or_else(|| d.get("pic"))
                        .or_else(|| d.get("image"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let lyrics = d
                        .get("lyrics")
                        .or_else(|| d.get("lrc"))
                        .or_else(|| d.get("lyric"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let duration = d
                        .get("duration")
                        .or_else(|| d.get("dt"))
                        .and_then(|v| v.as_f64());
                    return Some(SongInfoResult {
                        id: song_id.to_string(),
                        title,
                        artist,
                        album,
                        cover_url: cover,
                        lyrics: lyrics.filter(|l| !l.is_empty()),
                        duration: duration.map(|d| if d > 10000.0 { d / 1000.0 } else { d }),
                        platform: platform.to_string(),
                    });
                }
            }
        }
    }
    None
}

pub async fn parse_kugou_shared_playlist(url: &str) -> Result<SharedPlaylist, String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|err| err.to_string())?;

    let text = client
        .get(url)
        .send()
        .await
        .map_err(|err| format!("分享链接访问失败: {err}"))?
        .text()
        .await
        .map_err(|err| format!("分享内容读取失败: {err}"))?;

    let title = extract_html_title(&text).unwrap_or_else(|| "酷狗分享歌单".to_string());
    let songs = extract_kugou_songs_from_text(&text);
    let note = if songs.is_empty() {
        Some("已收藏分享链接，但当前接口没有返回可解析的歌曲列表。".to_string())
    } else {
        None
    };

    Ok(SharedPlaylist {
        playlist: PlaylistInfo {
            id: url.to_string(),
            name: title,
            cover: extract_first_image(&text),
            song_count: Some(songs.len()),
            source_id: 0,
            source_name: "酷狗分享".to_string(),
        },
        songs,
        external_url: url.to_string(),
        note,
    })
}

fn extract_html_title(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let start = lower.find("<title>")? + "<title>".len();
    let end = lower[start..].find("</title>")? + start;
    let title = decode_basic_html(text[start..end].trim());
    let title = title
        .replace(" - 酷狗音乐", "")
        .replace("_酷狗音乐", "")
        .trim()
        .to_string();
    (!title.is_empty()).then_some(title)
}

fn extract_first_image(text: &str) -> Option<String> {
    for key in ["og:image", "twitter:image"] {
        if let Some(pos) = text.find(key) {
            let tail = &text[pos..text.len().min(pos + 500)];
            if let Some(url) = extract_url_like(tail) {
                return Some(url);
            }
        }
    }
    None
}

fn extract_url_like(text: &str) -> Option<String> {
    let start = text.find("http")?;
    let rest = &text[start..];
    let end = rest
        .find(|ch: char| ch == '"' || ch == '\'' || ch.is_whitespace() || ch == '<')
        .unwrap_or(rest.len());
    Some(decode_basic_html(&rest[..end]))
}

fn decode_basic_html(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn extract_kugou_songs_from_text(text: &str) -> Vec<SongDetail> {
    let mut out = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        collect_song_details(&value, &mut out);
    }
    if let Some(value) = extract_assigned_json(text, "var nData") {
        collect_song_details(&value, &mut out);
    }
    if let Some(value) = extract_assigned_json(text, "nData") {
        collect_song_details(&value, &mut out);
    }
    for value in collect_json_values(text) {
        collect_song_details(&value, &mut out);
        if out.len() >= 300 {
            break;
        }
    }
    dedupe_song_details(out)
}

fn extract_assigned_json(text: &str, marker: &str) -> Option<serde_json::Value> {
    let pos = text.find(marker)?;
    let tail = &text[pos + marker.len()..];
    let eq = tail.find('=')?;
    let tail = &tail[eq + 1..];
    for open in ['{', '['] {
        if let Some(start) = tail.find(open) {
            if let Some(slice) = balanced_json_slice(&tail[start..]) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(slice) {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn collect_json_values(text: &str) -> Vec<serde_json::Value> {
    let mut values = Vec::new();
    for marker in [
        "window.__INITIAL_STATE__=",
        "__NUXT__=",
        "var nData",
        "songs",
        "list",
    ] {
        let Some(pos) = text.find(marker) else {
            continue;
        };
        let tail = &text[pos + marker.len()..];
        for open in ['{', '['] {
            if let Some(start) = tail.find(open) {
                if let Some(slice) = balanced_json_slice(&tail[start..]) {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(slice) {
                        values.push(value);
                    }
                }
            }
        }
    }
    values
}

fn balanced_json_slice(text: &str) -> Option<&str> {
    let mut depth = 0_i32;
    let mut in_str = false;
    let mut escape = false;
    for (idx, ch) in text.char_indices() {
        if in_str {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[..=idx]);
                }
            }
            _ => {}
        }
    }
    None
}

fn collect_song_details(value: &serde_json::Value, out: &mut Vec<SongDetail>) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_song_details(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(song) = song_detail_from_json(value) {
                out.push(song);
            }
            for child in map.values() {
                collect_song_details(child, out);
            }
        }
        _ => {}
    }
}

fn song_detail_from_json(value: &serde_json::Value) -> Option<SongDetail> {
    let title = first_str(value, &["songname", "song_name", "name", "title"])?;
    let artist = first_str(
        value,
        &[
            "singername",
            "singer_name",
            "author_name",
            "artist",
            "singer",
        ],
    )
    .unwrap_or_else(|| "未知歌手".to_string());
    let id = first_str(value, &["hash", "audio_id", "songid", "song_id", "id"])
        .unwrap_or_else(|| format!("kg-share-{}-{}", title, artist));
    if title.len() > 80 || title.contains("http") {
        return None;
    }
    Some(SongDetail {
        id,
        title,
        artist,
        album: first_str(value, &["album_name", "album", "albumname"]),
        album_id: first_str(value, &["album_id", "albumid"]),
        cover_url: first_str(value, &["img", "image", "cover", "pic"]),
        duration: first_f64(value, &["duration", "time_len", "timelen"]).map(|d| {
            if d > 10000.0 {
                d / 1000.0
            } else {
                d
            }
        }),
        source_id: 0,
        platform: "kg".to_string(),
    })
}

fn first_str(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .or_else(|| value.as_i64().map(|v| v.to_string()))
        })
        .filter(|value| !value.trim().is_empty())
}

fn first_f64(value: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
}

fn dedupe_song_details(songs: Vec<SongDetail>) -> Vec<SongDetail> {
    let mut seen = std::collections::HashSet::new();
    songs
        .into_iter()
        .filter(|song| seen.insert(format!("{}::{}", song.title, song.artist)))
        .collect()
}

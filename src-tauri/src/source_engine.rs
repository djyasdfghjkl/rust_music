use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

const MAX_SEARCH_CONCURRENT: usize = 6;

include!(concat!(env!("OUT_DIR"), "/embedded_sources.rs"));

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
    can_search: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeSourceAdapter {
    Xiaowo,
    Kugou,
    Netease,
    Bilibili,
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
    pub cover: Option<String>,
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
    pub fn new(source_dir: PathBuf, audio_cache_dir: PathBuf) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            .connect_timeout(Duration::from_secs(2))
            .danger_accept_invalid_certs(true)
            .user_agent("MikuTunes/1.0")
            .build()
            .unwrap_or_default();

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

        println!(
            "[scan_sources] EMBEDDED_SOURCES has {} entries",
            EMBEDDED_SOURCES.len()
        );

        for (file_name, content) in EMBEDDED_SOURCES.iter().copied() {
            let stem = std::path::Path::new(file_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let (name, meta) = parse_source_content(content, &stem);
            scanned.push((stem, name, meta));
        }

        println!("[scan_sources] After embedded: {} entries", scanned.len());

        if scanned.is_empty() {
            println!(
                "[scan_sources] EMBEDDED_SOURCES empty, falling back to physical dir: {:?}",
                self.source_dir
            );
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
                println!("[scan_sources] Physical dir: {} entries", scanned.len());
            } else {
                println!("[scan_sources] Failed to read physical dir");
            }
        }

        if !scanned
            .iter()
            .any(|(file_name, _, _)| file_name.eq_ignore_ascii_case("xiaowo"))
        {
            let (_, meta) = parse_source_content("", "xiaowo");
            scanned.insert(0, ("xiaowo".to_string(), "xiaowo".to_string(), meta));
            println!("[scan_sources] xiaowo adapter inserted at top");
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

        println!(
            "[scan_sources] Final: {} sources, {} with meta (api)",
            sources.len(),
            meta_map.len()
        );

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
        let mut all_results = Vec::new();
        let batch_index = offset / 100; // 0=initial, 1=first load_more, 2=second, ...

        for source in sources.iter() {
            let Some(meta) = meta_map.get(&source.id).cloned() else {
                continue;
            };
            let adapter = match &meta.adapter {
                Some(a) => a,
                _ => continue,
            };
            let results: Vec<SongResult> = match adapter {
                NativeSourceAdapter::Xiaowo => {
                    search_via_xiaowo_pages(&client, &meta, keyword, batch_index * 3, 3).await
                }
                NativeSourceAdapter::Kugou => {
                    let page_start = batch_index * 3 + 1; // pages 4,7,10,...
                    let mut r = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for page in page_start..page_start + 3 {
                        let Ok(resp) = client
                            .get("http://mobilecdn.kugou.com/api/v3/search/song")
                            .query(&[
                                ("format", "json"),
                                ("keyword", keyword),
                                ("page", &page.to_string()),
                                ("pagesize", "100"),
                                ("showtype", "1"),
                            ])
                            .timeout(Duration::from_secs(4))
                            .send()
                            .await
                        else {
                            continue;
                        };
                        let Ok(json) = resp.json::<serde_json::Value>().await else {
                            continue;
                        };
                        let Some(songs) = json
                            .get("data")
                            .and_then(|d| d.get("info"))
                            .and_then(|v| v.as_array())
                        else {
                            break;
                        };
                        let mut has_data = false;
                        for item in songs {
                            let hash = item
                                .get("hash")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if hash.is_empty() || !seen.insert(hash.clone()) {
                                continue;
                            }
                            has_data = true;
                            let title = item
                                .get("songname")
                                .or_else(|| item.get("filename"))
                                .map(json_value_to_string)
                                .unwrap_or_default();
                            let artist = item
                                .get("singername")
                                .or_else(|| item.get("singer"))
                                .map(json_value_to_string)
                                .unwrap_or_default();
                            let album = item
                                .get("album_name")
                                .map(json_value_to_string)
                                .filter(|v| !v.is_empty());
                            let duration = item
                                .get("duration")
                                .or_else(|| item.get("second"))
                                .and_then(|v| v.as_f64())
                                .map(|s| s.max(30.0));
                            let cover_url = item
                                .get("img")
                                .or_else(|| item.get("album_img"))
                                .or_else(|| item.get("cover"))
                                .map(json_value_to_string)
                                .filter(|v| !v.is_empty());
                            let quality = kugou_quality_from_item(item);
                            r.push(SongResult {
                                id: hash,
                                title,
                                artist,
                                source: "kugou".to_string(),
                                source_id: 0,
                                platform: "kg".to_string(),
                                album,
                                cover_url,
                                duration,
                                score: 0,
                                quality,
                            });
                        }
                        if !has_data {
                            break;
                        }
                    }
                    r
                }
                NativeSourceAdapter::Netease => {
                    let offset_start = batch_index * 300; // offsets 300,600,900,...
                    let mut r = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for off in (offset_start..offset_start + 300).step_by(100) {
                        let Ok(resp) = client
                            .get("http://music.163.com/api/search/get/")
                            .query(&[
                                ("s", keyword),
                                ("type", "1"),
                                ("offset", &off.to_string()),
                                ("limit", "100"),
                            ])
                            .header("Referer", "https://music.163.com/")
                            .header(
                                "User-Agent",
                                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
                            )
                            .timeout(Duration::from_secs(4))
                            .send()
                            .await
                        else {
                            continue;
                        };
                        let Ok(json) = resp.json::<serde_json::Value>().await else {
                            continue;
                        };
                        let Some(songs) = json
                            .get("result")
                            .and_then(|r| r.get("songs"))
                            .and_then(|v| v.as_array())
                        else {
                            break;
                        };
                        let mut has_data = false;
                        for item in songs {
                            let id = item.get("id").map(|v| v.to_string()).unwrap_or_default();
                            if id.is_empty() || !seen.insert(id.clone()) {
                                continue;
                            }
                            has_data = true;
                            let title = item
                                .get("name")
                                .map(json_value_to_string)
                                .unwrap_or_default();
                            let artists: Vec<String> = item
                                .get("artists")
                                .or_else(|| item.get("ar"))
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|a| {
                                            a.get("name").and_then(|n| n.as_str().map(String::from))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            let album = item
                                .get("album")
                                .or_else(|| item.get("al"))
                                .and_then(|a| a.get("name"))
                                .map(json_value_to_string);
                            let cover_url = item
                                .get("album")
                                .or_else(|| item.get("al"))
                                .and_then(|a| {
                                    a.get("picUrl")
                                        .or_else(|| a.get("picurl"))
                                        .or_else(|| a.get("pic"))
                                })
                                .map(json_value_to_string);
                            let duration = item
                                .get("duration")
                                .or_else(|| item.get("dt"))
                                .and_then(|v| v.as_f64())
                                .map(|ms| ms / 1000.0)
                                .filter(|&s| s > 10.0);
                            let quality = netease_quality_from_item(item);
                            r.push(SongResult {
                                id,
                                title,
                                artist: artists.join(" / "),
                                source: "netease".to_string(),
                                source_id: 0,
                                platform: "wy".to_string(),
                                album,
                                cover_url,
                                duration,
                                score: 0,
                                quality,
                            });
                        }
                        if !has_data {
                            break;
                        }
                    }
                    r
                }
                NativeSourceAdapter::Bilibili => {
                    let page_start = batch_index * 3 + 1; // pages 4,7,10,...
                    let mut r = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for page in page_start..page_start + 3 {
                        let Ok(resp) = client
                            .get("https://api.bilibili.com/x/web-interface/search/type")
                            .query(&[
                                ("search_type", "video"),
                                ("keyword", keyword),
                                ("page", &page.to_string()),
                            ])
                            .header("Referer", "https://www.bilibili.com/")
                            .header(
                                "User-Agent",
                                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
                            )
                            .timeout(Duration::from_secs(4))
                            .send()
                            .await
                        else {
                            continue;
                        };
                        let Ok(json) = resp.json::<serde_json::Value>().await else {
                            continue;
                        };
                        let Some(videos) = json
                            .get("data")
                            .and_then(|d| d.get("result"))
                            .and_then(|v| v.as_array())
                        else {
                            break;
                        };
                        let mut has_data = false;
                        for item in videos {
                            let bvid = item
                                .get("bvid")
                                .map(json_value_to_string)
                                .or_else(|| item.get("aid").map(|v| v.to_string()))
                                .unwrap_or_default();
                            if bvid.is_empty() || !seen.insert(bvid.clone()) {
                                continue;
                            }
                            has_data = true;
                            let title = item
                                .get("title")
                                .map(json_value_to_string)
                                .unwrap_or_default();
                            let author = item
                                .get("author")
                                .map(json_value_to_string)
                                .unwrap_or_default();
                            let cover_url = item
                                .get("pic")
                                .or_else(|| item.get("cover"))
                                .map(json_value_to_string);
                            let duration = item.get("duration").and_then(|v| v.as_f64());
                            if !title.is_empty() {
                                r.push(SongResult {
                                    id: bvid,
                                    title,
                                    artist: author,
                                    source: "bilibili".to_string(),
                                    source_id: 0,
                                    platform: "bilibili".to_string(),
                                    album: None,
                                    cover_url,
                                    duration,
                                    score: 0,
                                    quality: None,
                                });
                            }
                        }
                        if !has_data {
                            break;
                        }
                    }
                    r
                }
            };
            for mut item in results {
                item.source_id = source.id;
                item.score = source.score;
                all_results.push(item);
            }
        }

        SearchResponse {
            total: all_results.len(),
            results: all_results,
            from_source: None,
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
                    Some(NativeSourceAdapter::Kugou) => search_via_kugou(&client, &kw).await,
                    Some(NativeSourceAdapter::Netease) => search_via_netease(&client, &kw).await,
                    Some(NativeSourceAdapter::Bilibili) => search_via_bilibili(&client, &kw).await,
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
            .filter(|source| {
                !searched_source_ids.contains(&source.id)
                    && meta_map
                        .get(&source.id)
                        .is_some_and(|meta| meta.adapter.is_some() || meta.can_search)
            })
            .collect::<Vec<_>>();
        remaining_sources.sort_by(|a, b| b.score.cmp(&a.score));

        for batch in remaining_sources.chunks(MAX_SEARCH_CONCURRENT) {
            let mut tasks = FuturesUnordered::new();
            for source in batch.iter().cloned() {
                let Some(meta) = meta_map.get(&source.id).cloned() else {
                    continue;
                };
                if meta.adapter.is_none() && !meta.can_search {
                    println!("Skip non-search source {}", source.name);
                    continue;
                }
                let client = client.clone();
                let kw = kw.clone();
                tasks.push(tokio::spawn(async move {
                    let search_future = async {
                        match meta.adapter {
                            Some(NativeSourceAdapter::Xiaowo) => {
                                search_via_xiaowo(&client, &meta, &kw).await
                            }
                            Some(NativeSourceAdapter::Kugou) => {
                                search_via_kugou(&client, &kw).await
                            }
                            Some(NativeSourceAdapter::Netease) => {
                                search_via_netease(&client, &kw).await
                            }
                            Some(NativeSourceAdapter::Bilibili) => {
                                search_via_bilibili(&client, &kw).await
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

        // Use FuturesUnordered for true racing — process results as they arrive
        let mut tasks = FuturesUnordered::new();
        for (id, meta) in meta_map.iter() {
            if meta.adapter.is_some() || meta.api_url.is_empty() {
                continue;
            }
            let meta = meta.clone();
            let id = *id;
            let client = client.clone();
            tasks.push(tokio::spawn(async move {
                let items = hot_via_lxmusic(&client, &meta).await;
                (id, meta.name.clone(), items)
            }));
        }

        let mut results = Vec::new();
        let target = limit * 2;
        while let Some(result) = tasks.next().await {
            match result {
                Ok((id, name, items)) if !items.is_empty() => {
                    println!("Source {}: found {} hot keywords", name, items.len());
                    for item in items {
                        results.push(HotItem {
                            title: item,
                            source: name.clone(),
                            source_id: id,
                        });
                    }
                }
                Ok(_) => {} // empty results, ignore silently
                Err(e) => {
                    println!("Hot keywords task error: {:?}", e);
                }
            }
            // Return early as soon as we have enough
            if results.len() >= target {
                break;
            }
        }

        if results.is_empty() {
            // Fallback: use xiaowo rankings
            for (id, meta) in meta_map.iter() {
                if !matches!(meta.adapter, Some(NativeSourceAdapter::Xiaowo)) {
                    continue;
                }
                let client = self.http_client.clone();
                if let Ok(rankings) = tokio::time::timeout(
                    Duration::from_secs(3),
                    fetch_xiaowo_rankings(&client, *id, &meta.name),
                )
                .await
                {
                    for ranking in rankings.into_iter().take(2) {
                        if let Ok(songs) = tokio::time::timeout(
                            Duration::from_secs(4),
                            fetch_xiaowo_ranking_songs(&client, *id, &ranking.id),
                        )
                        .await
                        {
                            for song in songs.into_iter().take(limit.saturating_sub(results.len()))
                            {
                                results.push(HotItem {
                                    title: song.title,
                                    source: meta.name.clone(),
                                    source_id: *id,
                                });
                            }
                        }
                        if results.len() >= limit {
                            break;
                        }
                    }
                }
                if results.len() >= limit {
                    break;
                }
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
            _ => vec![],
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
                let fetch = async {
                    match meta.adapter {
                        Some(NativeSourceAdapter::Xiaowo) => {
                            fetch_xiaowo_rankings(&client, source.id, &meta.name).await
                        }
                        _ => vec![],
                    }
                };
                tokio::time::timeout(Duration::from_secs(4), fetch)
                    .await
                    .unwrap_or_default()
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
            _ => vec![],
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
            _ => vec![],
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
                let fetch = async {
                    match meta.adapter {
                        Some(NativeSourceAdapter::Xiaowo) => {
                            fetch_xiaowo_playlists(&client, source.id, &meta.name).await
                        }
                        _ => vec![],
                    }
                };
                tokio::time::timeout(Duration::from_secs(4), fetch)
                    .await
                    .unwrap_or_default()
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
            _ => vec![],
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
            Some(NativeSourceAdapter::Kugou) => fetch_kugou_song_url(&client, song_id).await,
            Some(NativeSourceAdapter::Netease) => fetch_netease_song_url(&client, song_id).await,
            Some(NativeSourceAdapter::Bilibili) => fetch_bilibili_song_url(&client, song_id).await,
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
        let mut result = match meta.adapter {
            Some(NativeSourceAdapter::Xiaowo) => fetch_xiaowo_song_info(&client, song_id).await,
            Some(NativeSourceAdapter::Kugou) => fetch_kugou_song_info(&client, song_id).await,
            Some(NativeSourceAdapter::Netease) => fetch_netease_song_info(&client, song_id).await,
            Some(NativeSourceAdapter::Bilibili) => None,
            None => fetch_song_info(&client, &meta, source_id, platform, song_id).await,
        };
        if platform == "kw"
            && result.as_ref().is_none_or(|info| {
                info.lyrics
                    .as_deref()
                    .is_none_or(|lyrics| lyrics.trim().is_empty())
            })
        {
            if let Some(kuwo_info) = fetch_xiaowo_song_info(&client, song_id).await {
                if let Some(info) = result.as_mut() {
                    if info
                        .lyrics
                        .as_deref()
                        .is_none_or(|lyrics| lyrics.trim().is_empty())
                    {
                        info.lyrics = kuwo_info.lyrics.clone();
                    }
                    if info
                        .cover_url
                        .as_deref()
                        .is_none_or(|cover| cover.trim().is_empty())
                    {
                        info.cover_url = kuwo_info.cover_url.clone();
                    }
                    if info
                        .album
                        .as_deref()
                        .is_none_or(|album| album.trim().is_empty())
                    {
                        info.album = kuwo_info.album.clone();
                    }
                } else {
                    result = Some(kuwo_info);
                }
            }
        }
        result
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

    parse_source_content(&content, file_name)
}

fn parse_source_content(content: &str, file_name: &str) -> (String, Option<SourceMeta>) {
    let name = extract_source_name(content).unwrap_or_else(|| file_name.to_string());
    let api_url = extract_js_var(content, "API_URL").map(|s| s.trim_matches('"').to_string());
    let api_key = extract_js_var(content, "API_KEY").map(|s| s.trim_matches('"').to_string());
    let quality_str = extract_js_var(content, "MUSIC_QUALITY");

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
                can_search: true,
            }),
        );
    }

    // Native adapters for other platforms (detected by file_name prefix)
    let native_adapter = detect_native_adapter(file_name);
    if let Some(adapter) = native_adapter {
        let plat = match adapter {
            NativeSourceAdapter::Kugou => vec!["kg".to_string()],
            NativeSourceAdapter::Netease => vec!["wy".to_string()],
            NativeSourceAdapter::Bilibili => vec!["bilibili".to_string()],
            _ => vec!["kw".to_string()],
        };
        println!("Parsed source '{}': native adapter={:?}", name, adapter);
        return (
            name.clone(),
            Some(SourceMeta {
                name,
                api_url: String::new(),
                api_key: String::new(),
                platforms: plat,
                adapter: Some(adapter),
                can_search: true,
            }),
        );
    }

    let can_search = source_supports_search(content);
    let meta = api_url.map(|url| {
        println!(
            "Parsed source '{}': API_URL={}, platforms={:?}, can_search={}",
            name, url, platforms, can_search
        );
        SourceMeta {
            name: name.clone(),
            api_url: url,
            api_key: api_key.unwrap_or_default(),
            platforms,
            adapter: None,
            can_search,
        }
    });

    (name, meta)
}

fn detect_native_adapter(file_name: &str) -> Option<NativeSourceAdapter> {
    let lower = file_name.to_ascii_lowercase();
    if lower.contains("kugou") || lower.contains("kg_") || lower == "kg.js" {
        Some(NativeSourceAdapter::Kugou)
    } else if lower.contains("netease")
        || lower.contains("wangyi")
        || lower.contains("wy_")
        || lower == "wy.js"
    {
        Some(NativeSourceAdapter::Netease)
    } else if lower.contains("bilibili") || lower.contains("bili") {
        Some(NativeSourceAdapter::Bilibili)
    } else {
        None
    }
}

fn source_supports_search(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("/api/v1/search")
        || lower.contains("case 'search'")
        || lower.contains("case \"search\"")
        || lower.contains("case 'musicsearch'")
        || lower.contains("case \"musicsearch\"")
        || lower.contains("searchmusic")
        || lower.contains("musicsearch")
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
    client: &reqwest::Client,
    cache_dir: &std::path::Path,
    mut song: SongUrlResult,
) -> Option<SongUrlResult> {
    if song.url.is_empty() {
        return None;
    }
    if song.url.starts_with("data:") {
        return Some(song);
    }
    if !song.url.starts_with("http://") && !song.url.starts_with("https://") {
        return Some(song);
    }

    if let Some(cached_path) = cache_remote_audio(client, cache_dir, &song.url, &song.format).await {
        song.url = cached_path;
    }

    Some(song)
}

async fn cache_remote_audio(
    client: &reqwest::Client,
    cache_dir: &std::path::Path,
    url: &str,
    format: &str,
) -> Option<String> {
    if fs::create_dir_all(cache_dir).is_err() {
        return None;
    }

    let extension = sanitize_audio_extension(if format.trim().is_empty() {
        infer_audio_format(url, None)
    } else {
        format.trim().to_string()
    });
    let file_path = cache_dir.join(format!("{}.{}", stable_hash(url), extension));

    if file_path.is_file() {
        return Some(file_path.to_string_lossy().to_string());
    }

    let response = match client.get(url).timeout(Duration::from_secs(90)).send().await {
        Ok(response) => match response.error_for_status() {
            Ok(response) => response,
            Err(error) => {
                println!("Audio cache HTTP error for {}: {}", url, error);
                return None;
            }
        },
        Err(error) => {
            println!("Audio cache request failed for {}: {}", url, error);
            return None;
        }
    };

    let temp_path = file_path.with_extension(format!("{}.part", extension));
    let mut file = match tokio::fs::File::create(&temp_path).await {
        Ok(file) => file,
        Err(error) => {
            println!("Audio cache file create failed for {:?}: {}", temp_path, error);
            return None;
        }
    };

    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            println!("Audio cache read failed for {}: {}", url, error);
            let _ = tokio::fs::remove_file(&temp_path).await;
            return None;
        }
    };

    if let Err(error) = file.write_all(&bytes).await {
        println!("Audio cache write failed for {:?}: {}", temp_path, error);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return None;
    }

    if let Err(error) = file.flush().await {
        println!("Audio cache flush failed for {:?}: {}", temp_path, error);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return None;
    }

    if let Err(error) = tokio::fs::rename(&temp_path, &file_path).await {
        println!(
            "Audio cache rename failed from {:?} to {:?}: {}",
            temp_path, file_path, error
        );
        let _ = tokio::fs::remove_file(&temp_path).await;
        return None;
    }

    Some(file_path.to_string_lossy().to_string())
}

fn stable_hash(value: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn sanitize_audio_extension(value: String) -> String {
    let sanitized = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    if sanitized.is_empty() {
        "mp3".to_string()
    } else {
        sanitized
    }
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

    let mut lyrics = extract_kuwo_lyrics_from_json(data)
        .or_else(|| extract_kuwo_lyrics_from_json(song))
        .filter(|value| !value.is_empty());
    if lyrics.is_none() {
        lyrics = fetch_kuwo_lyrics(client, song_id).await;
    }

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

async fn fetch_kuwo_lyrics(client: &reqwest::Client, song_id: &str) -> Option<String> {
    for (url, id_key, referer) in [
        (
            "http://www.kuwo.cn/newh5/singles/songinfoandlrc",
            "musicId",
            "http://www.kuwo.cn/",
        ),
        (
            "http://m.kuwo.cn/newh5/singles/songinfoandlrc",
            "mid",
            "http://m.kuwo.cn/",
        ),
        (
            "http://m.kuwo.cn/newh5/singles/songinfoandlrc",
            "musicId",
            "http://m.kuwo.cn/",
        ),
    ] {
        let Ok(response) = client
            .get(url)
            .query(&[(id_key, song_id), ("httpStatus", "1"), ("httpsStatus", "1")])
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .header("Referer", referer)
            .timeout(Duration::from_secs(4))
            .send()
            .await
        else {
            continue;
        };
        if let Ok(json) = response.json::<serde_json::Value>().await {
            if let Some(lyrics) = extract_kuwo_lyrics_from_json(&json) {
                if !lyrics.trim().is_empty() {
                    return Some(lyrics);
                }
            }
        }
    }
    for url in [
        format!(
            "https://www.kuwo.cn/openapi/v1/www/lyric/getlyric?musicId={song_id}&httpsStatus=1"
        ),
        format!("https://m.kuwo.cn/newh5/singles/songinfoandlrc?musicId={song_id}&httpsStatus=1"),
        format!("https://m.kuwo.cn/newh5/singles/songinfoandlrc?mid={song_id}&httpsStatus=1"),
    ] {
        let Ok(response) = client
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .header("Referer", "https://www.kuwo.cn/")
            .timeout(Duration::from_secs(4))
            .send()
            .await
        else {
            continue;
        };
        let Ok(text) = response.text().await else {
            continue;
        };
        if let Some(lyrics) = parse_json_or_jsonp_lyrics(&text) {
            if !lyrics.trim().is_empty() {
                return Some(lyrics);
            }
        }
    }
    None
}

fn parse_json_or_jsonp_lyrics(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let json_text = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        trimmed
    } else {
        let start = trimmed.find(['{', '['])?;
        let end = trimmed.rfind(['}', ']'])?;
        &trimmed[start..=end]
    };
    serde_json::from_str::<serde_json::Value>(json_text)
        .ok()
        .and_then(|value| extract_kuwo_lyrics_from_json(&value))
}

fn extract_kuwo_lyrics_from_json(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        }
        serde_json::Value::Array(items) => {
            let lines = items
                .iter()
                .filter_map(kuwo_lyric_line_from_item)
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                return Some(lines.join("\n"));
            }
            items.iter().find_map(extract_kuwo_lyrics_from_json)
        }
        serde_json::Value::Object(map) => {
            for key in [
                "lrclist",
                "lrcList",
                "lyrics",
                "lrc",
                "lyric",
                "lyricText",
                "content",
            ] {
                if let Some(found) = map.get(key).and_then(extract_kuwo_lyrics_from_json) {
                    return Some(found);
                }
            }
            if let Some(line) = kuwo_lyric_line_from_item(value) {
                return Some(line);
            }
            for key in ["data", "songinfo", "songInfo", "musicInfo", "song"] {
                if let Some(found) = map.get(key).and_then(extract_kuwo_lyrics_from_json) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn kuwo_lyric_line_from_item(value: &serde_json::Value) -> Option<String> {
    let text = value
        .get("lineLyric")
        .or_else(|| value.get("lyric"))
        .or_else(|| value.get("text"))
        .or_else(|| value.get("content"))
        .and_then(|value| value.as_str())?
        .trim();
    if text.is_empty() {
        return None;
    }
    let time = value
        .get("time")
        .or_else(|| value.get("lineTime"))
        .or_else(|| value.get("startTime"))
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
                    let cover = extract_cover_url(item);
                    if !id.is_empty() && !name.is_empty() {
                        out.push(RankingCategory {
                            id,
                            name,
                            cover,
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
    let mut all = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Fetch 6 pages (up to 180 playlists)
    for pn in 0..6 {
        let response = client
            .get("https://wapi.kuwo.cn/api/pc/classify/playlist/getRcmPlayList")
            .query(&[
                ("loginUid", "0"),
                ("loginSid", "0"),
                ("appUid", "76039576"),
                ("pn", &pn.to_string()),
                ("rn", "30"),
                ("order", "hot"),
            ])
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        let Ok(response) = response else {
            break;
        };
        let Ok(json) = response.json::<serde_json::Value>().await else {
            break;
        };
        let items = json
            .get("data")
            .and_then(|value| value.get("data"))
            .or_else(|| json.get("data"))
            .and_then(|value| value.as_array());
        let Some(items) = items else { break };

        let mut page_count = 0;
        for item in items {
            let id = item.get("id").map(json_value_to_string).unwrap_or_default();
            let name = item
                .get("name")
                .or_else(|| item.get("title"))
                .map(json_value_to_string)
                .unwrap_or_default();
            if id.is_empty() || name.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            page_count += 1;
            all.push(PlaylistInfo {
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
            });
        }
        // If page returned fewer than expected, no more pages
        if page_count < 25 {
            break;
        }
    }

    println!(
        "xiaowo playlists: fetched {} total from {} pages",
        all.len(),
        if all.len() > 60 {
            3
        } else if all.len() > 30 {
            2
        } else {
            1
        }
    );
    all
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

// ─── Kugou native adapter ───

async fn search_via_kugou(client: &reqwest::Client, keyword: &str) -> Vec<SongResult> {
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for page in 1..=3 {
        let response = client
            .get("http://mobilecdn.kugou.com/api/v3/search/song")
            .query(&[
                ("format", "json"),
                ("keyword", keyword),
                ("page", &page.to_string()),
                ("pagesize", "100"),
                ("showtype", "1"),
            ])
            .timeout(Duration::from_secs(4))
            .send()
            .await;
        let Ok(response) = response else {
            continue;
        };
        let Ok(json) = response.json::<serde_json::Value>().await else {
            continue;
        };
        let songs = json
            .get("data")
            .and_then(|d| d.get("info"))
            .and_then(|v| v.as_array());
        let Some(songs) = songs else {
            continue;
        };
        let mut has_data = false;
        for item in songs {
            let hash = item
                .get("hash")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if hash.is_empty() || !seen.insert(hash.clone()) {
                continue;
            }
            has_data = true;
            let title = item
                .get("songname")
                .or_else(|| item.get("filename"))
                .map(json_value_to_string)
                .unwrap_or_default();
            let artist = item
                .get("singername")
                .or_else(|| item.get("singer"))
                .map(json_value_to_string)
                .unwrap_or_default();
            let album = item
                .get("album_name")
                .map(json_value_to_string)
                .filter(|v| !v.is_empty());
            let duration = item
                .get("duration")
                .or_else(|| item.get("second"))
                .and_then(|v| v.as_f64())
                .map(|s| s.max(30.0));
            let cover_url = item
                .get("img")
                .or_else(|| item.get("album_img"))
                .or_else(|| item.get("cover"))
                .map(json_value_to_string)
                .filter(|v| !v.is_empty());
            // Kugou quality: sqfilesize → flac, hqfilesize → 320k, filesize → 128k
            let quality = kugou_quality_from_item(item);
            results.push(SongResult {
                id: hash,
                title,
                artist,
                source: "kugou".to_string(),
                source_id: 0,
                platform: "kg".to_string(),
                album,
                cover_url,
                duration,
                score: 0,
                quality,
            });
        }
        if !has_data {
            break;
        }
    }
    results
}

async fn fetch_kugou_song_url(client: &reqwest::Client, song_id: &str) -> Option<SongUrlResult> {
    let response = client
        .get("http://trackercdn.kugou.com/i/v2/")
        .query(&[
            ("key", song_id),
            ("pid", "2"),
            ("behavior", "play"),
            ("cmd", "26"),
            ("version", "9108"),
            ("br", "320"),
        ])
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?;
    let json = response.json::<serde_json::Value>().await.ok()?;
    let play_url = json
        .get("url")
        .or_else(|| json.get("play_url"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if play_url.is_empty() || play_url == "null" {
        return None;
    }
    Some(SongUrlResult {
        url: play_url.to_string(),
        quality: "320k".to_string(),
        format: "mp3".to_string(),
    })
}

async fn fetch_kugou_song_info(client: &reqwest::Client, song_id: &str) -> Option<SongInfoResult> {
    let response = client
        .get("http://m.kugou.com/app/i/getSongInfo.php")
        .query(&[("cmd", "playInfo"), ("hash", song_id)])
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?;
    let json = response.json::<serde_json::Value>().await.ok()?;
    let title = json
        .get("songName")
        .or_else(|| json.get("name"))
        .map(json_value_to_string)
        .unwrap_or_default();
    let artist = json
        .get("singerName")
        .or_else(|| json.get("singer"))
        .or_else(|| json.get("author"))
        .map(json_value_to_string)
        .unwrap_or_default();
    let cover_url = json
        .get("imgUrl")
        .or_else(|| json.get("cover"))
        .map(json_value_to_string)
        .unwrap_or_default();
    // Fetch lyrics from URL
    let lyrics = if let Some(lyric_url) = json
        .get("lyrics")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        if let Ok(r) = client
            .get(lyric_url)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            r.text().await.ok()
        } else {
            None
        }
    } else {
        None
    };
    Some(SongInfoResult {
        id: song_id.to_string(),
        title,
        artist,
        album: json.get("album_name").map(json_value_to_string),
        cover_url: Some(cover_url).filter(|v| !v.is_empty()),
        lyrics,
        duration: None,
        platform: "kg".to_string(),
    })
}

// ─── Netease native adapter ───

async fn search_via_netease(client: &reqwest::Client, keyword: &str) -> Vec<SongResult> {
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for offset in (0..3).map(|i| i * 100) {
        let response = client
            .get("http://music.163.com/api/search/get/")
            .query(&[
                ("s", keyword),
                ("type", "1"),
                ("offset", &offset.to_string()),
                ("limit", "100"),
            ])
            .header("Referer", "https://music.163.com/")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .timeout(Duration::from_secs(4))
            .send()
            .await;
        let Ok(response) = response else {
            continue;
        };
        let Ok(json) = response.json::<serde_json::Value>().await else {
            continue;
        };
        let songs = json
            .get("result")
            .and_then(|r| r.get("songs"))
            .and_then(|v| v.as_array());
        let Some(songs) = songs else {
            break;
        };
        let mut has_data = false;
        for item in songs {
            let id = item.get("id").map(|v| v.to_string()).unwrap_or_default();
            if id.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            has_data = true;
            let title = item
                .get("name")
                .map(json_value_to_string)
                .unwrap_or_default();
            let artists: Vec<String> = item
                .get("artists")
                .or_else(|| item.get("ar"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.get("name").and_then(|n| n.as_str().map(String::from)))
                        .collect()
                })
                .unwrap_or_default();
            let album = item
                .get("album")
                .or_else(|| item.get("al"))
                .and_then(|a| a.get("name"))
                .map(json_value_to_string);
            let cover_url = item
                .get("album")
                .or_else(|| item.get("al"))
                .and_then(|a| {
                    a.get("picUrl")
                        .or_else(|| a.get("picurl"))
                        .or_else(|| a.get("pic"))
                })
                .map(json_value_to_string);
            let duration = item
                .get("duration")
                .or_else(|| item.get("dt"))
                .and_then(|v| v.as_f64())
                .map(|ms| ms / 1000.0)
                .filter(|&s| s > 10.0);
            let quality = netease_quality_from_item(item);
            results.push(SongResult {
                id,
                title,
                artist: artists.join(" / "),
                source: "netease".to_string(),
                source_id: 0,
                platform: "wy".to_string(),
                album,
                cover_url,
                duration,
                score: 0,
                quality,
            });
        }
        if !has_data {
            break;
        }
    }
    results
}

async fn fetch_netease_song_url(client: &reqwest::Client, song_id: &str) -> Option<SongUrlResult> {
    let response = client
        .get("http://music.163.com/api/song/enhance/player/url")
        .query(&[
            ("id", song_id),
            ("ids", &format!("[{}]", song_id)),
            ("br", "320000"),
        ])
        .header("Referer", "https://music.163.com/")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        )
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    let json = response.json::<serde_json::Value>().await.ok()?;
    let data = json.get("data").and_then(|d| d.as_array())?.first()?;
    let play_url = data.get("url").and_then(|v| v.as_str()).unwrap_or("");
    if play_url.is_empty() || play_url.contains("null") {
        return None;
    }
    let br = data.get("br").and_then(|v| v.as_f64()).unwrap_or(320000.0);
    let quality = if br >= 320000.0 {
        "320k"
    } else if br >= 128000.0 {
        "128k"
    } else {
        "mp3"
    };
    Some(SongUrlResult {
        url: play_url.to_string(),
        quality: quality.to_string(),
        format: "mp3".to_string(),
    })
}

async fn fetch_netease_song_info(
    client: &reqwest::Client,
    song_id: &str,
) -> Option<SongInfoResult> {
    let resp = client
        .get(&format!(
            "http://music.163.com/api/song/lyric?id={}&lv=-1&kv=-1&tv=-1",
            song_id
        ))
        .header("Referer", "https://music.163.com/")
        .header("User-Agent", "Mozilla/5.0")
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?;
    let json = resp.json::<serde_json::Value>().await.ok()?;
    let lyrics = json
        .get("lrc")
        .and_then(|l| l.get("lyric"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    // Fetch song detail (title/artist/cover)
    let detail_json = client
        .get(&format!(
            "http://music.163.com/api/song/detail?id={}&ids=[{}]",
            song_id, song_id
        ))
        .header("Referer", "https://music.163.com/")
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok();
    let detail = if let Some(resp) = detail_json {
        resp.json::<serde_json::Value>().await.ok()
    } else {
        None
    };
    let (title, artist, cover) = detail
        .and_then(|d| {
            d.get("songs")
                .and_then(|s| s.as_array())
                .and_then(|a| a.first().cloned())
        })
        .map(|s| {
            let t = s.get("name").map(json_value_to_string).unwrap_or_default();
            let a: Vec<String> = s
                .get("artists")
                .or_else(|| s.get("ar"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.get("name").and_then(|n| n.as_str().map(String::from)))
                        .collect()
                })
                .unwrap_or_default();
            let c = s
                .get("album")
                .and_then(|al| al.get("picUrl").or_else(|| al.get("picurl")))
                .map(json_value_to_string);
            (t, a.join(" / "), c)
        })
        .unwrap_or_default();
    Some(SongInfoResult {
        id: song_id.to_string(),
        title,
        artist,
        album: None,
        cover_url: cover,
        lyrics,
        duration: None,
        platform: "wy".to_string(),
    })
}

// ─── Bilibili native adapter ───

async fn search_via_bilibili(client: &reqwest::Client, keyword: &str) -> Vec<SongResult> {
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for page in 1..=3 {
        let response = client
            .get("https://api.bilibili.com/x/web-interface/search/type")
            .query(&[
                ("search_type", "video"),
                ("keyword", keyword),
                ("page", &page.to_string()),
            ])
            .header("Referer", "https://www.bilibili.com/")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .timeout(Duration::from_secs(4))
            .send()
            .await;
        let Ok(response) = response else {
            continue;
        };
        let Ok(json) = response.json::<serde_json::Value>().await else {
            continue;
        };
        let videos = json
            .get("data")
            .and_then(|d| d.get("result"))
            .and_then(|v| v.as_array());
        let Some(videos) = videos else {
            break;
        };
        let mut has_data = false;
        for item in videos {
            let bvid = item
                .get("bvid")
                .map(json_value_to_string)
                .or_else(|| item.get("aid").map(|v| v.to_string()))
                .unwrap_or_default();
            if bvid.is_empty() || !seen.insert(bvid.clone()) {
                continue;
            }
            has_data = true;
            let title = item
                .get("title")
                .map(json_value_to_string)
                .unwrap_or_default();
            let author = item
                .get("author")
                .map(json_value_to_string)
                .unwrap_or_default();
            let cover_url = item
                .get("pic")
                .or_else(|| item.get("cover"))
                .map(json_value_to_string);
            let duration = item.get("duration").and_then(|v| v.as_f64());
            if !title.is_empty() {
                results.push(SongResult {
                    id: bvid,
                    title,
                    artist: author,
                    source: "bilibili".to_string(),
                    source_id: 0,
                    platform: "bilibili".to_string(),
                    album: None,
                    cover_url,
                    duration,
                    score: 0,
                    quality: None,
                });
            }
        }
        if !has_data {
            break;
        }
    }
    results
}

async fn fetch_bilibili_song_url(client: &reqwest::Client, bvid: &str) -> Option<SongUrlResult> {
    let resp = client
        .get(&format!(
            "https://api.bilibili.com/x/web-interface/view?bvid={}",
            bvid
        ))
        .header("Referer", "https://www.bilibili.com/")
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?;
    let json = resp.json::<serde_json::Value>().await.ok()?;
    let cid = json
        .get("data")
        .and_then(|d| d.get("cid"))
        .and_then(|v| v.as_f64())
        .map(|v| v as u64)?;
    let play_resp = client
        .get(&format!(
            "https://api.bilibili.com/x/player/playurl?bvid={}&cid={}&qn=16&type=mp4",
            bvid, cid
        ))
        .header("Referer", "https://www.bilibili.com/")
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?;
    let play_json = play_resp.json::<serde_json::Value>().await.ok()?;
    let first_url = play_json
        .get("data")
        .and_then(|d| d.get("durl"))
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|d| d.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if first_url.is_empty() {
        return None;
    }
    Some(SongUrlResult {
        url: first_url.to_string(),
        quality: "mp4".to_string(),
        format: "mp4".to_string(),
    })
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
            .map(|value| {
                if value > 10000.0 {
                    value / 1000.0
                } else {
                    value
                }
            }),
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
    if rate >= 1000000 {
        "Lossless".to_string()
    } else if rate >= 1000 {
        // rate in bps like 320000 → "320k"
        format!("{}k", rate / 1000)
    } else if rate > 0 {
        // rate in kbps like 320 → "320k"
        format!("{rate}k")
    } else {
        String::new()
    }
}

/// Kugou quality: sqfilesize (>0 → flac), hqfilesize (>0 → 320k), filesize (>0 → 128k)
fn kugou_quality_from_item(item: &serde_json::Value) -> Option<String> {
    let sq = item
        .get("sqfilesize")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let hq = item
        .get("hqfilesize")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let fsize = item
        .get("filesize")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    if sq > 0 {
        Some("flac".to_string())
    } else if hq > 0 {
        Some("320k".to_string())
    } else if fsize > 0 {
        Some("128k".to_string())
    } else {
        None
    }
}

/// Netease quality: extract from privilege.maxBitrate (in bps)
fn netease_quality_from_item(item: &serde_json::Value) -> Option<String> {
    let br = item
        .get("privilege")
        .and_then(|p| p.get("maxBitrate").or_else(|| p.get("maxbr")))
        .and_then(|v| v.as_f64())
        .map(|b| b as u32)
        .unwrap_or(0);
    if br >= 999000 {
        Some("flac".to_string())
    } else if br >= 320000 {
        Some("320k".to_string())
    } else if br >= 192000 {
        Some("192k".to_string())
    } else if br >= 128000 {
        Some("128k".to_string())
    } else if br > 0 {
        Some(format_bitrate(br))
    } else {
        None
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
                    let cover = extract_cover_url(item);
                    if !id.is_empty() && !name.is_empty() {
                        results.push(RankingCategory {
                            id: id.to_string(),
                            name: name.to_string(),
                            cover,
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
                        let cover = extract_cover_url(item);
                        if !id.is_empty() && !name.is_empty() {
                            results.push(RankingCategory {
                                id: id.to_string(),
                                name: name.to_string(),
                                cover,
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

pub async fn parse_kugou_shared_playlist(input: &str) -> Result<SharedPlaylist, String> {
    let trimmed = input.trim();

    // 1) Try to parse as direct JSON / HTML text (pasted response body)
    if let Some(result) = try_parse_pasted_text(trimmed) {
        return Ok(result);
    }

    // 2) Otherwise treat as URL
    let normalized_url = normalize_share_url(trimmed)
        .ok_or_else(|| "没有找到可解析的酷狗歌单链接或数据，请粘贴完整分享链接、API响应JSON或页面HTML内容".to_string())?;
    let (songs, final_url, title, cover) = fetch_and_parse_url(&normalized_url).await?;

    let note = if songs.is_empty() {
        Some("已识别酷狗歌单链接，但页面和公开接口都没有返回可解析歌曲。请确认链接不是私密歌单，或重新复制完整分享链接。如果是在浏览器中登录后复制的页面内容，可以尝试直接粘贴页面HTML。".to_string())
    } else {
        None
    };

    Ok(SharedPlaylist {
        playlist: PlaylistInfo {
            id: final_url.clone(),
            name: title,
            cover,
            song_count: Some(songs.len()),
            source_id: 0,
            source_name: "酷狗分享".to_string(),
        },
        songs,
        external_url: final_url,
        note,
    })
}

/// Try to parse pasted raw text (JSON API response or HTML) directly
fn try_parse_pasted_text(text: &str) -> Option<SharedPlaylist> {
    let trimmed = text.trim();
    // Reject if it looks like a URL
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return None;
    }
    // Reject if it's not JSON/HTML-like at all
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') && !trimmed.contains('<') {
        return None;
    }

    let mut songs = extract_kugou_songs_from_text(trimmed);
    if songs.is_empty() {
        songs = extract_songs_from_html_lists(trimmed);
    }
    if songs.is_empty() {
        return None;
    }

    // Try to extract title from JSON data
    let mut title = "酷狗分享歌单".to_string();
    let mut external_url = String::new();

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        // Try listname from get_other_list_file response
        if let Some(name) = value
            .get("data")
            .and_then(|d| d.get("listname"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            title = name.to_string();
        }
        // Extract global_collection_id for external_url
        if let Some(gcid) = value
            .get("data")
            .and_then(|d| d.get("global_collection_id"))
            .or_else(|| value.get("global_collection_id"))
            .and_then(|v| v.as_str())
        {
            external_url = format!(
                "https://pubsongscdn.kugou.com/v2/get_other_list_file?type=0&module=playlist&global_collection_id={}",
                gcid
            );
        }
    }

    Some(SharedPlaylist {
        playlist: PlaylistInfo {
            id: external_url.clone(),
            name: title,
            cover: extract_first_image(trimmed),
            song_count: Some(songs.len()),
            source_id: 0,
            source_name: "酷狗分享".to_string(),
        },
        songs,
        external_url,
        note: None,
    })
}

/// Build HTTP client with kugou cookies loaded from config
fn build_kugou_client() -> reqwest::Result<reqwest::Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8"
            .parse()
            .unwrap(),
    );
    headers.insert(
        reqwest::header::ACCEPT_LANGUAGE,
        "zh-CN,zh;q=0.9".parse().unwrap(),
    );
    headers.insert("sec-ch-ua", r#""Google Chrome";v="149", "Chromium";v="149", "Not)A;Brand";v="24""#.parse().unwrap());
    headers.insert("sec-ch-ua-platform", "\"Windows\"".parse().unwrap());

    reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36",
        )
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(Duration::from_secs(12))
        .build()
}

/// Load kugou cookies from a local config file
fn load_kugou_cookie() -> String {
    // Try kugou_cookies.txt in current directory
    let config_path = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("kugou_cookies.txt");
    if let Ok(content) = fs::read_to_string(&config_path) {
        let cookie = content.trim().to_string();
        if !cookie.is_empty() {
            println!("[parse_kugou] Loaded cookie from kugou_cookies.txt");
            return cookie;
        }
    }
    // Also try user home directory
    if let Some(home) = dirs_next_home() {
        let cookie_file = home.join(".miku_tunes").join("kugou_cookies.txt");
        if let Ok(content) = fs::read_to_string(&cookie_file) {
            let cookie = content.trim().to_string();
            if !cookie.is_empty() {
                println!("[parse_kugou] Loaded cookie from ~/.miku_tunes/kugou_cookies.txt");
                return cookie;
            }
        }
    }
    String::new()
}

fn dirs_next_home() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    } else {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

async fn fetch_and_parse_url(url: &str) -> Result<(Vec<SongDetail>, String, String, Option<String>), String> {
    let client = build_kugou_client().map_err(|err| err.to_string())?;

    let cookie = load_kugou_cookie();
    let mut req = client.get(url);
    if !cookie.is_empty() {
        req = req.header(reqwest::header::COOKIE, &cookie);
    }

    let response = req
        .send()
        .await
        .map_err(|err| format!("分享链接访问失败: {err}"))?;
    let final_url = response.url().to_string();
    let text = response
        .text()
        .await
        .map_err(|err| format!("分享内容读取失败: {err}"))?;

    let title = extract_html_title(&text).unwrap_or_else(|| "酷狗分享歌单".to_string());
    let cover = extract_first_image(&text);
    let mut songs = extract_kugou_songs_from_text(&text);

    // Also try parsing rendered HTML <li> lists
    if songs.is_empty() {
        songs = extract_songs_from_html_lists(&text);
    }

    if songs.is_empty() {
        for nested_url in extract_kugou_urls(&text)
            .into_iter()
            .chain(kugou_playlist_candidate_urls(url, &final_url, &text))
        {
            if nested_url == url || nested_url == final_url {
                continue;
            }
            let mut req = client.get(&nested_url);
            if !cookie.is_empty() {
                req = req.header(reqwest::header::COOKIE, &cookie);
            }
            if let Ok(response) = req.send().await {
                if let Ok(nested_text) = response.text().await {
                    songs = extract_kugou_songs_from_text(&nested_text);
                    if songs.is_empty() {
                        songs = extract_songs_from_html_lists(&nested_text);
                    }
                    if !songs.is_empty() {
                        break;
                    }
                }
            }
        }
    }

    Ok((songs, final_url, title, cover))
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

fn normalize_share_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let mut url = extract_url_like(trimmed).unwrap_or_else(|| trimmed.to_string());
    url = url
        .trim()
        .trim_matches(|ch| {
            matches!(
                ch,
                '"' | '\'' | '`' | '<' | '>' | ')' | ']' | '}' | '，' | '。'
            )
        })
        .to_string();
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(decode_basic_html(&url).replace("\\/", "/"))
    } else {
        None
    }
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
        .find(|ch: char| {
            ch == '"'
                || ch == '\''
                || ch == '`'
                || ch.is_whitespace()
                || ch == '<'
                || ch == ')'
                || ch == ']'
        })
        .unwrap_or(rest.len());
    Some(decode_basic_html(&rest[..end]).replace("\\/", "/"))
}

fn extract_kugou_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find("http") {
        if let Some(candidate) = extract_url_like(&rest[pos..]) {
            if candidate.contains("kugou.com")
                || candidate.contains("pubsongscdn.kugou.com")
                || candidate.contains("t1.kugou.com")
            {
                let url = decode_basic_html(candidate.trim_matches(['\\', '"', '\'']));
                if !urls.contains(&url) {
                    urls.push(url);
                }
            }
        }
        rest = &rest[pos + 4..];
    }
    urls
}

fn decode_basic_html(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn kugou_playlist_candidate_urls(input_url: &str, final_url: &str, text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    push_unique(&mut urls, final_url.to_string());
    for url in extract_kugou_urls(text) {
        push_unique(&mut urls, url);
    }
    for gcid in extract_gcid_values(input_url)
        .into_iter()
        .chain(extract_gcid_values(final_url))
        .chain(extract_gcid_values(text))
    {
        push_unique(
            &mut urls,
            format!("https://www.kugou.com/songlist/{gcid}/?src_cid={gcid}"),
        );
    }
    for collection_id in extract_collection_ids(text)
        .into_iter()
        .chain(extract_collection_ids(final_url))
        .chain(extract_collection_ids(input_url))
    {
        push_unique(
            &mut urls,
            format!(
                "https://pubsongscdn.kugou.com/v2/get_other_list_file?srcappid=2919&clientver=20000&clienttime=1780992177049&mid=1fb95e3ddf2a3557b2c5ba9dff1d6186&uuid=1780992177049&dfid=1u0bY23Rg3184bpf3n37fOPQ&appid=1058&type=0&module=playlist&page=1&pagesize=500&global_collection_id={collection_id}"
            ),
        );
    }
    urls
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.trim().is_empty() && !values.contains(&value) {
        values.push(value);
    }
}

fn extract_gcid_values(text: &str) -> Vec<String> {
    extract_token_values(text, "gcid_", |ch| ch.is_ascii_alphanumeric() || ch == '_')
        .into_iter()
        .map(|suffix| format!("gcid_{suffix}"))
        .collect()
}

fn extract_collection_ids(text: &str) -> Vec<String> {
    extract_token_values(text, "collection_", |ch| {
        ch.is_ascii_alphanumeric() || ch == '_'
    })
    .into_iter()
    .map(|suffix| format!("collection_{suffix}"))
    .collect()
}

fn extract_token_values<F>(text: &str, prefix: &str, keep: F) -> Vec<String>
where
    F: Fn(char) -> bool,
{
    let decoded = decode_basic_html(text).replace("\\/", "/");
    let mut values = Vec::new();
    let mut rest = decoded.as_str();
    while let Some(pos) = rest.find(prefix) {
        let tail = &rest[pos + prefix.len()..];
        let token: String = tail.chars().take_while(|ch| keep(*ch)).collect();
        let step = token.len().min(tail.len()).max(1);
        if !token.is_empty() && !values.contains(&token) {
            values.push(token);
        }
        rest = &tail[step..];
    }
    values
}

/// Extract songs from rendered HTML <li> tag format (the songlist page when login cookies work)
fn extract_songs_from_html_lists(text: &str) -> Vec<SongDetail> {
    let mut out = Vec::new();
    let decoded = decode_basic_html(text).replace("\\/", "/");

    // Pattern: <li>...<a title="Artist - Song Name" href="..." data="hash|timelen">...</a>...</li>
    let mut rest = decoded.as_str();
    while let Some(li_start) = rest.find("<li>") {
        let li_tail = &rest[li_start..];
        let Some(li_end) = li_tail.find("</li>") else {
            break;
        };
        let li_content = &li_tail[..li_end + 5];

        // Extract data attribute: data="HASH|timelen"
        let hash = extract_html_attr(li_content, r#"data=""#, '"')
            .and_then(|attr| attr.split('|').next().map(|s| s.to_string()));

        // Extract href for mixsong URL (available for future use)
        let _mixsong_url = extract_html_attr(li_content, r#"href=""#, '"');

        // Extract title="Artist - Song Title"
        let title_attr = extract_html_attr(li_content, r#"title=""#, '"')
            .unwrap_or_default();

        // Parse "Artist - Song Title" from the title attribute
        let (artist, title) = if let Some(pos) = title_attr.find(" - ") {
            let artist = title_attr[..pos].trim().to_string();
            let title = title_attr[pos + 3..].trim().to_string();
            if !artist.is_empty() && !title.is_empty() {
                (artist, title)
            } else {
                (String::new(), title_attr)
            }
        } else {
            (String::new(), title_attr)
        };

        // Fallback: try <i> tag content
        let (fallback_artist, fallback_title) = extract_html_inner_text(li_content, "i");
        let artist = if artist.is_empty() { fallback_artist } else { artist };
        let title = if title.is_empty() { fallback_title } else { title };

        if title.is_empty() || title.contains("http") || title.len() > 200 {
            rest = &rest[li_start + li_end + 5..];
            continue;
        }

        let id = hash.unwrap_or_else(|| format!("kg-li-{}-{}", title, artist));
        let timelen = extract_html_attr(li_content, r#"data=""#, '"')
            .and_then(|attr| attr.split('|').nth(1).and_then(|s| s.parse::<f64>().ok()));

        out.push(SongDetail {
            id,
            title,
            artist: if artist.is_empty() { "未知歌手".to_string() } else { artist },
            album: None,
            album_id: None,
            cover_url: None,
            duration: timelen.map(|t| {
                if t > 10000.0 { t / 1000.0 } else { t }
            }),
            source_id: 0,
            platform: "kg".to_string(),
        });

        rest = &rest[li_start + li_end + 5..];
    }
    dedupe_song_details(out)
}

/// Extract an HTML attribute value like attr="value"
fn extract_html_attr(text: &str, prefix: &str, delimiter: char) -> Option<String> {
    let pos = text.find(prefix)?;
    let val_start = pos + prefix.len();
    let val_tail = &text[val_start..];
    let end = val_tail.find(delimiter)?;
    let val = &val_tail[..end];
    let decoded = decode_basic_html(val).trim().to_string();
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

/// Extract inner text of the first matching HTML tag
fn extract_html_inner_text(text: &str, tag: &str) -> (String, String) {
    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);
    let Some(start) = text.find(&open_tag) else {
        return (String::new(), String::new());
    };
    let inner_start = start + open_tag.len();
    let Some(end) = text[inner_start..].find(&close_tag) else {
        return (String::new(), String::new());
    };
    let inner = text[inner_start..inner_start + end].trim().to_string();
    // Parse "Artist - Title" from <i>Artist - Title</i>
    if let Some(pos) = inner.find(" - ") {
        let artist = inner[..pos].trim().to_string();
        let title = inner[pos + 3..].trim().to_string();
        (artist, title)
    } else {
        (String::new(), inner)
    }
}

fn extract_kugou_songs_from_text(text: &str) -> Vec<SongDetail> {
    let mut out = Vec::new();
    let decoded = decode_basic_html(text).replace("\\/", "/");
    for candidate_text in [text, decoded.as_str()] {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(candidate_text.trim()) {
            collect_song_details(&value, &mut out);
        }
        for marker in [
            "var nData",
            "nData",
            "window.__INITIAL_STATE__",
            "window.__INITIAL_DATA__",
            "__NUXT__",
        ] {
            if let Some(value) = extract_assigned_json(candidate_text, marker) {
                collect_song_details(&value, &mut out);
            }
        }
        for value in collect_json_values(candidate_text) {
            collect_song_details(&value, &mut out);
            if out.len() >= 500 {
                break;
            }
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
        "window.__INITIAL_DATA__=",
        "__NUXT__=",
        "var nData",
        "\"data\"",
        "\"info\"",
        "\"listinfo\"",
        "\"global_collection_id\"",
        "songs",
        "\"songs\"",
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
    let song_like = [
        "hash",
        "audio_id",
        "songid",
        "song_id",
        "filename",
        "fileName",
        "timelen",
        "time_len",
        "duration",
        "download",
        "relate_goods",
        "song_url",
        "albuminfo",
    ]
    .iter()
    .any(|key| value.get(*key).is_some());
    if !song_like {
        return None;
    }
    let raw_title = first_str(
        value,
        &[
            "songname",
            "song_name",
            "audio_name",
            "filename",
            "fileName",
            "name",
            "title",
        ],
    )?;
    let mut title = raw_title.clone();
    let mut artist = first_str(
        value,
        &[
            "singername",
            "singer_name",
            "author_name",
            "artist",
            "singer",
            "singers",
            "singerinfo",
            "singerInfo",
        ],
    )
    .unwrap_or_else(|| "未知歌手".to_string());
    if (artist.is_empty() || artist == "未知歌手") && raw_title.contains(" - ") {
        let mut parts = raw_title.splitn(2, " - ");
        if let (Some(left), Some(right)) = (parts.next(), parts.next()) {
            if !left.trim().is_empty() && !right.trim().is_empty() {
                artist = left.trim().to_string();
                title = right.trim().to_string();
            }
        }
    }
    if title.contains(" - ") {
        let current_title = title.clone();
        let mut parts = current_title.splitn(2, " - ");
        if let (Some(left), Some(right)) = (parts.next(), parts.next()) {
            if artist == left.trim() && !right.trim().is_empty() {
                title = right.trim().to_string();
            }
        }
    }
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
        cover_url: first_str(
            value,
            &[
                "img",
                "image",
                "cover",
                "pic",
                "album_img",
                "albumimg",
                "sizable_cover",
            ],
        )
        .map(normalize_kugou_cover_url),
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
        .map(json_value_to_string)
        .filter(|value| !value.trim().is_empty())
}

fn normalize_kugou_cover_url(value: String) -> String {
    let mut url = value
        .replace("{size}", "400")
        .replace("{SIZE}", "400")
        .trim()
        .to_string();
    if url.starts_with("//") {
        url = format!("https:{url}");
    } else if url.starts_with("http://") {
        url = url.replacen("http://", "https://", 1);
    }
    url
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kugou_concept_playlist_json() {
        let text = r#"{
            "data": {
                "info": [{
                    "hash": "95014B8A1544E30110B1663549BAD271",
                    "audio_id": 56390512,
                    "name": "Sasha Alex Sloan - Dancing With Your Ghost",
                    "timelen": 198000,
                    "albuminfo": {"name": "Dancing With Your Ghost"},
                    "singerinfo": [{"name": "Sasha Alex Sloan"}],
                    "cover": "http:\/\/imge.kugou.com\/stdmusic\/{size}\/cover.jpg"
                }]
            }
        }"#;
        let songs = extract_kugou_songs_from_text(text);
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].title, "Dancing With Your Ghost");
        assert_eq!(songs[0].artist, "Sasha Alex Sloan");
        assert_eq!(songs[0].duration, Some(198.0));
        assert!(songs[0]
            .cover_url
            .as_deref()
            .unwrap_or_default()
            .starts_with("https://"));
    }

    #[test]
    fn parses_kugou_html_ndata_songs() {
        let text = r#"<script>
            var nData = {
                "listinfo": {
                    "global_collection_id": "collection_3_1488927378_18_0",
                    "encode_gcid": "gcid_3zsc75w8ziz07c"
                },
                "songs": [{
                    "hash": "284017622B0A01117DF25019176BCCA7",
                    "audio_id": 571602467,
                    "name": "\u7b11\u5929\u5199\u610f - \u5c3d\u529b\u4e86\u5c31\u662f\u5706\u6ee1",
                    "timelen": 243000,
                    "singerinfo": [{"name": "\u7b11\u5929\u5199\u610f"}],
                    "cover": "http:\/\/imge.kugou.com\/stdmusic\/{size}\/20260509.jpg"
                }]
            };
        </script>"#;
        let songs = extract_kugou_songs_from_text(text);
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].artist, "笑天写意");
        assert_eq!(songs[0].title, "尽力了就是圆满");
    }

    #[test]
    fn extracts_url_from_pasted_label() {
        assert_eq!(
            normalize_share_url("Link https://www.kugou.com/songlist/gcid_abc/?x=1").as_deref(),
            Some("https://www.kugou.com/songlist/gcid_abc/?x=1")
        );
    }

    #[test]
    fn candidate_urls_include_final_redirect_metadata() {
        let urls = kugou_playlist_candidate_urls(
            "https://t1.kugou.com/2PRIzd7G2V2",
            "https://www.kugou.com/songlist/gcid_3zsc75w8ziz07c/?share=1",
            r#"{"listinfo":{"global_collection_id":"collection_3_1488927378_18_0"}}"#,
        );
        assert!(urls.iter().any(|url| url.contains("gcid_3zsc75w8ziz07c")));
        assert!(urls
            .iter()
            .any(|url| url.contains("global_collection_id=collection_3_1488927378_18_0")));
    }

    #[test]
    fn parses_html_li_songlist_format() {
        let text = r#"<div class="r" style="min-height: 400px;">
                 <div id="songs" class="list1">
                     <strong>&lt;念 &gt;- 歌曲列表</strong>
                     <ul>
                         <li>
                             <input type="checkbox" class="cb checkItem" checked="true"  style="margin-left: 8px;">
                             <a title="笑天写意 - 尽力了就是圆满" hidefocus="true" href="https://www.kugou.com/mixsong/ejov0g0c.html" data="284017622B0A01117DF25019176BCCA7|243000">
                                 <span class="num1">01</span>
                                 <span class="text">
                                     <i>笑天写意 - 尽力了就是圆满</i>
                                 </span>
                             </a>
                         </li>
                         <li>
                             <input type="checkbox" class="cb checkItem" checked="true"  style="margin-left: 8px;">
                             <a title="沧桑大涵 - 荒漠不败的花" hidefocus="true" href="https://www.kugou.com/mixsong/eof6wg94.html" data="21753BAF95A576D4D5CB1099A9AD6095|209000">
                                 <span class="num1">02</span>
                                 <span class="text">
                                     <i>沧桑大涵 - 荒漠不败的花</i>
                                 </span>
                             </a>
                         </li>
                     </ul>
                 </div>
        </div>"#;
        let songs = extract_songs_from_html_lists(text);
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].title, "尽力了就是圆满");
        assert_eq!(songs[0].artist, "笑天写意");
        assert_eq!(songs[0].duration, Some(243.0));
        assert_eq!(songs[0].id, "284017622B0A01117DF25019176BCCA7");
        assert_eq!(songs[1].title, "荒漠不败的花");
        assert_eq!(songs[1].artist, "沧桑大涵");
        assert_eq!(songs[1].id, "21753BAF95A576D4D5CB1099A9AD6095");
    }

    #[test]
    fn parses_get_other_list_file_json() {
        let text = r#"{"error_code":0,"errmsg":"","data":{"info":[{"hash":"95014B8A1544E30110B1663549BAD271","audio_id":56390512,"name":"Sasha Alex Sloan - Dancing With Your Ghost","timelen":197773,"albuminfo":{"name":"Dancing With Your Ghost"},"singerinfo":[{"name":"Sasha Alex Sloan"}],"cover":"http://imge.kugou.com/stdmusic/{size}/20190626/20190626210118627550.jpg"}],"count":278}}"#;
        let songs = extract_kugou_songs_from_text(text);
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].title, "Dancing With Your Ghost");
        assert_eq!(songs[0].artist, "Sasha Alex Sloan");
        assert_eq!(songs[0].duration, Some(197.773));
    }

    #[test]
    fn try_parse_pasted_json_text() {
        let text = r#"{"error_code":0,"data":{"info":[{"hash":"HASH001","audio_id":123,"name":"Test Song","timelen":180000,"albuminfo":{"name":"Test Album"},"singerinfo":[{"name":"Test Artist"}],"cover":"http://img.test/cover.jpg"}]}}"#;
        let result = try_parse_pasted_text(text);
        assert!(result.is_some());
        let playlist = result.unwrap();
        assert!(playlist.songs.len() > 0);
        assert_eq!(playlist.songs[0].title, "Test Song");
        assert_eq!(playlist.songs[0].artist, "Test Artist");
    }

    #[test]
    fn rejects_urls_in_pasted_text() {
        assert!(try_parse_pasted_text("https://www.kugou.com/songlist/gcid_abc").is_none());
    }
}

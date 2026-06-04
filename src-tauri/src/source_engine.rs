use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

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
    platforms: Vec<String>,  // e.g. ["kw", "kg", "tx", "wy", "mg"]
}

// ─── Search result ───
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongResult {
    pub title: String,
    pub artist: String,
    pub source: String,
    pub source_id: usize,
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

// ─── Hot keyword result ───
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotItem {
    pub title: String,
    pub source: String,
    pub source_id: usize,
}

// ─── Engine state ───
pub struct SourceEngine {
    sources: Mutex<Vec<SourceInfo>>,
    meta_map: Mutex<HashMap<usize, SourceMeta>>,
    active_source: Mutex<usize>,
    http_client: reqwest::Client,
    source_dir: PathBuf,
}

impl SourceEngine {
    pub fn new(source_dir: PathBuf) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .user_agent("MikuTunes/1.0")
            .build()
            .unwrap_or_default();

        let engine = Self {
            sources: Mutex::new(Vec::new()),
            meta_map: Mutex::new(HashMap::new()),
            active_source: Mutex::new(0),
            http_client,
            source_dir,
        };
        engine.scan_sources();
        engine
    }

    /// Scan 音源 directory, parse each JS file for metadata
    pub fn scan_sources(&self) {
        let mut sources = Vec::new();
        let mut meta_map = HashMap::new();
        let mut idx = 0usize;

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
                    let (name, meta) = parse_source_file(&path, idx, &file_name);

                    sources.push(SourceInfo {
                        id: idx,
                        name,
                        score: 0,
                        enabled: true,
                        file_name,
                    });
                    if let Some(m) = meta {
                        meta_map.insert(idx, m);
                    }
                    idx += 1;
                }
            }
        }

        // Random initial sort
        let mut rng = rand::thread_rng();
        sources.shuffle(&mut rng);
        for (i, s) in sources.iter_mut().enumerate() {
            s.id = i;
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

    pub fn score_source(&self, source_id: usize, found: bool) {
        if let Ok(mut sources) = self.sources.lock() {
            if let Some(source) = sources.iter_mut().find(|s| s.id == source_id) {
                if found { source.score += 1; } else { source.score -= 1; }
            }
        }
    }

    /// Search music from ALL sources in parallel
    pub async fn search(&self, keyword: &str) -> SearchResponse {
        let start = Instant::now();
        let sources = {
            let mut s = self.sources.lock().unwrap().clone();
            s.sort_by(|a, b| b.score.cmp(&a.score));
            s.into_iter().filter(|s| s.enabled).collect::<Vec<_>>()
        };
        let meta_map = self.meta_map.lock().unwrap().clone();
        let client = self.http_client.clone();
        let kw = keyword.to_string();

        let tasks: Vec<_> = sources.into_iter().filter_map(|source| {
            let meta = meta_map.get(&source.id).cloned()?;
            let client = client.clone();
            let kw = kw.clone();
            Some(tokio::spawn(async move {
                let results = search_via_lxmusic(&client, &meta, &kw).await;
                (source, results)
            }))
        }).collect();

        let mut all_results = Vec::new();
        let mut found_source: Option<String> = None;

        for task in tasks {
            if let Ok((source, results)) = task.await {
                if !results.is_empty() && found_source.is_none() {
                    found_source = Some(source.name.clone());
                }
                for mut r in results {
                    r.source_id = source.id;
                    r.score = source.score;
                    all_results.push(r);
                }
            }
        }

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
        let client = self.http_client.clone();

        let tasks: Vec<_> = meta_map.into_iter().map(|(id, meta)| {
            let client = client.clone();
            tokio::spawn(async move {
                let items = hot_via_lxmusic(&client, &meta).await;
                (id, meta.name, items)
            })
        }).collect();

        let mut results = Vec::new();
        for task in tasks {
            if let Ok((id, name, items)) = task.await {
                for item in items {
                    results.push(HotItem {
                        title: item,
                        source: name.clone(),
                        source_id: id,
                    });
                }
            }
        }

        results.sort_by(|a, b| b.source.cmp(&a.source));
        results.truncate(limit);
        results
    }
}

// ─── Parse JS source file ───

fn parse_source_file(path: &std::path::Path, id: usize, file_name: &str) -> (String, Option<SourceMeta>) {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (file_name.to_string(), None),
    };

    let name = extract_source_name(&content).unwrap_or_else(|| file_name.to_string());
    let api_url = extract_js_var(&content, "API_URL")
        .map(|s| s.trim_matches('"').to_string());
    let api_key = extract_js_var(&content, "API_KEY")
        .map(|s| s.trim_matches('"').to_string());
    let quality_str = extract_js_var(&content, "MUSIC_QUALITY");

    let platforms = quality_str
        .and_then(|s| {
            // Parse JSON object like {"kw":["128k"],"kg":["128k"],...}
            serde_json::from_str::<HashMap<String, serde_json::Value>>(&s).ok()
                .map(|map| map.keys().cloned().collect::<Vec<_>>())
        })
        .unwrap_or_else(|| vec!["kw".to_string()]); // default fallback

    let meta = api_url.map(|url| SourceMeta {
        name: name.clone(),
        api_url: url,
        api_key: api_key.unwrap_or_default(),
        platforms,
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
            let val = rest.split(&[' ', ';', ',', '\n', '\r'][..]).next().unwrap_or(rest).to_string();
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

/// Search via LxMusic format: POST {API_URL}/search
async fn search_via_lxmusic(
    client: &reqwest::Client,
    meta: &SourceMeta,
    keyword: &str,
) -> Vec<SongResult> {
    let mut results = Vec::new();

    // Try each platform supported by this source
    for platform in &meta.platforms {
        let body = serde_json::json!({
            "keyword": keyword,
            "source": platform,
            "limit": 5,
            "key": meta.api_key,
        });

        // Try POST first, then GET
        let resp = client
            .post(format!("{}/search", meta.api_url.trim_end_matches('/')))
            .json(&body)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;

        if let Ok(r) = resp {
            if let Ok(json) = r.json::<serde_json::Value>().await {
                // LxMusic returns: { data: [{ name, singer, id, ... }] }
                let songs = json.get("data")
                    .or_else(|| json.get("songs"))
                    .or_else(|| json.get("results"))
                    .and_then(|v| v.as_array())
                    .map(|a| a.clone())
                    .unwrap_or_default();

                for item in songs {
                    let title = item.get("name").or_else(|| item.get("title"))
                        .and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let artist = item.get("singer").or_else(|| item.get("artist"))
                        .or_else(|| item.get("author"))
                        .and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !title.is_empty() {
                        results.push(SongResult {
                            title,
                            artist,
                            source: format!("{}-{}", meta.name, platform),
                            source_id: 0,
                            score: 0,
                        });
                    }
                }
            }
        }
    }
    results
}

/// Fetch hot keywords via LxMusic format: POST {API_URL}/hot or GET {API_URL}/hot
async fn hot_via_lxmusic(
    client: &reqwest::Client,
    meta: &SourceMeta,
) -> Vec<String> {
    let mut items = Vec::new();

    for platform in &meta.platforms {
        let base = meta.api_url.trim_end_matches('/');

        // Try POST first
        let body = serde_json::json!({
            "source": platform,
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
                // Various response formats
                let list = json.get("data")
                    .or_else(|| json.get("hots"))
                    .or_else(|| json.get("result"))
                    .and_then(|v| v.as_array());
                if let Some(arr) = list {
                    for item in arr {
                        let keyword = item.get("keyword").or_else(|| item.get("name"))
                            .or_else(|| item.get("title"))
                            .and_then(|v| v.as_str()).unwrap_or("");
                        if !keyword.is_empty() {
                            items.push(keyword.to_string());
                        }
                    }
                }
            }
        }

        if items.len() >= 10 {
            break;
        }

        // Try GET fallback
        if items.is_empty() {
            let url = format!("{}/hot?source={}", base, platform);
            if let Ok(r) = client.get(&url).timeout(std::time::Duration::from_secs(4)).send().await {
                if let Ok(json) = r.json::<serde_json::Value>().await {
                    let list = json.get("data")
                        .or_else(|| json.get("hots"))
                        .and_then(|v| v.as_array());
                    if let Some(arr) = list {
                        for item in arr {
                            let keyword = item.get("keyword").or_else(|| item.get("name"))
                                .and_then(|v| v.as_str()).unwrap_or("");
                            if !keyword.is_empty() {
                                items.push(keyword.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    items
}

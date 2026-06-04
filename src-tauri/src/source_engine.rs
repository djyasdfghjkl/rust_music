use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
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

// ─── Engine state ───
pub struct SourceEngine {
    sources: Mutex<Vec<SourceInfo>>,
    active_source: Mutex<usize>,
    http_client: reqwest::Client,
    source_dir: PathBuf,
}

impl SourceEngine {
    pub fn new(source_dir: PathBuf) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("MikuTunes/1.0")
            .build()
            .unwrap_or_default();

        let engine = Self {
            sources: Mutex::new(Vec::new()),
            active_source: Mutex::new(0),
            http_client,
            source_dir,
        };
        engine.scan_sources();
        engine
    }

    /// Scan 音源 directory for JS source files, parse names, randomize
    pub fn scan_sources(&self) {
        let mut sources = Vec::new();
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

                    // Try to extract @name from JS file (first few lines)
                    let name = extract_source_name(&path).unwrap_or_else(|| file_name.clone());

                    sources.push(SourceInfo {
                        id: idx,
                        name,
                        file_name,
                        score: 0,
                        enabled: true,
                    });
                    idx += 1;
                }
            }
        }

        // Random initial sort
        let mut rng = rand::thread_rng();
        sources.shuffle(&mut rng);

        // Assign new IDs after shuffle
        for (i, s) in sources.iter_mut().enumerate() {
            s.id = i;
        }

        if let Ok(mut current) = self.sources.lock() {
            *current = sources;
        }
    }

    /// Get all sources with their current scores
    pub fn get_sources(&self) -> Vec<SourceInfo> {
        self.sources.lock().unwrap().clone()
    }

    /// Get active source info
    pub fn get_active_source(&self) -> Option<SourceInfo> {
        let active_id = *self.active_source.lock().unwrap();
        let sources = self.sources.lock().unwrap();
        sources.iter().find(|s| s.id == active_id).cloned()
    }

    /// Switch active source to a specific ID
    pub fn switch_source(&self, id: usize) -> bool {
        let sources = self.sources.lock().unwrap();
        if sources.iter().any(|s| s.id == id) {
            *self.active_source.lock().unwrap() = id;
            true
        } else {
            false
        }
    }

    /// Score a source: +1 if found, -1 if not found
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

    /// Search for music by keyword. Tries sources in score order.
    pub async fn search(&self, keyword: &str) -> SearchResponse {
        let start = Instant::now();

        // Try all enabled sources in parallel, sorted by score (highest first)
        let sources = {
            let mut s = self.sources.lock().unwrap().clone();
            s.sort_by(|a, b| b.score.cmp(&a.score));
            s.into_iter()
                .filter(|s| s.enabled)
                .collect::<Vec<_>>()
        };

        if sources.is_empty() {
            return SearchResponse {
                results: vec![],
                total: 0,
                from_source: None,
                elapsed_ms: start.elapsed().as_millis() as u64,
            };
        }

        // Search all sources in parallel
        let client = self.http_client.clone();
        let keyword = keyword.to_string();

        let tasks: Vec<_> = sources
            .into_iter()
            .map(|source| {
                let client = client.clone();
                let kw = keyword.clone();
                tokio::spawn(async move {
                    let result = search_single_source(&client, &source, &kw).await;
                    (source, result)
                })
            })
            .collect();

        let mut all_results = Vec::new();
        let mut found_source: Option<String> = None;

        for task in tasks {
            if let Ok((source, results)) = task.await {
                let found = !results.is_empty();

                // Auto-score based on result
                // (actual scoring via frontend +1/-1 is separate)
                if found && found_source.is_none() {
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
}

/// Try to search a single source using its API pattern
async fn search_single_source(
    client: &reqwest::Client,
    source: &SourceInfo,
    keyword: &str,
) -> Vec<SongResult> {
    let source_key = source.file_name.as_str();

    match source_key {
        "ixiaowo" | "xiaowo" => {
            search_xiaowo(client, keyword).await
        }
        "monster" => {
            search_monster(client, keyword).await
        }
        "sixyin-music-source-v1.2.0-encrypt" | "六音1.2.1版（最高支持无损音质）" | "六音自定义源" => {
            search_sixyin(client, keyword).await
        }
        // Fallback: use a generic iTunes/audius search
        _ => {
            search_fallback(client, keyword).await
        }
    }
}

/// iTunes search API (good fallback for any source)
async fn search_fallback(client: &reqwest::Client, keyword: &str) -> Vec<SongResult> {
    let url = format!(
        "https://itunes.apple.com/search?term={}&limit=10&media=music",
        urlencoding(keyword)
    );

    match client.get(&url).send().await {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let mut results = Vec::new();
                    if let Some(results_arr) = json["results"].as_array() {
                        for item in results_arr {
                            let title = item["trackName"].as_str().unwrap_or("").to_string();
                            let artist = item["artistName"].as_str().unwrap_or("").to_string();
                            if !title.is_empty() {
                                results.push(SongResult {
                                    title,
                                    artist,
                                    source: "iTunes".to_string(),
                                    source_id: 0,
                                    score: 0,
                                });
                            }
                        }
                    }
                    results
                }
                Err(_) => vec![],
            }
        }
        Err(_) => vec![],
    }
}

/// Xiaowo / 小窝 API
async fn search_xiaowo(client: &reqwest::Client, keyword: &str) -> Vec<SongResult> {
    let url = format!("https://api.ixiaowo.com/api/search?keyword={}&type=song", urlencoding(keyword));
    match client.get(&url).send().await {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let mut results = Vec::new();
                    if let Some(data) = json["data"].as_array() {
                        for item in data {
                            results.push(SongResult {
                                title: item["name"].as_str().unwrap_or("").to_string(),
                                artist: item["artist"].as_str().unwrap_or("").to_string(),
                                source: "小窝".to_string(),
                                source_id: 0,
                                score: 0,
                            });
                        }
                    }
                    results
                }
                Err(_) => vec![],
            }
        }
        Err(_) => vec![],
    }
}

/// Monster API (monster)
async fn search_monster(client: &reqwest::Client, keyword: &str) -> Vec<SongResult> {
    let url = format!("https://api.monster.la/search?keyword={}&limit=10", urlencoding(keyword));
    match client.get(&url).send().await {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let mut results = Vec::new();
                    if let Some(songs) = json["songs"].as_array() {
                        for item in songs {
                            results.push(SongResult {
                                title: item["title"].as_str().unwrap_or("").to_string(),
                                artist: item["author"].as_str().unwrap_or("").to_string(),
                                source: "Monster".to_string(),
                                source_id: 0,
                                score: 0,
                            });
                        }
                    }
                    results
                }
                Err(_) => vec![],
            }
        }
        Err(_) => vec![],
    }
}

/// Sixyin API
async fn search_sixyin(client: &reqwest::Client, keyword: &str) -> Vec<SongResult> {
    let url = format!("https://api.6yin.com/search?keyword={}&type=song&limit=10", urlencoding(keyword));
    match client.get(&url).send().await {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let mut results = Vec::new();
                    if let Some(data) = json["data"].as_array() {
                        for item in data {
                            results.push(SongResult {
                                title: item["title"].as_str().unwrap_or("").to_string(),
                                artist: item["singer"].as_str().unwrap_or("").to_string(),
                                source: "六音".to_string(),
                                source_id: 0,
                                score: 0,
                            });
                        }
                    }
                    results
                }
                Err(_) => vec![],
            }
        }
        Err(_) => vec![],
    }
}

/// Simple URL encoding for query params
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => String::from("+"),
            _ => {
                let bytes = c.to_string().into_bytes();
                bytes
                    .iter()
                    .map(|&b| format!("%{:02X}", b))
                    .collect::<String>()
            }
        })
        .collect()
}

/// Extract @name from JS file header comment
fn extract_source_name(path: &std::path::Path) -> Option<String> {
    if let Ok(content) = fs::read_to_string(path) {
        // Look for @name in comment header
        for line in content.lines().take(30) {
            let trimmed = line.trim();
            if let Some(name_val) = trimmed.strip_prefix("* @name ") {
                return Some(name_val.trim().to_string());
            }
            if let Some(name_val) = trimmed.strip_prefix(" * @name ") {
                return Some(name_val.trim().to_string());
            }
            // Also check the source filename for patterns like "sixyin-music-source"
            if trimmed.contains("@name") {
                if let Some(idx) = trimmed.find("@name") {
                    let rest = &trimmed[idx + 6..];
                    if !rest.is_empty() {
                        return Some(rest.trim().to_string());
                    }
                }
            }
        }
        // Try parsing name from first line: // @name xxx
        if let Some(first_line) = content.lines().next() {
            let fl = first_line.trim();
            if fl.starts_with("// @name") || fl.starts_with("//@name") {
                let rest = fl.trim_start_matches("//").trim_start_matches('@')
                    .trim_start_matches("name").trim();
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
            // Also try: // name = "xxx"
            if fl.contains("name") && fl.contains('"') {
                if let Some(start) = fl.find('"') {
                    if let Some(end) = fl[start + 1..].find('"') {
                        return Some(fl[start + 1..start + 1 + end].to_string());
                    }
                }
            }
        }
    }
    None
}

// ─── Shared Types ───

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: usize,
    pub name: String,
    pub file_name: String,
    pub score: i32,
    pub enabled: bool,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SongResult>,
    pub total: usize,
    pub from_source: Option<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchBatchEvent {
    pub token: u64,
    pub results: Vec<SongResult>,
    pub from_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotItem {
    pub title: String,
    pub source: String,
    pub source_id: usize,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FavoriteSong {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub duration: Option<f64>,
    pub source_id: usize,
    pub source: String,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FavoritePlaylist {
    pub id: String,
    pub name: String,
    pub cover: Option<String>,
    pub song_count: Option<usize>,
    pub source_id: usize,
    pub source_name: String,
    pub external_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FavoritesData {
    pub songs: Vec<FavoriteSong>,
    pub playlists: Vec<FavoritePlaylist>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedPlaylist {
    pub playlist: PlaylistInfo,
    pub songs: Vec<SongDetail>,
    pub external_url: String,
    pub note: Option<String>,
}

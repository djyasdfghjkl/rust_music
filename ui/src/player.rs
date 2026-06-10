use crate::tauri_utils::{convert_file_src, invoke};
use crate::types::{FavoriteSong, FavoritesData, SongDetail, SongInfoResult, SongResult};
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[derive(Debug, Clone, PartialEq)]
pub enum PlayMode {
    Sequential,
    Shuffle,
    RepeatOne,
}

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub cover_color: String,
    pub cover_url: Option<String>,
    pub url: Option<String>,
    pub source_id: usize,
    pub song_id: String,
    pub platform: String,
    pub quality: Option<String>,
    pub format: Option<String>,
    pub duration: Option<f64>,
    pub downloaded_path: Option<String>,
}

impl TrackInfo {
    pub fn from_song(title: &str, artist: &str) -> Self {
        Self {
            title: title.to_string(),
            artist: artist.to_string(),
            cover_color: random_color(),
            cover_url: None,
            url: None,
            source_id: 0,
            song_id: String::new(),
            platform: String::new(),
            quality: None,
            format: None,
            duration: None,
            downloaded_path: None,
        }
    }

    pub fn from_search_result(song: &SongResult) -> Self {
        let mut track = Self::from_song(&song.title, &song.artist);
        track.source_id = song.source_id;
        track.song_id = song.id.clone();
        track.platform = song.platform.clone();
        track.cover_url = song.cover_url.clone();
        track.quality = song.quality.clone();
        track.duration = song.duration;
        track
    }

    pub fn from_song_detail(song: &SongDetail) -> Self {
        let mut track = Self::from_song(&song.title, &song.artist);
        track.source_id = song.source_id;
        track.song_id = song.id.clone();
        track.platform = song.platform.clone();
        track.cover_url = song.cover_url.clone();
        track.duration = song.duration;
        track
    }
}

fn random_color() -> String {
    let colors = [
        "linear-gradient(135deg,#39C5BB,#2A9D95)",
        "linear-gradient(135deg,#FF9EC5,#FF6B9D)",
        "linear-gradient(135deg,#6C8BFF,#4A6BDF)",
        "linear-gradient(135deg,#8EDBD5,#39C5BB)",
        "linear-gradient(135deg,#E85555,#B83030)",
        "linear-gradient(135deg,#FFB8D6,#FF9EC5)",
    ];
    let idx = (js_sys::Math::random() * colors.len() as f64) as usize;
    colors[idx.min(colors.len() - 1)].to_string()
}

fn is_mobile_viewport() -> bool {
    web_sys::window()
        .and_then(|window| window.inner_width().ok())
        .and_then(|value| value.as_f64())
        .is_some_and(|width| width <= 900.0)
}

#[derive(Clone)]
pub struct PlayerState {
    pub queue: RwSignal<Vec<TrackInfo>>,
    pub current_index: RwSignal<Option<usize>>,
    pub is_playing: RwSignal<bool>,
    pub is_resolving: RwSignal<bool>,
    pub progress: RwSignal<f64>,
    pub current_time: RwSignal<f64>,
    pub duration: RwSignal<f64>,
    pub volume: RwSignal<f64>,
    pub play_mode: RwSignal<PlayMode>,
    pub show_full_player: RwSignal<bool>,
    pub show_lyrics: RwSignal<bool>,
    pub show_queue: RwSignal<bool>,
    pub song_info: RwSignal<Option<SongInfoResult>>,
    pub last_error: RwSignal<Option<String>>,
    pub is_downloading: RwSignal<bool>,
    pub download_progress: RwSignal<f64>,
    pub download_status: RwSignal<Option<String>>,
    pub download_message: RwSignal<Option<String>>,
    pub last_download_path: RwSignal<Option<String>>,
    pub last_download_track_key: RwSignal<Option<String>>,
    pub lyric_auto_scroll_after: RwSignal<f64>,
    pub spectrum: RwSignal<Vec<f64>>,
}

impl PlayerState {
    pub fn new() -> Self {
        Self {
            queue: RwSignal::new(Vec::new()),
            current_index: RwSignal::new(None),
            is_playing: RwSignal::new(false),
            is_resolving: RwSignal::new(false),
            progress: RwSignal::new(0.0),
            current_time: RwSignal::new(0.0),
            duration: RwSignal::new(0.0),
            volume: RwSignal::new(0.7),
            play_mode: RwSignal::new(PlayMode::Sequential),
            show_full_player: RwSignal::new(false),
            show_lyrics: RwSignal::new(true),
            show_queue: RwSignal::new(false),
            song_info: RwSignal::new(None),
            last_error: RwSignal::new(None),
            is_downloading: RwSignal::new(false),
            download_progress: RwSignal::new(0.0),
            download_status: RwSignal::new(None),
            download_message: RwSignal::new(None),
            last_download_path: RwSignal::new(None),
            last_download_track_key: RwSignal::new(None),
            lyric_auto_scroll_after: RwSignal::new(0.0),
            spectrum: RwSignal::new(vec![0.18; 18]),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct DownloadProgressEvent {
    title: String,
    artist: String,
    progress: f64,
    status: String,
    message: String,
    path: Option<String>,
}

fn track_download_key(title: &str, artist: &str) -> String {
    format!("{}::{}", title.trim(), artist.trim())
}

fn set_download_feedback(
    state: &PlayerState,
    status: &str,
    message: impl Into<String>,
    progress: f64,
    path: Option<String>,
    track_key: Option<String>,
) {
    let status = status.to_string();
    state.is_downloading.set(matches!(status.as_str(), "started" | "progress"));
    state.download_status.set(Some(status));
    state.download_message.set(Some(message.into()));
    state.download_progress.set(progress.clamp(0.0, 1.0));
    state.last_download_path.set(path);
    state.last_download_track_key.set(track_key);
}

fn get_or_create_audio() -> JsValue {
    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();
    let body = document.body().unwrap();
    if let Some(audio) = document.get_element_by_id("__miku_audio") {
        return audio.into();
    }

    let audio = document.create_element("audio").unwrap();
    audio.set_id("__miku_audio");
    let _ = js_sys::Reflect::set(
        &audio,
        &JsValue::from_str("preload"),
        &JsValue::from_str("auto"),
    );
    let _ = js_sys::Reflect::set(
        &audio,
        &JsValue::from_str("playsInline"),
        &JsValue::from_bool(true),
    );
    let _ = audio.set_attribute("playsinline", "true");
    let _ = audio.set_attribute("webkit-playsinline", "true");
    let _ = body.append_child(&audio);
    audio.into()
}

fn create_audio_analyser(_audio: &JsValue) -> Option<(web_sys::AnalyserNode, Vec<u8>)> {
    None
}

macro_rules! audio_call {
    ($method:expr) => {{
        let audio = get_or_create_audio();
        if let Ok(function) = js_sys::Reflect::get(&audio, &JsValue::from_str($method))
            .and_then(|f| Ok(js_sys::Function::from(f)))
        {
            let _ = function.call0(&audio);
        }
    }};
}

macro_rules! audio_get {
    ($prop:expr) => {{
        let audio = get_or_create_audio();
        js_sys::Reflect::get(&audio, &JsValue::from_str($prop)).unwrap_or(JsValue::UNDEFINED)
    }};
}

macro_rules! audio_set {
    ($prop:expr, $value:expr) => {{
        let audio = get_or_create_audio();
        let _ = js_sys::Reflect::set(&audio, &JsValue::from_str($prop), &JsValue::from($value));
    }};
}

pub fn setup_audio_events(state: PlayerState) {
    let audio = get_or_create_audio();
    audio_set!("volume", state.volume.get());
    let analyser_state = StoredValue::new(create_audio_analyser(&audio));

    let add_listener = |event: &str, cb: &Closure<dyn Fn()>| {
        if let Ok(f) = js_sys::Reflect::get(&audio, &JsValue::from_str("addEventListener")) {
            let f = js_sys::Function::from(f);
            let args = js_sys::Array::of3(&JsValue::from_str(event), cb.as_ref(), &JsValue::null());
            let _ = f.apply(&audio, &args);
        }
    };

    let state_time = state.clone();
    let cb_time = Closure::wrap(Box::new(move || {
        let current_time = audio_get!("currentTime").as_f64().unwrap_or(0.0);
        let duration = audio_get!("duration").as_f64().unwrap_or(0.0);
        state_time.current_time.set(current_time);
        state_time.duration.set(duration);
        state_time.progress.set(if duration > 0.0 {
            current_time / duration
        } else {
            0.0
        });
        if let Some((analyser, mut buffer)) = analyser_state.get_value() {
            analyser.get_byte_frequency_data(&mut buffer);
            let len = buffer.len().max(1);
            let mut bars = (0..18)
                .map(|index| {
                    let start = index * len / 18;
                    let end = ((index + 1) * len / 18).max(start + 1);
                    let mut total = 0.0;
                    let mut count: f64 = 0.0;
                    for pos in start..end {
                        total += buffer.get(pos).copied().unwrap_or(0) as f64;
                        count += 1.0;
                    }
                    ((total / count.max(1.0)) / 255.0).clamp(0.06, 1.0)
                })
                .collect::<Vec<_>>();
            if state_time.is_playing.get_untracked() && bars.iter().all(|value| *value <= 0.08) {
                bars = animated_spectrum(current_time);
            }
            state_time.spectrum.set(bars);
        } else if state_time.is_playing.get_untracked() {
            state_time.spectrum.set(animated_spectrum(current_time));
        }
    }) as Box<dyn Fn()>);
    add_listener("timeupdate", &cb_time);
    cb_time.forget();

    let state_loaded = state.clone();
    let cb_loaded = Closure::wrap(Box::new(move || {
        let duration = audio_get!("duration").as_f64().unwrap_or(0.0);
        state_loaded.duration.set(duration);
        state_loaded.is_resolving.set(false);
        state_loaded.is_playing.set(true);
        state_loaded.last_error.set(None);
    }) as Box<dyn Fn()>);
    add_listener("loadedmetadata", &cb_loaded);
    cb_loaded.forget();

    let state_play = state.clone();
    let cb_play = Closure::wrap(Box::new(move || {
        state_play.is_playing.set(true);
        state_play.is_resolving.set(false);
        state_play.last_error.set(None);
    }) as Box<dyn Fn()>);
    add_listener("play", &cb_play);
    cb_play.forget();

    let state_pause = state.clone();
    let cb_pause = Closure::wrap(Box::new(move || {
        state_pause.is_playing.set(false);
    }) as Box<dyn Fn()>);
    add_listener("pause", &cb_pause);
    cb_pause.forget();

    let state_wait = state.clone();
    let cb_wait = Closure::wrap(Box::new(move || {
        state_wait.is_resolving.set(true);
    }) as Box<dyn Fn()>);
    add_listener("waiting", &cb_wait);
    cb_wait.forget();

    let state_error = state.clone();
    let cb_error = Closure::wrap(Box::new(move || {
        state_error.is_playing.set(false);
        state_error.is_resolving.set(false);
        let audio = get_or_create_audio();
        let media_error = js_sys::Reflect::get(&audio, &JsValue::from_str("error"))
            .ok()
            .filter(|value| !value.is_null() && !value.is_undefined());
        let code = media_error
            .as_ref()
            .and_then(|value| js_sys::Reflect::get(value, &JsValue::from_str("code")).ok())
            .and_then(|value| value.as_f64())
            .map(|value| value as i32);
        let current_src = js_sys::Reflect::get(&audio, &JsValue::from_str("src"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default();
        let message = match code {
            Some(4) => "播放失败：当前链接在 Android WebView 中不可用".to_string(),
            Some(3) => "播放失败：音频内容解码失败".to_string(),
            Some(2) => "播放失败：音频网络请求失败".to_string(),
            Some(1) => "播放失败：音频加载被中止".to_string(),
            _ => "播放失败".to_string(),
        };
        web_sys::console::error_1(&JsValue::from_str(&format!(
            "audio error code={:?}, src={}",
            code, current_src
        )));
        state_error.last_error.set(Some(message));
    }) as Box<dyn Fn()>);
    add_listener("error", &cb_error);
    cb_error.forget();

    let state_ended = state.clone();
    let cb_ended = Closure::wrap(Box::new(move || {
        state_ended.is_playing.set(false);
        state_ended.progress.set(0.0);
        state_ended.current_time.set(0.0);
        next_track(state_ended.clone());
    }) as Box<dyn Fn()>);
    add_listener("ended", &cb_ended);
    cb_ended.forget();
}

pub fn setup_download_events(state: PlayerState) {
    wasm_bindgen_futures::spawn_local(async move {
        let handler = Closure::<dyn FnMut(JsValue)>::wrap(Box::new(move |event: JsValue| {
            let payload = js_sys::Reflect::get(&event, &JsValue::from_str("payload"))
                .ok()
                .filter(|value| !value.is_undefined() && !value.is_null())
                .unwrap_or(event);
            let Ok(progress) = serde_wasm_bindgen::from_value::<DownloadProgressEvent>(payload) else {
                return;
            };
            let track_key = track_download_key(&progress.title, &progress.artist);
            set_download_feedback(
                &state,
                &progress.status,
                progress.message.clone(),
                progress.progress,
                progress.path.clone(),
                Some(track_key),
            );
            if progress.status == "success" {
                if let Some(path) = progress.path {
                    let mut queue = state.queue.get_untracked();
                    for track in &mut queue {
                        if track.title == progress.title && track.artist == progress.artist {
                            track.downloaded_path = Some(path.clone());
                        }
                    }
                    state.queue.set(queue);
                }
            }
        }));
        let _ = crate::tauri_utils::listen(
            "download_progress",
            handler.as_ref().unchecked_ref::<js_sys::Function>(),
        )
        .await;
        handler.forget();
    });
}

fn animated_spectrum(time: f64) -> Vec<f64> {
    (0..18)
        .map(|index| {
            let phase = time * 3.2 + index as f64 * 0.47;
            let wave = (phase.sin() * 0.5 + 0.5) * 0.62;
            let beat = ((time * 1.7 + index as f64 * 0.13).cos() * 0.5 + 0.5) * 0.28;
            (0.10 + wave + beat).clamp(0.10, 0.95)
        })
        .collect()
}

pub fn play_url(url: &str) {
    audio_set!("src", url);
    audio_call!("load");
    play_current_audio();
}

fn resume_or_play_url(url: &str) {
    let current_src = audio_get!("src").as_string().unwrap_or_default();
    let same_track = !current_src.is_empty()
        && (current_src == url || current_src.ends_with(url) || url.ends_with(&current_src));
    if same_track {
        play_current_audio();
    } else {
        play_url(url);
    }
}

fn play_current_audio() {
    let audio = get_or_create_audio();
    if let Ok(function) = js_sys::Reflect::get(&audio, &JsValue::from_str("play"))
        .and_then(|f| Ok(js_sys::Function::from(f)))
    {
        if let Ok(value) = function.call0(&audio) {
            if value.is_instance_of::<js_sys::Promise>() {
                let promise = js_sys::Promise::from(value);
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(error) = wasm_bindgen_futures::JsFuture::from(promise).await {
                        web_sys::console::error_2(
                            &JsValue::from_str("audio.play() rejected"),
                            &error,
                        );
                    }
                });
            }
        }
    }
}

pub fn set_volume(vol: f64) {
    audio_set!("volume", vol.clamp(0.0, 1.0));
}

pub fn seek(ratio: f64) {
    let duration = audio_get!("duration").as_f64().unwrap_or(0.0);
    if duration > 0.0 {
        audio_set!("currentTime", duration * ratio.clamp(0.0, 1.0));
    }
}

pub fn seek_to_time(seconds: f64) {
    if seconds.is_finite() && seconds >= 0.0 {
        audio_set!("currentTime", seconds);
    }
}

pub fn next_track(state: PlayerState) {
    let queue = state.queue.get();
    if queue.is_empty() {
        return;
    }
    let Some(current) = state.current_index.get() else {
        return;
    };
    let next = match state.play_mode.get() {
        PlayMode::Sequential => {
            if current + 1 >= queue.len() {
                state.is_playing.set(false);
                return;
            }
            current + 1
        }
        PlayMode::Shuffle => ((js_sys::Math::random() * queue.len() as f64) as usize)
            .min(queue.len().saturating_sub(1)),
        PlayMode::RepeatOne => current,
    };
    play_track_at(state, next, false);
}

pub fn prev_track(state: PlayerState) {
    let queue = state.queue.get();
    if queue.is_empty() {
        return;
    }
    let Some(current) = state.current_index.get() else {
        return;
    };
    let prev = current.saturating_sub(1);
    play_track_at(state, prev, false);
}

pub fn play_track_at(state: PlayerState, index: usize, open_full_player: bool) {
    let Some(track) = state.queue.get().get(index).cloned() else {
        return;
    };
    state.current_index.set(Some(index));
    state.progress.set(0.0);
    state.current_time.set(0.0);
    state.song_info.set(None);
    state.last_error.set(None);
    if open_full_player {
        state.show_full_player.set(true);
    }

    if let Some(url) = track.url.clone() {
        play_url(&url);
        if !track.song_id.is_empty() {
            let state_for_info = state.clone();
            wasm_bindgen_futures::spawn_local(async move {
                fetch_song_info_for_track(
                    &state_for_info,
                    index,
                    track.source_id,
                    &track.song_id,
                    &track.platform,
                )
                .await;
            });
        }
        return;
    }

    if !track.song_id.is_empty() {
        wasm_bindgen_futures::spawn_local(async move {
            play_song_by_id(
                &state,
                index,
                track.source_id,
                &track.song_id,
                &track.platform,
            )
            .await;
        });
    }
}

pub fn replace_queue_and_play(state: PlayerState, songs: Vec<SongResult>, index: usize) {
    let queue = songs
        .iter()
        .map(TrackInfo::from_search_result)
        .collect::<Vec<_>>();
    if queue.is_empty() || index >= queue.len() {
        return;
    }
    state.queue.set(queue);
    state.show_queue.set(false);
    play_track_at(state, index, true);
}

pub fn replace_queue_with_song_details_and_play(
    state: PlayerState,
    songs: Vec<SongDetail>,
    index: usize,
) {
    let queue = songs
        .iter()
        .map(TrackInfo::from_song_detail)
        .collect::<Vec<_>>();
    if queue.is_empty() || index >= queue.len() {
        return;
    }
    state.queue.set(queue);
    state.show_queue.set(false);
    play_track_at(state, index, true);
}

pub fn append_search_results_to_queue(state: PlayerState, songs: Vec<SongResult>) {
    let mut queue = state.queue.get_untracked();
    let was_empty = queue.is_empty();
    queue.extend(songs.iter().map(TrackInfo::from_search_result));
    state.queue.set(queue);
    if was_empty && state.current_index.get_untracked().is_none() {
        state.current_index.set(Some(0));
        state.show_queue.set(true);
    }
}

fn current_track_matches(state: &PlayerState, idx: usize, song_id: &str, platform: &str) -> bool {
    state.current_index.get_untracked() == Some(idx)
        && state
            .queue
            .get_untracked()
            .get(idx)
            .is_some_and(|track| track.song_id == song_id && track.platform == platform)
}

fn playable_url_from_result(result: &crate::types::SongUrlResult) -> String {
    if result.url.contains(":\\")
        || result.url.starts_with("\\\\")
        || result.url.starts_with('/')
    {
        return convert_file_src(&result.url, Some("asset"));
    }

    result.url.clone()
}

fn apply_song_url_result(
    state: &PlayerState,
    idx: usize,
    result: crate::types::SongUrlResult,
) -> Option<String> {
    let playable_url = playable_url_from_result(&result);
    if playable_url.trim().is_empty() {
        state.is_resolving.set(false);
        state.is_playing.set(false);
        audio_call!("pause");
        state.last_error.set(Some("播放链接为空".to_string()));
        return None;
    }
    let mut queue = state.queue.get();
    if let Some(track) = queue.get_mut(idx) {
        track.url = Some(playable_url.clone());
        track.quality = Some(result.quality);
        track.format = Some(result.format);
    }
    state.queue.set(queue);
    Some(playable_url)
}

pub async fn play_song_by_id(
    state: &PlayerState,
    idx: usize,
    source_id: usize,
    song_id: &str,
    platform: &str,
) {
    state.is_resolving.set(true);
    state.is_playing.set(false);
    state.last_error.set(None);
    state.song_info.set(None);

    let args = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &args,
        &JsValue::from_str("sourceId"),
        &JsValue::from_f64(source_id as f64),
    );
    let _ = js_sys::Reflect::set(
        &args,
        &JsValue::from_str("songId"),
        &JsValue::from_str(song_id),
    );
    let _ = js_sys::Reflect::set(
        &args,
        &JsValue::from_str("platform"),
        &JsValue::from_str(platform),
    );

    match invoke("get_song_url", Some(&args)).await {
        Ok(value) => match serde_wasm_bindgen::from_value::<crate::types::SongUrlResult>(value) {
            Ok(result) => {
                if let Some(playable_url) = apply_song_url_result(state, idx, result) {
                    play_url(&playable_url);
                }
            }
            Err(_) => {
                state.is_resolving.set(false);
                state.is_playing.set(false);
                audio_call!("pause");
                state.last_error.set(Some(
                    "播放链接返回格式异常，请重新搜索或切换网络后再试".to_string(),
                ));
            }
        },
        Err(error) => {
            let first_msg = error.as_string().unwrap_or_else(|| {
                format!(
                    "请求播放链接失败：source_id={}, song_id={}, platform={}",
                    source_id, song_id, platform
                )
            });
            state
                .last_error
                .set(Some(format!("{first_msg}，正在尝试其它音源...")));

            if let Some(track) = state.queue.get_untracked().get(idx).cloned() {
                let fallback_args = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &fallback_args,
                    &JsValue::from_str("title"),
                    &JsValue::from_str(&track.title),
                );
                let _ = js_sys::Reflect::set(
                    &fallback_args,
                    &JsValue::from_str("artist"),
                    &JsValue::from_str(&track.artist),
                );
                let _ = js_sys::Reflect::set(
                    &fallback_args,
                    &JsValue::from_str("failedSourceId"),
                    &JsValue::from_f64(source_id as f64),
                );
                let _ = js_sys::Reflect::set(
                    &fallback_args,
                    &JsValue::from_str("failedSongId"),
                    &JsValue::from_str(song_id),
                );
                let _ = js_sys::Reflect::set(
                    &fallback_args,
                    &JsValue::from_str("failedPlatform"),
                    &JsValue::from_str(platform),
                );
                if let Ok(value) = invoke("get_song_url_fallback", Some(&fallback_args)).await {
                    if let Ok(result) =
                        serde_wasm_bindgen::from_value::<crate::types::SongUrlResult>(value)
                    {
                        if let Some(playable_url) = apply_song_url_result(state, idx, result) {
                            state.last_error.set(None);
                            play_url(&playable_url);
                            fetch_song_info_with_args(state, idx, &args).await;
                            return;
                        }
                    }
                }
            }

            state.is_resolving.set(false);
            state.is_playing.set(false);
            audio_call!("pause");
            state.last_error.set(Some(format!(
                "{first_msg}。已尝试其它音源仍失败，请检查网络代理/DNS，或重新搜索后再播放。"
            )));
        }
    }

    fetch_song_info_with_args(state, idx, &args).await;
}

async fn fetch_song_info_for_track(
    state: &PlayerState,
    idx: usize,
    source_id: usize,
    song_id: &str,
    platform: &str,
) {
    let args = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &args,
        &JsValue::from_str("sourceId"),
        &JsValue::from_f64(source_id as f64),
    );
    let _ = js_sys::Reflect::set(
        &args,
        &JsValue::from_str("songId"),
        &JsValue::from_str(song_id),
    );
    let _ = js_sys::Reflect::set(
        &args,
        &JsValue::from_str("platform"),
        &JsValue::from_str(platform),
    );
    fetch_song_info_with_args(state, idx, &args).await;
}

async fn fetch_song_info_with_args(state: &PlayerState, idx: usize, args: &js_sys::Object) {
    let requested_song_id = js_sys::Reflect::get(args, &JsValue::from_str("songId"))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_default();
    let requested_platform = js_sys::Reflect::get(args, &JsValue::from_str("platform"))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_default();
    if let Ok(value) = invoke("get_song_info", Some(args)).await {
        if let Ok(info) = serde_wasm_bindgen::from_value::<crate::types::SongInfoResult>(value) {
            if !current_track_matches(state, idx, &requested_song_id, &requested_platform) {
                return;
            }
            let mut queue = state.queue.get();
            if let Some(track) = queue.get_mut(idx) {
                track.cover_url = info.cover_url.clone();
                track.duration = info.duration.or(track.duration);
            }
            state.queue.set(queue);
            state.song_info.set(Some(info));
        }
    }
}

pub fn format_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "00:00".to_string();
    }
    let total = secs as i32;
    format!("{:02}:{:02}", total / 60, total % 60)
}

#[component]
pub fn PlayerBar(
    state: PlayerState,
    favorites: RwSignal<FavoritesData>,
    on_toggle_favorite: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync>,
) -> impl IntoView {
    let title = move || {
        state
            .current_index
            .get()
            .and_then(|index| state.queue.get().get(index).cloned())
            .map(|track| track.title)
            .unwrap_or_else(|| "未在播放".to_string())
    };
    let artist = move || {
        state
            .current_index
            .get()
            .and_then(|index| state.queue.get().get(index).cloned())
            .map(|track| track.artist)
            .unwrap_or_else(|| "点击歌曲开始播放".to_string())
    };
    let hint = move || {
        if let Some(error) = state.last_error.get() {
            error
        } else if state.is_resolving.get()
            && state.song_info.get().is_none_or(|info| {
                info.lyrics
                    .as_deref()
                    .is_none_or(|lyrics| lyrics.trim().is_empty())
            })
        {
            "正在解析播放链接...".to_string()
        } else {
            format!(
                "{} / {}",
                format_time(state.current_time.get()),
                format_time(state.duration.get())
            )
        }
    };
    let _play_icon = move || {
        if state.is_playing.get() {
            "⏸"
        } else {
            "▶"
        }
    };
    let progress = move || format!("width:{}%", (state.progress.get() * 100.0).round());
    let current_favorite_song = move || {
        state
            .current_index
            .get()
            .and_then(|index| state.queue.get().get(index).cloned())
            .map(favorite_from_track)
    };
    let is_favorited = move || {
        current_favorite_song().is_some_and(|song| {
            favorites
                .get()
                .songs
                .iter()
                .any(|item| same_favorite_song(item, &song))
        })
    };
    let lyric_triplet = move || {
        lyric_window(
            state
                .song_info
                .get()
                .and_then(|info| info.lyrics)
                .unwrap_or_default(),
            state.current_time.get(),
            title(),
            artist(),
        )
    };
    let bar_volume_value = move || ((state.volume.get() * 100.0).round() as i32).to_string();
    let bar_volume_style = move || {
        let value = (state.volume.get() * 100.0).round().clamp(0.0, 100.0);
        format!(
            "background:linear-gradient(to right,#39C5BB 0%,#39C5BB {value}%,rgba(255,255,255,0.18) {value}%,rgba(255,255,255,0.18) 100%);"
        )
    };
    let on_bar_volume = {
        let state = state.clone();
        move |ev: leptos::ev::Event| {
            if let Ok(value) = event_target_value(&ev).parse::<f64>() {
                let volume = (value / 100.0).clamp(0.0, 1.0);
                set_volume(volume);
                state.volume.set(volume);
            }
        }
    };

    let play_click_state = state.clone();
    let on_play_click = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        if play_click_state.queue.get().is_empty() {
            return;
        }
        if play_click_state.is_playing.get() {
            audio_call!("pause");
            play_click_state.is_playing.set(false);
        } else if let Some(index) = play_click_state.current_index.get() {
            if let Some(track) = play_click_state.queue.get().get(index).cloned() {
                if let Some(url) = track.url {
                    resume_or_play_url(&url);
                } else if !track.song_id.is_empty() {
                    let state_clone = play_click_state.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        play_song_by_id(
                            &state_clone,
                            index,
                            track.source_id,
                            &track.song_id,
                            &track.platform,
                        )
                        .await;
                    });
                }
            }
        }
    };
    let prev_click_state = state.clone();
    let on_prev_click = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        prev_track(prev_click_state.clone());
    };
    let next_click_state = state.clone();
    let on_next_click = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        next_track(next_click_state.clone());
    };

    let open_player = {
        let state = state.clone();
        move |_| state.show_full_player.set(true)
    };
    let favorite_click = {
        let on_toggle = on_toggle_favorite.clone();
        move |ev: leptos::ev::MouseEvent| {
            ev.stop_propagation();
            if let Some(song) = current_favorite_song() {
                on_toggle(song);
            }
        }
    };

    view! {
        <div class="player-bar" class:idle=move || state.current_index.get().is_none() on:click=open_player>
            <div class="player-bar-progress" style=progress></div>
            <div class="player-bar-inner">
                <div class="player-bar-info">
                    <span class="player-bar-title">{title}</span>
                    <span class="player-bar-artist">{artist}</span>
                </div>
                <div class="player-bar-controls" on:click=move |ev| ev.stop_propagation()>
                    <button class="pb-btn" on:click=on_prev_click><i class="btn-icon iconfont icon-shangyishoushangyige"></i></button>
                    <button class="pb-btn" on:click=on_play_click>
                        <i class=move || if state.is_playing.get() { "btn-icon iconfont icon-zanting" } else { "btn-icon iconfont icon-bofang" }></i>
                    </button>
                    <button class="pb-btn" on:click=on_next_click><i class="btn-icon iconfont icon-xiayigexiayishou"></i></button>
                    <button class="pb-btn pb-fav" class:active=is_favorited on:click=favorite_click>
                        <i class="btn-icon iconfont icon-shoucang"></i>
                    </button>
                </div>
                <div class="player-bar-time">{hint}</div>
                <div class="player-bar-lyrics">
                    {move || {
                        lyric_triplet()
                            .into_iter()
                            .enumerate()
                            .map(|(idx, line)| {
                                view! { <span class:current=move || idx == 1>{line}</span> }
                            })
                            .collect_view()
                    }}
                </div>
                <input
                    class="player-bar-volume"
                    type="range"
                    min="0"
                    max="100"
                    style=bar_volume_style
                    prop:value=bar_volume_value
                    on:click=move |ev| ev.stop_propagation()
                    on:input=on_bar_volume
                />
                <AudioBars active=state.is_playing spectrum=state.spectrum />
            </div>
        </div>
    }
}

#[component]
fn AudioBars(active: RwSignal<bool>, spectrum: RwSignal<Vec<f64>>) -> impl IntoView {
    view! {
        <div class="audio-bars" class:active=move || active.get()>
            {(0..18)
                .map(|index| {
                    view! {
                        <span style=move || {
                            let values = spectrum.get();
                            let value = values.get(index).copied().unwrap_or(0.18);
                            format!("height:{}px; opacity:{};", 6.0 + value * 24.0, 0.42 + value * 0.58)
                        }></span>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
pub fn FullPlayer(
    state: PlayerState,
    favorites: RwSignal<FavoritesData>,
    on_toggle_favorite: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync>,
) -> impl IntoView {
    let toolbar_expanded = RwSignal::new(!is_mobile_viewport());
    let close_player = {
        let state = state.clone();
        move |_| state.show_full_player.set(false)
    };

    let current_track = move || {
        state
            .current_index
            .get()
            .and_then(|index| state.queue.get().get(index).cloned())
    };
    let current_track_key = move || {
        current_track()
            .map(|track| track_download_key(&track.title, &track.artist))
            .unwrap_or_default()
    };

    let title = move || current_track().map(|track| track.title).unwrap_or_default();
    let artist = move || {
        current_track()
            .map(|track| track.artist)
            .unwrap_or_default()
    };
    let quality = move || {
        current_track()
            .and_then(|track| track.quality)
            .unwrap_or_else(|| "320k".to_string())
    };
    let format_label = move || {
        current_track()
            .and_then(|track| track.format)
            .unwrap_or_else(|| "mp3".to_string())
    };
    let cover = move || {
        current_track()
            .map(|track| track.cover_color)
            .unwrap_or_else(|| "#0D0D1A".to_string())
    };
    let cover_url = move || {
        state
            .song_info
            .get()
            .and_then(|info| info.cover_url)
            .or_else(|| current_track().and_then(|track| track.cover_url))
            .unwrap_or_default()
    };
    let current_favorite_song = move || current_track().map(favorite_from_track);
    let is_favorited = move || {
        current_favorite_song().is_some_and(|song| {
            favorites
                .get()
                .songs
                .iter()
                .any(|item| same_favorite_song(item, &song))
        })
    };
    let album = move || {
        state
            .song_info
            .get()
            .and_then(|info| info.album)
            .unwrap_or_default()
    };
    let lyrics = move || {
        state
            .song_info
            .get()
            .and_then(|info| info.lyrics)
            .unwrap_or_default()
    };
    Effect::new(move |_| {
        let _ = state.current_time.get();
        schedule_center_current_lyric(state.lyric_auto_scroll_after.get());
    });
    let info_text = move || {
        if let Some(error) = state.last_error.get() {
            error
        } else if state.is_resolving.get()
            && state.current_index.get().is_none()
            && state.song_info.get().is_none()
        {
            "正在加载歌曲和歌词...".to_string()
        } else {
            String::new()
        }
    };
    let progress_slider_value =
        move || ((state.progress.get().clamp(0.0, 1.0) * 1000.0).round() as i32).to_string();
    let progress_slider_style = move || {
        let value = (state.progress.get().clamp(0.0, 1.0) * 100.0).round();
        format!(
            "background:linear-gradient(to right,#39C5BB 0%,#39C5BB {value}%,rgba(255,255,255,0.12) {value}%,rgba(255,255,255,0.12) 100%);"
        )
    };
    let download_progress = move || {
        format!(
            "width:{}%",
            (state.download_progress.get().clamp(0.0, 1.0) * 100.0).round()
        )
    };
    let volume_value = move || ((state.volume.get() * 100.0).round() as i32).to_string();
    let volume_style = move || {
        let value = (state.volume.get() * 100.0).round().clamp(0.0, 100.0);
        format!(
            "background:linear-gradient(to right,#39C5BB 0%,#39C5BB {value}%,rgba(255,255,255,0.18) {value}%,rgba(255,255,255,0.18) 100%);"
        )
    };
    let mode_label = move || match state.play_mode.get() {
        PlayMode::Sequential => "顺序",
        PlayMode::Shuffle => "随机",
        PlayMode::RepeatOne => "单曲",
    };
    let current_download_path = move || current_track().and_then(|track| track.downloaded_path);
    let show_download_notice = move || {
        state.is_downloading.get()
            || state.download_message.get().is_some()
                && state
                    .last_download_track_key
                    .get()
                    .is_some_and(|key| key == current_track_key())
    };
    let download_notice_class = move || {
        match state.download_status.get().as_deref() {
            Some("success") => "success",
            Some("error") => "error",
            _ => "active",
        }
    };

    let on_progress_input = {
        let state = state.clone();
        move |ev: leptos::ev::Event| {
            let Ok(value) = event_target_value(&ev).parse::<f64>() else {
                return;
            };
            let ratio = (value / 1000.0).clamp(0.0, 1.0);
            let should_resume = state.is_playing.get_untracked();
            let current_url = state
                .current_index
                .get_untracked()
                .and_then(|index| state.queue.get_untracked().get(index).and_then(|track| track.url.clone()));
            seek(ratio);
            if should_resume {
                let current_src = audio_get!("src").as_string().unwrap_or_default();
                if current_src.is_empty() {
                    if let Some(url) = current_url {
                        play_url(&url);
                    }
                } else {
                    play_current_audio();
                }
            }
        }
    };

    let on_volume = {
        let state = state.clone();
        move |ev: leptos::ev::Event| {
            if let Ok(value) = event_target_value(&ev).parse::<f64>() {
                let volume = (value / 100.0).clamp(0.0, 1.0);
                set_volume(volume);
                state.volume.set(volume);
            }
        }
    };

    let toggle_play_handler = {
        let state = state.clone();
        move |_| {
            if state.is_playing.get() {
                audio_call!("pause");
                state.is_playing.set(false);
            } else if let Some(track) = current_track() {
                if let Some(url) = track.url {
                    resume_or_play_url(&url);
                } else if let Some(index) = state.current_index.get() {
                    play_track_at(state.clone(), index, false);
                }
            }
        }
    };

    let prev_handler = {
        let state = state.clone();
        move |_| prev_track(state.clone())
    };

    let next_handler = {
        let state = state.clone();
        move |_| next_track(state.clone())
    };

    let toggle_mode = {
        let state = state.clone();
        move |_| {
            state.play_mode.update(|mode| {
                *mode = match mode {
                    PlayMode::Sequential => PlayMode::Shuffle,
                    PlayMode::Shuffle => PlayMode::RepeatOne,
                    PlayMode::RepeatOne => PlayMode::Sequential,
                };
            });
        }
    };

    let toggle_lyrics = {
        let state = state.clone();
        move |_| state.show_lyrics.update(|show| *show = !*show)
    };
    let pause_lyric_auto_scroll = {
        let state = state.clone();
        move || {
            state
                .lyric_auto_scroll_after
                .set(js_sys::Date::now() + 3500.0);
        }
    };
    let on_lyrics_scroll = {
        let pause = pause_lyric_auto_scroll.clone();
        move |_: leptos::ev::Event| pause()
    };
    let on_lyrics_hover = {
        let pause = pause_lyric_auto_scroll.clone();
        move |_: leptos::ev::MouseEvent| pause()
    };

    let toggle_queue = {
        let state = state.clone();
        move |_| state.show_queue.update(|show| *show = !*show)
    };
    let toggle_toolbar = move |_| {
        toolbar_expanded.update(|open| *open = !*open);
    };
    let toggle_favorite = {
        let on_toggle = on_toggle_favorite.clone();
        move |_| {
            if let Some(song) = current_favorite_song() {
                on_toggle(song);
            }
        }
    };
    let copy_link = {
        let state = state.clone();
        move |_| {
            let url = current_track()
                .and_then(|track| track.url)
                .unwrap_or_default();
            if url.is_empty() {
                state.last_error.set(Some("暂无播放链接可复制".to_string()));
                return;
            }
            if let Some(window) = web_sys::window() {
                let _ = window.navigator().clipboard().write_text(&url);
            }
            state.last_error.set(Some("播放链接已复制".to_string()));
        }
    };
    let download_current = {
        let state = state.clone();
        move |_| {
            let Some(track) = current_track() else {
                set_download_feedback(
                    &state,
                    "error",
                    "暂无歌曲可下载",
                    0.0,
                    None,
                    None,
                );
                return;
            };
            let Some(url) = track.url.clone() else {
                set_download_feedback(
                    &state,
                    "error",
                    "暂无播放链接可下载",
                    0.0,
                    None,
                    Some(track_download_key(&track.title, &track.artist)),
                );
                return;
            };
            set_download_feedback(
                &state,
                "started",
                format!("开始下载《{}》", track.title),
                0.0,
                None,
                Some(track_download_key(&track.title, &track.artist)),
            );
            let args = js_sys::Object::new();
            let _ =
                js_sys::Reflect::set(&args, &JsValue::from_str("url"), &JsValue::from_str(&url));
            let _ = js_sys::Reflect::set(
                &args,
                &JsValue::from_str("title"),
                &JsValue::from_str(&track.title),
            );
            let _ = js_sys::Reflect::set(
                &args,
                &JsValue::from_str("artist"),
                &JsValue::from_str(&track.artist),
            );
            let _ = js_sys::Reflect::set(
                &args,
                &JsValue::from_str("format"),
                &JsValue::from_str(track.format.as_deref().unwrap_or("mp3")),
            );
            let state = state.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match invoke("download_song", Some(&args)).await {
                    Ok(value) => {
                        if let Some(path) = value.as_string() {
                            let mut queue = state.queue.get_untracked();
                            for queued in &mut queue {
                                if queued.title == track.title && queued.artist == track.artist {
                                    queued.downloaded_path = Some(path.clone());
                                }
                            }
                            state.queue.set(queue);
                        }
                    }
                    Err(error) => {
                        if state.download_status.get_untracked().as_deref() != Some("error") {
                            set_download_feedback(
                                &state,
                                "error",
                                error.as_string().unwrap_or_else(|| "下载失败".to_string()),
                                0.0,
                                None,
                                Some(track_download_key(&track.title, &track.artist)),
                            );
                        }
                    }
                }
            });
        }
    };
    let reveal_download_action: std::sync::Arc<dyn Fn() + Send + Sync> = {
        let state = state.clone();
        std::sync::Arc::new(move || {
            let Some(path) = current_track().and_then(|track| track.downloaded_path) else {
                set_download_feedback(&state, "error", "当前歌曲尚未下载", 0.0, None, None);
                return;
            };
            let args = js_sys::Object::new();
            let _ =
                js_sys::Reflect::set(&args, &JsValue::from_str("path"), &JsValue::from_str(&path));
            let state = state.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(error) = invoke("reveal_in_folder", Some(&args)).await {
                    set_download_feedback(
                        &state,
                        "error",
                        error.as_string().unwrap_or_else(|| "打开文件夹失败".to_string()),
                        0.0,
                        Some(path),
                        state.last_download_track_key.get_untracked(),
                    );
                }
            });
        })
    };

    let queue_state = state.clone();
    let queue_view = move || {
        queue_state
            .queue
            .get()
            .into_iter()
            .enumerate()
            .map(|(index, track)| {
                let state_for_current = queue_state.clone();
                let current = move || state_for_current.current_index.get() == Some(index);
                let state_click = queue_state.clone();
                let on_click = move |_| {
                    play_track_at(state_click.clone(), index, false);
                };
                view! {
                    <div class="fp-queue-item" class:current=current on:click=on_click>
                        <span class="fqi-title">{track.title}</span>
                        <span class="fqi-artist">{track.artist}</span>
                        <span class="fqi-meta">
                            {track.duration.map(format_time).unwrap_or_else(|| "--:--".to_string())}
                            " · "
                            {track.quality.unwrap_or_else(|| "未知音质".to_string())}
                            " · "
                            {track.platform.to_uppercase()}
                        </span>
                    </div>
                }
            })
            .collect_view()
    };

    view! {
        <div class="full-player-overlay" on:click=close_player>
            <div class="full-player" on:click=move |ev| ev.stop_propagation()>
                <div class="fp-background" style=move || format!("background:{}", cover())></div>
                <div class="fp-backdrop"></div>

                <div class="fp-toolbar-shell">
                    <div class="fp-toolbar-summary">
                        <button class="fp-tool-btn fp-tool-btn-back" on:click=close_player><i class="btn-icon iconfont icon-31fanhui1"></i><span>"返回"</span></button>
                        <button class="fp-tool-btn fp-toolbar-toggle" class:active=move || toolbar_expanded.get() on:click=toggle_toolbar>
                            <i class="btn-icon iconfont icon-add-s"></i>
                            <span>{move || if toolbar_expanded.get() { "收起操作" } else { "展开操作" }}</span>
                        </button>
                    </div>
                    <div class="fp-toolbar-actions" style=move || if toolbar_expanded.get() { "" } else { "display:none;" }>
                        <button class="fp-tool-btn" class:active=move || state.show_lyrics.get() on:click=toggle_lyrics><i class="btn-icon iconfont icon-geci"></i><span>"歌词"</span></button>
                        <button class="fp-tool-btn" class:active=move || state.show_queue.get() on:click=toggle_queue><i class="btn-icon iconfont icon-gedan"></i><span>"队列"</span></button>
                        <button class="fp-tool-btn" on:click=toggle_mode><i class="btn-icon iconfont icon-shunxubofang"></i><span>{mode_label}</span></button>
                        <button class="fp-tool-btn fp-fav-btn" class:active=is_favorited on:click=toggle_favorite>
                            <i class="btn-icon iconfont icon-shoucang"></i>
                            <span>{move || if is_favorited() { "已收藏" } else { "收藏" }}</span>
                        </button>
                        <button class="fp-tool-btn" on:click=copy_link><i class="btn-icon iconfont icon-fuzhilianjie"></i><span>"复制链接"</span></button>
                        <button class="fp-tool-btn" on:click=download_current><i class="btn-icon iconfont icon-xiazai"></i><span>"下载"</span></button>
                        <button
                            class="fp-tool-btn"
                            style=move || if current_download_path().is_some() { "" } else { "display:none;" }
                            on:click={
                                let reveal_download_action = reveal_download_action.clone();
                                move |_| (reveal_download_action)()
                            }
                        ><i class="btn-icon iconfont icon-wenjianjia"></i><span>"打开文件夹"</span></button>
                    </div>
                </div>
                <div
                    class=move || format!("fp-download-notice {}", download_notice_class())
                    style=move || if show_download_notice() { "" } else { "display:none;" }
                >
                    <div class="fp-download-meta">
                        <span>{move || state.download_message.get().unwrap_or_default()}</span>
                        <button
                            class="fp-download-open-btn"
                            style=move || {
                                if current_download_path().is_some()
                                    && state.download_status.get().as_deref() == Some("success")
                                {
                                    ""
                                } else {
                                    "display:none;"
                                }
                            }
                            on:click={
                                let reveal_download_action = reveal_download_action.clone();
                                move |_| (reveal_download_action)()
                            }
                        >"打开文件夹"</button>
                    </div>
                    <div class="fp-download-track">
                        <div class="fp-download-fill" style=download_progress></div>
                    </div>
                </div>

                <div class="fp-main-area">
                    <div class="fp-left-col">
                        <div class="fp-song-info">
                            <div class="fp-cover-art" style=move || format!("background:{}", cover())>
                                <Show when=move || !cover_url().is_empty()>
                                    <img
                                        class="fp-cover-img"
                                        src=cover_url
                                        alt="cover"
                                        on:error=move |ev| {
                                            let element = event_target::<web_sys::HtmlElement>(&ev);
                                            let _ = element.style().set_property("display", "none");
                                        }
                                    />
                                </Show>
                                <i class="fp-cover-icon iconfont icon-yinle1"></i>
                            </div>
                            <h1 class="fp-title">{title}</h1>
                            <p class="fp-artist">{artist}</p>
                            <div class="fp-track-tags">
                                <span>{move || quality()}</span>
                                <span>{move || format_label().to_uppercase()}</span>
                                <span>{move || current_track().map(|track| track.platform).unwrap_or_default()}</span>
                            </div>
                            <Show when=move || !album().is_empty()>
                                <p class="fp-album">{album}</p>
                            </Show>
                            <Show when=move || !info_text().is_empty()>
                                <p class="fp-album">{info_text}</p>
                            </Show>
                        </div>

                        <div class="fp-controls">
                            <div class="fp-progress-area">
                                <input
                                    class="fp-progress-slider"
                                    type="range"
                                    min="0"
                                    max="1000"
                                    style=progress_slider_style
                                    prop:value=progress_slider_value
                                    on:input=on_progress_input
                                />
                                <div class="fp-time-display">
                                    <span>{move || format_time(state.current_time.get())}</span>
                                    <span>{move || format_time(state.duration.get())}</span>
                                </div>
                            </div>

                            <div class="fp-buttons">
                                <button class="fp-btn" title="上一首" on:click=prev_handler><i class="btn-icon iconfont icon-shangyishoushangyige"></i></button>
                                <button class="fp-btn fp-btn-play" on:click=toggle_play_handler>
                                    <i class=move || if state.is_playing.get() { "btn-icon iconfont icon-zanting" } else { "btn-icon iconfont icon-bofang" }></i>
                                </button>
                                <button class="fp-btn" title="下一首" on:click=next_handler><i class="btn-icon iconfont icon-xiayigexiayishou"></i></button>
                                <div class="fp-volume inline">
                                    <i class="fp-vol-icon iconfont icon-yinliang"></i>
                                    <input
                                        type="range"
                                        min="0"
                                        max="100"
                                        class="fp-volume-slider"
                                        style=volume_style
                                        prop:value=volume_value
                                        on:input=on_volume
                                    />
                                </div>
                            </div>
                        </div>
                    </div>

                    <Show when=move || state.show_lyrics.get()>
                        <div class="fp-lyrics-immersive">
                            <div class="fp-lyrics-content" on:scroll=on_lyrics_scroll on:mouseenter=on_lyrics_hover>
                                {move || render_lyrics(lyrics(), state.current_time.get(), title(), artist())}
                            </div>
                        </div>
                    </Show>

                    <div class="fp-right-col" class:open=move || state.show_queue.get()>
                        <Show when=move || state.show_lyrics.get()>
                            <div class="fp-lyrics-panel">
                                <h3 class="fp-panel-title">"歌词"</h3>
                                <div class="fp-lyrics-content">
                                    {move || {
                                        if !lyrics().is_empty() {
                                            lyrics()
                                                .lines()
                                                .map(|line| {
                                                    let line = if line.trim().is_empty() { " ".to_string() } else { line.to_string() };
                                                    view! { <div class="fp-lyric-line">{line}</div> }.into_any()
                                                })
                                                .collect::<Vec<_>>()
                                        } else {
                                            vec![
                                                view! { <div class="fp-lyric-line current">"正在播放..."</div> }.into_any(),
                                                view! { <div class="fp-lyric-line">{title}</div> }.into_any(),
                                                view! { <div class="fp-lyric-line">{artist}</div> }.into_any(),
                                                view! { <div class="fp-lyric-line dim">"暂无歌词"</div> }.into_any(),
                                            ]
                                        }
                                    }}
                                </div>
                            </div>
                        </Show>

                        <div class="fp-queue-panel">
                            <h3 class="fp-panel-title">"播放队列"</h3>
                            <div class="fp-queue-list">
                                {queue_view}
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    }
}

fn parse_lrc_time(input: &str) -> Option<f64> {
    let (minutes, rest) = input.split_once(':')?;
    Some(minutes.parse::<f64>().ok()? * 60.0 + rest.parse::<f64>().ok()?)
}

fn parse_lrc_line(line: &str) -> Option<(f64, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find(']')?;
    let time = parse_lrc_time(&trimmed[1..end])?;
    let text = trimmed[end + 1..].trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some((time, text))
}

fn render_lyrics(
    raw: String,
    current_time: f64,
    title: String,
    artist: String,
) -> Vec<leptos::prelude::AnyView> {
    let parsed = raw.lines().filter_map(parse_lrc_line).collect::<Vec<_>>();
    if parsed.is_empty() {
        let plain = raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if plain.is_empty() {
            return vec![
                view! { <div class="fp-lyric-line current">"正在播放"</div> }.into_any(),
                view! { <div class="fp-lyric-line">{title}</div> }.into_any(),
                view! { <div class="fp-lyric-line dim">{artist}</div> }.into_any(),
                view! { <div class="fp-lyric-line dim">"暂无歌词"</div> }.into_any(),
            ];
        }
        return plain
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                view! {
                    <div class="fp-lyric-line" class:current=move || index == 0>
                        <span class="fp-lyric-text">{line}</span>
                    </div>
                }
                .into_any()
            })
            .collect();
    }

    let current = parsed
        .iter()
        .enumerate()
        .rev()
        .find(|(_, (time, _))| *time <= current_time + 0.25)
        .map(|(index, _)| index)
        .unwrap_or(0);

    parsed
        .into_iter()
        .enumerate()
        .map(|(index, (time, line))| {
            let jump_time = time;
            let jump_click = move |ev: leptos::ev::MouseEvent| {
                ev.stop_propagation();
                seek_to_time(jump_time);
            };
            let jump_double = move |_| {
                seek_to_time(time);
            };
            view! {
                <div class="fp-lyric-line timed" class:current=move || index == current on:dblclick=jump_double>
                    <span class="fp-lyric-text">{line}</span>
                    <button class="fp-lyric-play" on:click=jump_click>"▶"</button>
                </div>
            }
                .into_any()
        })
        .collect()
}

fn lyric_window(raw: String, current_time: f64, title: String, artist: String) -> Vec<String> {
    let parsed = raw.lines().filter_map(parse_lrc_line).collect::<Vec<_>>();
    if parsed.is_empty() {
        return vec![
            title,
            "正在播放".to_string(),
            if artist.is_empty() {
                "暂无歌词".to_string()
            } else {
                artist
            },
        ];
    }

    let current = parsed
        .iter()
        .enumerate()
        .rev()
        .find(|(_, (time, _))| *time <= current_time + 0.25)
        .map(|(index, _)| index)
        .unwrap_or(0);

    let prev = current
        .checked_sub(1)
        .and_then(|index| parsed.get(index).map(|(_, line)| line.clone()))
        .unwrap_or_default();
    let now = parsed
        .get(current)
        .map(|(_, line)| line.clone())
        .unwrap_or_else(|| "正在播放".to_string());
    let next = parsed
        .get(current + 1)
        .map(|(_, line)| line.clone())
        .unwrap_or_default();
    vec![prev, now, next]
}

fn schedule_center_current_lyric(auto_scroll_after: f64) {
    if js_sys::Date::now() < auto_scroll_after {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };

    let callback = Closure::once(move || {
        center_current_lyric();
    });

    if window
        .request_animation_frame(callback.as_ref().unchecked_ref())
        .is_err()
    {
        center_current_lyric();
    }
    callback.forget();
}

fn center_current_lyric() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let (Ok(Some(line)), Ok(Some(container))) = (
        document.query_selector(".fp-lyrics-immersive .fp-lyric-line.current"),
        document.query_selector(".fp-lyrics-immersive .fp-lyrics-content"),
    ) else {
        return;
    };

    let line_el: web_sys::HtmlElement = line.unchecked_into();
    let container_el: web_sys::HtmlElement = container.unchecked_into();
    let Some(line_rect) = element_rect(&line_el) else {
        return;
    };
    let Some(container_rect) = element_rect(&container_el) else {
        return;
    };
    let delta = line_rect.0 + line_rect.1 / 2.0 - (container_rect.0 + container_rect.1 / 2.0);
    let target = container_el.scroll_top() as f64 + delta;
    container_el.set_scroll_top(target.max(0.0).round() as i32);
}

fn element_rect(element: &web_sys::HtmlElement) -> Option<(f64, f64)> {
    let rect = js_sys::Reflect::get(element, &JsValue::from_str("getBoundingClientRect"))
        .ok()
        .and_then(|function| js_sys::Function::from(function).call0(element).ok())?;
    let top = js_sys::Reflect::get(&rect, &JsValue::from_str("top"))
        .ok()
        .and_then(|value| value.as_f64())?;
    let height = js_sys::Reflect::get(&rect, &JsValue::from_str("height"))
        .ok()
        .and_then(|value| value.as_f64())?;
    Some((top, height))
}

fn favorite_from_track(track: TrackInfo) -> FavoriteSong {
    FavoriteSong {
        id: track.song_id,
        title: track.title,
        artist: track.artist,
        album: None,
        cover_url: track.cover_url,
        duration: None,
        source_id: track.source_id,
        source: track.platform.clone(),
        platform: track.platform,
    }
}

fn same_favorite_song(left: &FavoriteSong, right: &FavoriteSong) -> bool {
    if !left.id.is_empty() || !right.id.is_empty() {
        left.id == right.id && left.platform == right.platform
    } else {
        left.title == right.title && left.artist == right.artist
    }
}

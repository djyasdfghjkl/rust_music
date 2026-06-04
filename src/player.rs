use leptos::prelude::*;
use wasm_bindgen::prelude::*;

// ─── Types ───

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
    pub url: Option<String>,
    pub source: String,
}

impl TrackInfo {
    pub fn from_song(title: &str, artist: &str, source: &str) -> Self {
        Self {
            title: title.to_string(),
            artist: artist.to_string(),
            cover_color: random_color(),
            url: None,
            source: source.to_string(),
        }
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

// ─── Reactive State ───

#[derive(Clone)]
pub struct PlayerState {
    pub queue: RwSignal<Vec<TrackInfo>>,
    pub current_index: RwSignal<Option<usize>>,
    pub is_playing: RwSignal<bool>,
    pub progress: RwSignal<f64>,      // 0.0 ~ 1.0
    pub current_time: RwSignal<f64>,  // seconds
    pub duration: RwSignal<f64>,      // seconds
    pub volume: RwSignal<f64>,        // 0.0 ~ 1.0
    pub play_mode: RwSignal<PlayMode>,
    pub show_full_player: RwSignal<bool>,
    pub show_lyrics: RwSignal<bool>,
}

impl PlayerState {
    pub fn new() -> Self {
        Self {
            queue: RwSignal::new(Vec::new()),
            current_index: RwSignal::new(None),
            is_playing: RwSignal::new(false),
            progress: RwSignal::new(0.0),
            current_time: RwSignal::new(0.0),
            duration: RwSignal::new(0.0),
            volume: RwSignal::new(0.7),
            play_mode: RwSignal::new(PlayMode::Sequential),
            show_full_player: RwSignal::new(false),
            show_lyrics: RwSignal::new(false),
        }
    }

    pub fn current_track(&self) -> Signal<Option<TrackInfo>> {
        let queue = self.queue.clone();
        let current_index = self.current_index.clone();
        Signal::derive(move || {
            let idx = current_index.get()?;
            queue.get().get(idx).cloned()
        })
    }
}

// ─── Audio Engine (raw JS via js_sys) ───

fn get_or_create_audio() -> JsValue {
    let w = web_sys::window().unwrap();
    let doc = w.document().unwrap();
    let body = doc.body().unwrap();
    let existing = doc.get_element_by_id("__miku_audio");
    match existing {
        Some(el) => el.into(),
        None => {
            let audio = doc.create_element("audio").unwrap();
            audio.set_id("__miku_audio");
            js_sys::Reflect::set(&audio, &JsValue::from_str("crossOrigin"), &JsValue::from_str("anonymous")).ok();
            body.append_child(&audio).ok();
            audio.into()
        }
    }
}

// Helper to call methods on the audio element
macro_rules! audio_call {
    ($method:expr $(, $arg:expr)*) => {{
        let audio = get_or_create_audio();
        let func = js_sys::Function::from(
            js_sys::Reflect::get(&audio, &JsValue::from_str($method)).unwrap()
        );
        let _ = func.call0(&audio);
    }};
}

macro_rules! audio_get {
    ($prop:expr) => {{
        let audio = get_or_create_audio();
        js_sys::Reflect::get(&audio, &JsValue::from_str($prop)).unwrap_or(JsValue::UNDEFINED)
    }};
}

macro_rules! audio_set {
    ($prop:expr, $val:expr) => {{
        let audio = get_or_create_audio();
        js_sys::Reflect::set(&audio, &JsValue::from_str($prop), &JsValue::from($val)).ok();
    }};
}

pub fn setup_audio_events(state: PlayerState) {
    let audio = get_or_create_audio();
    audio_set!("volume", state.volume.get());

    // Helper to add event listener
    let add_listener = |event: &str, cb: &Closure<dyn Fn()>| {
        let add_evt = js_sys::Function::from(
            js_sys::Reflect::get(&audio, &JsValue::from_str("addEventListener")).unwrap()
        );
        let args = js_sys::Array::of3(&JsValue::from_str(event), cb.as_ref(), &JsValue::null());
        let _ = add_evt.apply(&audio, &args);
    };

    // Timeupdate
    let s = state.clone();
    let cb1 = Closure::wrap(Box::new(move || {
        let ct = audio_get!("currentTime").as_f64().unwrap_or(0.0);
        let dur = audio_get!("duration").as_f64().unwrap_or(0.0);
        if dur > 0.0 && dur.is_finite() {
            s.current_time.set(ct);
            s.duration.set(dur);
            s.progress.set(ct / dur);
        }
    }) as Box<dyn Fn()>);
    add_listener("timeupdate", &cb1);
    std::mem::forget(cb1);

    // Ended
    let s = state.clone();
    let cb2 = Closure::wrap(Box::new(move || {
        s.is_playing.set(false);
        s.progress.set(0.0);
        s.current_time.set(0.0);
        next_track(s.clone());
    }) as Box<dyn Fn()>);
    add_listener("ended", &cb2);
    std::mem::forget(cb2);

    // Loadedmetadata
    let s = state.clone();
    let cb3 = Closure::wrap(Box::new(move || {
        let dur = audio_get!("duration").as_f64().unwrap_or(0.0);
        s.duration.set(dur);
        s.is_playing.set(true);
        audio_call!("play");
    }) as Box<dyn Fn()>);
    add_listener("loadedmetadata", &cb3);
    std::mem::forget(cb3);

    // Error
    let s = state.clone();
    let cb4 = Closure::wrap(Box::new(move || {
        s.is_playing.set(false);
        next_track(s.clone());
    }) as Box<dyn Fn()>);
    add_listener("error", &cb4);
    std::mem::forget(cb4);
}

pub fn play_url(url: &str) {
    audio_set!("src", url);
    audio_call!("play");
}

pub fn toggle_play() {
    let audio = get_or_create_audio();
    let paused = js_sys::Reflect::get(&audio, &JsValue::from_str("paused"))
        .unwrap_or(JsValue::TRUE);
    if paused.is_truthy() {
        audio_call!("play");
    } else {
        audio_call!("pause");
    }
}

pub fn set_volume(vol: f64) {
    audio_set!("volume", vol.min(1.0).max(0.0));
}

pub fn seek(ratio: f64) {
    let dur = audio_get!("duration").as_f64().unwrap_or(0.0);
    if dur > 0.0 && dur.is_finite() {
        audio_set!("currentTime", dur * ratio.min(1.0).max(0.0));
    }
}

pub fn next_track(state: PlayerState) {
    let queue = state.queue.get();
    if queue.is_empty() { return; }
    let mode = state.play_mode.get();
    let next_idx = match mode {
        PlayMode::RepeatOne => state.current_index.get().unwrap_or(0),
        PlayMode::Shuffle => {
            let r = (js_sys::Math::random() * queue.len() as f64) as usize;
            r.min(queue.len() - 1)
        }
        PlayMode::Sequential => {
            let cur = state.current_index.get().unwrap_or(0);
            if cur + 1 >= queue.len() { 0 } else { cur + 1 }
        }
    };
    state.current_index.set(Some(next_idx));
    if let Some(track) = queue.get(next_idx) {
        if let Some(ref url) = track.url {
            play_url(url);
            state.is_playing.set(true);
        }
    }
}

pub fn prev_track(state: PlayerState) {
    let queue = state.queue.get();
    if queue.is_empty() { return; }
    let cur = state.current_index.get().unwrap_or(0);
    let prev_idx = if cur == 0 { queue.len() - 1 } else { cur - 1 };
    state.current_index.set(Some(prev_idx));
    if let Some(track) = queue.get(prev_idx) {
        if let Some(ref url) = track.url {
            play_url(url);
            state.is_playing.set(true);
        }
    }
}

pub fn format_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "00:00".to_string();
    }
    let total = secs as i32;
    let m = total / 60;
    let s = total % 60;
    format!("{:02}:{:02}", m, s)
}

// ─── PlayerBar Component ───

#[component]
pub fn PlayerBar(
    state: PlayerState,
) -> impl IntoView {
    let title = move || {
        let idx = state.current_index.get();
        idx.and_then(|i| state.queue.get().get(i).cloned())
            .map_or("未在播放".to_string(), |t| t.title)
    };
    let artist = move || {
        let idx = state.current_index.get();
        idx.and_then(|i| state.queue.get().get(i).cloned())
            .map_or("点击选择歌曲".to_string(), |t| t.artist)
    };
    let is_playing = move || state.is_playing.get();
    let play_icon = move || if is_playing() { "⏸" } else { "▶" };
    let pct = move || (state.progress.get() * 100.0) as i32;

    let on_play_click = move |e: leptos::ev::MouseEvent| {
        e.stop_propagation();
        if state.queue.get().is_empty() { return; }
        if is_playing() {
            audio_call!("pause");
            state.is_playing.set(false);
        } else {
            audio_call!("play");
            state.is_playing.set(true);
        }
    };

    let open_player = move |_| state.show_full_player.set(true);

    view! {
        <div class="player-bar" on:click=open_player>
            <div class="player-bar-progress" style=format!("width:{}%", pct())></div>
            <div class="player-bar-inner">
                <div class="player-bar-info">
                    <span class="player-bar-title">{title}</span>
                    <span class="player-bar-artist">{artist}</span>
                </div>
                <div class="player-bar-controls" on:click=|e| e.stop_propagation()>
                    <button class="pb-btn" on:click=on_play_click>{play_icon}</button>
                </div>
            </div>
        </div>
    }
}

// ─── FullPlayer Component ───

#[component]
pub fn FullPlayer(
    state: PlayerState,
) -> impl IntoView {
    let title = move || {
        let idx = state.current_index.get();
        idx.and_then(|i| state.queue.get().get(i).cloned())
            .map_or("".to_string(), |t| t.title)
    };
    let artist = move || {
        let idx = state.current_index.get();
        idx.and_then(|i| state.queue.get().get(i).cloned())
            .map_or("".to_string(), |t| t.artist)
    };
    let cover = move || {
        let idx = state.current_index.get();
        idx.and_then(|i| state.queue.get().get(i).cloned())
            .map_or("#0D0D1A".to_string(), |t| t.cover_color)
    };
    let is_playing = move || state.is_playing.get();
    let play_icon = move || if is_playing() { "⏸" } else { "▶" };
    let cur_t = move || format_time(state.current_time.get());
    let dur_t = move || format_time(state.duration.get());
    let pct = move || state.progress.get() * 100.0;
    let vol = move || state.volume.get();
    let vol_icon = move || if vol() == 0.0 { "🔇" } else if vol() < 0.4 { "🔉" } else { "🔊" };
    let mode = move || state.play_mode.get();
    let mode_icon = move || match mode() {
        PlayMode::Sequential => "🔁",
        PlayMode::Shuffle => "🔀",
        PlayMode::RepeatOne => "🔂",
    };
    let show_lyr = move || state.show_lyrics.get();

    // Seek handler
    let on_seek = move |ev: leptos::ev::MouseEvent| {
        let el = event_target::<web_sys::Element>(&ev);
        let js_ev: &JsValue = ev.as_ref();
        let client_x = js_sys::Reflect::get(js_ev, &JsValue::from_str("clientX"))
            .ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        // Get element position via JS
        let js_el: &JsValue = el.as_ref();
        let rect = js_sys::Reflect::get(js_el, &JsValue::from_str("getBoundingClientRect"))
            .and_then(|f| {
                let f = js_sys::Function::from(f);
                f.call0(&el)
            }).ok();
        let left = rect.as_ref()
            .and_then(|r| js_sys::Reflect::get(r, &JsValue::from_str("left")).ok())
            .and_then(|v| v.as_f64()).unwrap_or(0.0);
        let width = rect.as_ref()
            .and_then(|r| js_sys::Reflect::get(r, &JsValue::from_str("width")).ok())
            .and_then(|v| v.as_f64()).unwrap_or(1.0);
        if width > 0.0 {
            let ratio = ((client_x - left) / width).min(1.0).max(0.0);
            seek(ratio);
        }
    };

    // Volume change
    let on_vol = move |ev: leptos::ev::Event| {
        let input = event_target_value(&ev);
        if let Ok(v) = input.parse::<f64>() {
            let v = (v / 100.0).min(1.0).max(0.0);
            set_volume(v);
            state.volume.set(v);
        }
    };

    let close_player = {
        let state = state.clone();
        move |_| state.show_full_player.set(false)
    };

    let toggle_play_handler = {
        let state = state.clone();
        move |_| {
            if state.queue.get().is_empty() { return; }
            if is_playing() {
                audio_call!("pause");
                state.is_playing.set(false);
            } else {
                audio_call!("play");
                state.is_playing.set(true);
            }
        }
    };

    let next_handler = {
        let state = state.clone();
        move |_| next_track(state.clone())
    };
    let prev_handler = {
        let state = state.clone();
        move |_| prev_track(state.clone())
    };

    let toggle_mode = {
        let state = state.clone();
        move |_| {
            state.play_mode.update(|m| {
                *m = match m {
                    PlayMode::Sequential => PlayMode::Shuffle,
                    PlayMode::Shuffle => PlayMode::RepeatOne,
                    PlayMode::RepeatOne => PlayMode::Sequential,
                }
            });
        }
    };

    let toggle_lyrics = {
        let state = state.clone();
        move |_| state.show_lyrics.update(|v| *v = !*v)
    };

    view! {
        <div class="full-player-overlay" on:click=close_player>
            <div class="full-player" on:click=|e| e.stop_propagation()>
                // Background blur layer
                <div class="fp-background" style=move || format!("background:{}", cover())></div>
                <div class="fp-backdrop"></div>

                // Close button
                <button class="fp-close" on:click=close_player>"✕"</button>

                // Top toolbar
                <div class="fp-toolbar">
                    <button class="fp-tool-btn" on:click=close_player>"🏠"</button>
                    <button class="fp-tool-btn" on:click=toggle_lyrics>"📄"</button>
                    <button class="fp-tool-btn" on:click=toggle_mode>{mode_icon}</button>
                </div>

                // Song info
                <div class="fp-song-info">
                    <div class="fp-cover-art" style=move || format!("background:{}", cover())>
                        <span class="fp-cover-icon">"♪"</span>
                    </div>
                    <h1 class="fp-title">{title}</h1>
                    <p class="fp-artist">{artist}</p>
                </div>

                // Controls
                <div class="fp-controls">
                    <div class="fp-progress-area" on:click=on_seek>
                        <div class="fp-progress-track">
                            <div class="fp-progress-fill" style=format!("width:{}%", pct())></div>
                            <div class="fp-progress-thumb" style=format!("left:{}%", pct())></div>
                        </div>
                        <div class="fp-time-display">
                            <span>{cur_t}</span>
                            <span>{dur_t}</span>
                        </div>
                    </div>

                    <div class="fp-buttons">
                        <button class="fp-btn" on:click=prev_handler>"⏮"</button>
                        <button class="fp-btn fp-btn-play" on:click=toggle_play_handler>{play_icon}</button>
                        <button class="fp-btn" on:click=next_handler>"⏭"</button>
                    </div>
                </div>

                // Volume
                <div class="fp-volume">
                    <span class="fp-vol-icon">{vol_icon}</span>
                    <input
                        type="range"
                        min="0"
                        max="100"
                        value=move || (vol() * 100.0) as i32
                        on:input=on_vol
                        class="fp-volume-slider"
                    />
                </div>

                // Lyrics panel
                <Show when=move || show_lyr()>
                    <div class="fp-lyrics-panel">
                        <div class="fp-lyrics-content">
                            <div class="fp-lyric-line current">"🎵 正在播放..."</div>
                            <div class="fp-lyric-line">{title}</div>
                            <div class="fp-lyric-line">{artist}</div>
                            <div class="fp-lyric-line dim">"歌词加载中"</div>
                        </div>
                    </div>
                </Show>
            </div>
        </div>
    }
}

use crate::player::*;
use crate::tauri_utils::invoke;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

// ─── Data types ───
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
    pub title: String,
    pub artist: String,
    pub source: String,
    pub source_id: usize,
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
pub struct HotItem {
    pub title: String,
    pub source: String,
    pub source_id: usize,
}

#[derive(Clone)]
struct NavItem {
    icon: &'static str,
    label: &'static str,
    active: bool,
}

const NAV_ITEMS: [NavItem; 5] = [
    NavItem { icon: "🏠", label: "首页", active: true },
    NavItem { icon: "🎵", label: "曲库", active: false },
    NavItem { icon: "🔍", label: "搜索", active: false },
    NavItem { icon: "❤️", label: "收藏", active: false },
    NavItem { icon: "⚙️", label: "设置", active: false },
];

fn queue_song(ps: &PlayerState, title: &str, artist: &str, source: &str) {
    let mut q = ps.queue.get();
    q.push(TrackInfo::from_song(title, artist, source));
    let idx = q.len() - 1;
    ps.queue.set(q);
    ps.current_index.set(Some(idx));
    ps.show_full_player.set(true);
}

// ========================================================================

#[component]
pub fn MainPage() -> impl IntoView {
    let show_menu = RwSignal::new(false);
    let toggle_menu = move |_| show_menu.update(|v| *v = !*v);

    let sources = RwSignal::new(Vec::<SourceInfo>::new());
    let search_results = RwSignal::new(Vec::<SongResult>::new());
    let search_query = RwSignal::new(String::new());
    let is_searching = RwSignal::new(false);
    let active_source = RwSignal::new(Option::<SourceInfo>::None);
    let hot_keywords = RwSignal::new(Vec::<HotItem>::new());
    let featured_songs = RwSignal::new(Vec::<SongResult>::new());

    // Player state
    let player_state = PlayerState::new();
    setup_audio_events(player_state.clone());

    // Load sources on mount
    let sources_c = sources.clone();
    let active_source_c = active_source.clone();
    let load_sources = async move {
        if let Ok(val) = invoke("get_sources", None).await {
            if let Ok(list) = serde_wasm_bindgen::from_value::<Vec<SourceInfo>>(val) {
                sources_c.set(list.clone());
                if let Ok(active) = invoke("get_active_source", None).await {
                    if let Ok(a) = serde_wasm_bindgen::from_value::<SourceInfo>(active) {
                        active_source_c.set(Some(a));
                    }
                }
            }
        }
    };
    let _ = wasm_bindgen_futures::spawn_local(load_sources);

    // Load hot keywords
    let hk = hot_keywords.clone();
    let fs = featured_songs.clone();
    let load_hot = async move {
        if let Ok(val) = invoke("get_hot_keywords", None).await {
            if let Ok(items) = serde_wasm_bindgen::from_value::<Vec<HotItem>>(val) {
                hk.set(items.clone());
                // Auto-search first hot keyword for featured songs
                if let Some(first) = items.first() {
                    let args = js_sys::Object::new();
                    js_sys::Reflect::set(&args, &JsValue::from_str("keyword"), &JsValue::from_str(&first.title)).ok();
                    if let Ok(resp) = invoke("search_music", Some(&args)).await {
                        if let Ok(resp) = serde_wasm_bindgen::from_value::<SearchResponse>(resp) {
                            fs.set(resp.results.into_iter().take(12).collect());
                        }
                    }
                }
            }
        }
    };
    let _ = wasm_bindgen_futures::spawn_local(load_hot);

    // Search handler
    let search_query_c = search_query.clone();
    let search_results_c = search_results.clone();
    let is_searching_c = is_searching.clone();
    let sources_c = sources.clone();
    let do_search: Box<dyn Fn(String) + Send + Sync> = Box::new(move |keyword: String| {
        if keyword.trim().is_empty() {
            search_results_c.set(vec![]);
            return;
        }
        search_query_c.set(keyword.clone());
        is_searching_c.set(true);
        let search_results_c2 = search_results_c.clone();
        let is_searching_c2 = is_searching_c.clone();
        let sources_c2 = sources_c.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let args = js_sys::Object::new();
            js_sys::Reflect::set(&args, &JsValue::from_str("keyword"), &JsValue::from_str(&keyword)).ok();
            if let Ok(val) = invoke("search_music", Some(&args)).await {
                if let Ok(resp) = serde_wasm_bindgen::from_value::<SearchResponse>(val) {
                    search_results_c2.set(resp.results);
                    if let Ok(s) = invoke("get_sources", None).await {
                        if let Ok(list) = serde_wasm_bindgen::from_value::<Vec<SourceInfo>>(s) {
                            sources_c2.set(list);
                        }
                    }
                }
            }
            is_searching_c2.set(false);
        });
    });

    view! {
        <div class="app-layout">
            <Sidebar sources />
            <MainContent
                search_results
                search_query
                is_searching
                do_search
                hot_keywords
                featured_songs
                player_state=player_state.clone()
            />
            <RightPanel sources active_source hot_keywords player_state=player_state.clone() />
            <PlayerBar state=player_state.clone() />
        </div>

        // Full Player
        <Show when=move || player_state.show_full_player.get()>
            <FullPlayer state=player_state.clone() />
        </Show>

        // Floating orb
        <div class="floating-orb-container">
            <button class="floating-orb" on:click=toggle_menu>
                <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
                    <circle cx="12" cy="12" r="3" fill="currentColor"/>
                    <path d="M12 5v14M5 12h14" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                </svg>
            </button>
            <div class="saturn-ring"></div>
            <div class="saturn-ring saturn-ring-2"></div>
            <div class="saturn-ring saturn-ring-3"></div>
        </div>

        // Floating menu
        <Show when=move || show_menu.get()>
            <div class="floating-menu-overlay" on:click=move |_| show_menu.set(false)></div>
            <div class="floating-menu">
                <div class="floating-menu-header">
                    <span>"Miku Tunes"</span>
                    <button class="floating-menu-close" on:click=move |_| show_menu.set(false)>"✕"</button>
                </div>
                <div class="floating-menu-items">
                    <button class="floating-menu-item">
                        <span class="fmi-icon">"📡"</span>
                        <span class="fmi-label">"切换音源"</span>
                        <span class="fmi-arrow">"→"</span>
                    </button>
                    <button class="floating-menu-item">
                        <span class="fmi-icon">"🐱"</span>
                        <span class="fmi-label">"桌宠"</span>
                        <span class="fmi-arrow">"→"</span>
                    </button>
                    <div class="floating-menu-divider"></div>
                    <div class="floating-source-list">
                        <For
                            each=move || sources.get()
                            key=|s| s.id
                            children=move |source: SourceInfo| {
                                let source_id = source.id;
                                let source_name = source.name.clone();
                                let dot_color = if source.score >= 0 { "#39C5BB".to_string() } else { "#E85555".to_string() };
                                let score_txt = if source.score >= 0 {
                                    format!("+{}", source.score)
                                } else {
                                    format!("{}", source.score)
                                };
                                let is_active = move || {
                                    active_source.get().as_ref().map_or(false, |a| a.id == source_id)
                                };
                                view! {
                                    <div class="floating-source-item" class:active=is_active>
                                        <span class="fs-dot" style=format!("background:{}", dot_color)></span>
                                        <span>{source_name}</span>
                                        <span class="fs-score">{score_txt}</span>
                                        <Show when=is_active>
                                            <span class="fs-badge active-badge">"当前"</span>
                                        </Show>
                                    </div>
                                }
                            }
                        />
                    </div>
                </div>
            </div>
        </Show>
    }
}

// ========================================================================
// Sidebar
// ========================================================================
#[component]
fn Sidebar(sources: RwSignal<Vec<SourceInfo>>) -> impl IntoView {
    let nav_items = NAV_ITEMS.to_vec();
    let playlists = vec!["我最爱的Miku曲", "Vocaloid精选", "日系推荐", "深夜听"];
    let source_count = move || sources.get().len();
    let total_score = Signal::derive(move || {
        sources.get().iter().map(|s| s.score).sum::<i32>()
    });

    view! {
        <aside class="sidebar">
            <div class="sidebar-logo">
                <div class="logo-icon">
                    <svg width="28" height="28" viewBox="0 0 28 28" fill="none">
                        <circle cx="14" cy="14" r="13" stroke="#39C5BB" stroke-width="2"/>
                        <path d="M10 18V10l8 4-8 4z" fill="#39C5BB"/>
                    </svg>
                </div>
                <span class="logo-text">"Miku Tunes"</span>
            </div>
            <nav class="sidebar-nav">
                {nav_items.into_iter().map(|item| {
                    let cls = if item.active { "nav-item active" } else { "nav-item" };
                    view! {
                        <a class={cls} href="#">
                            <span class="nav-icon">{item.icon}</span>
                            <span class="nav-label">{item.label}</span>
                        </a>
                    }
                }).collect_view()}
            </nav>
            <div class="sidebar-divider"></div>
            <div class="sidebar-section-title">"播放列表"</div>
            <div class="sidebar-playlists">
                {playlists.into_iter().map(|name| {
                    view! {
                        <a class="playlist-item" href="#">
                            <span class="playlist-icon">"♫"</span>
                            <span class="playlist-name">{name}</span>
                        </a>
                    }
                }).collect_view()}
            </div>
            <div class="sidebar-footer">
                <div class="scan-status">
                    <span class="scan-dot"></span>
                    <span>{move || format!("{} 个音源就绪 (总分: {})", source_count(), total_score.get())}</span>
                </div>
            </div>
        </aside>
    }
}

// ========================================================================
// MainContent
// ========================================================================
#[component]
fn MainContent(
    search_results: RwSignal<Vec<SongResult>>,
    search_query: RwSignal<String>,
    is_searching: RwSignal<bool>,
    do_search: Box<dyn Fn(String) + Send + Sync>,
    hot_keywords: RwSignal<Vec<HotItem>>,
    featured_songs: RwSignal<Vec<SongResult>>,
    player_state: PlayerState,
) -> impl IntoView {
    let do_search = std::sync::Arc::new(do_search);
    let hour = js_sys::Date::new_0().get_hours();
    let greeting = if hour < 12 { "上午好" } else if hour < 18 { "下午好" } else { "晚上好" };

    let on_search_input = {
        let do_search = do_search.clone();
        move |ev: leptos::ev::Event| {
            let input = event_target_value(&ev);
            if input.len() >= 2 {
                do_search(input);
            }
        }
    };

    // Fallback content (dynamic featured songs + hot keywords)
    let ps_fb = player_state.clone();
    let fallback_content = move || {
        let songs = featured_songs.get();
        let hots = hot_keywords.get();
        let ps = ps_fb.clone();
        let do_search = do_search.clone();

        let mut children: Vec<leptos::prelude::AnyView> = Vec::new();

        // Dynamic hot search section
        if !hots.is_empty() {
            let chips: Vec<leptos::prelude::AnyView> = hots.iter().take(16).enumerate().map(|(i, item)| {
                let kw = item.title.clone();
                let src = item.source.clone();
                let do_search = do_search.clone();
                let kw_handler = kw.clone();
                let handler = move |_| {
                    do_search(kw_handler.clone());
                };
                let badge = if i <= 2 {
                    Some(view! { <sup>{i+1}</sup> }.into_any())
                } else { None };
                let colors = ["#FF9EC5","#FF6B9D","#39C5BB","#6C8BFF","#8EDBD5","#E85555","#FFB8D6"];
                let color = colors[i % colors.len()];
                view! {
                    <button style=format!("border-color:{}", color) on:click=handler>
                        {badge}
                        <span>{kw}</span>
                        <small>{src}</small>
                    </button>
                }.into_any()
            }).collect();
            children.push(view! {
                <section class="section">
                    <div class="section-header">
                        <h2 class="section-title">"🔥 热门搜索"</h2>
                    </div>
                    <div class="hot-keywords-list">
                        {chips}
                    </div>
                </section>
            }.into_any());
        }

        // Dynamic featured songs section
        if !songs.is_empty() {
            let cards: Vec<leptos::prelude::AnyView> = songs.iter().map(|song| {
                let ps = ps.clone();
                let t = song.title.clone();
                let a = song.artist.clone();
                let s = song.source.clone();
                let cb = move |_| queue_song(&ps, &t, &a, &s);
                let colors = ["linear-gradient(135deg,#FF9EC5,#FF6B9D)","linear-gradient(135deg,#39C5BB,#2A9D95)","linear-gradient(135deg,#6C8BFF,#4A6BDF)","linear-gradient(135deg,#8EDBD5,#39C5BB)","linear-gradient(135deg,#E85555,#B83030)","linear-gradient(135deg,#FFB8D6,#FF9EC5)"];
                let color = colors[js_sys::Math::random() as usize % colors.len()];
                view! {
                    <div class="song-card" style=format!("background:{};", color)>
                        <div class="song-card-overlay">
                            <button class="play-btn-small" on:click=cb>"▶"</button>
                        </div>
                        <div class="song-card-info">
                            <div class="song-card-title">{song.title.clone()}</div>
                            <div class="song-card-artist">{song.artist.clone()}</div>
                        </div>
                    </div>
                }.into_any()
            }).collect();
            children.push(view! {
                <section class="section">
                    <div class="section-header">
                        <h2 class="section-title">"🎵 精选推荐"</h2>
                        <a class="section-more" href="#">"更多 →"</a>
                    </div>
                    <div class="featured-grid">
                        {cards}
                    </div>
                </section>
            }.into_any());
        }

        children.into_any()
    };

    // Search results content
    let ps_rc = player_state.clone();
    let results_content = move || {
        let query = search_query.get();
        let count = search_results.get().len();
        view! {
            <section class="section">
                <div class="section-header">
                    <h2 class="section-title">{format!("搜索结果: 「{}」", query)}</h2>
                    <a class="section-more" href="#">{format!("共 {} 首", count)}</a>
                </div>
                <div class="search-results-list">
                    {render_search_results(search_results.get(), ps_rc.clone())}
                </div>
            </section>
        };
    };
    let _ = results_content;

    view! {
        <main class="main-content">
            <section class="hero-section">
                <div class="hero-text">
                    <h1 class="hero-greeting">{greeting}"，Miku Fan"</h1>
                    <div class="search-bar">
                        <span class="search-icon">"🔍"</span>
                        <input
                            class="search-input"
                            type="text"
                            placeholder="搜索音乐..."
                            on:input=on_search_input
                        />
                        <Show when=move || is_searching.get()>
                            <span class="search-spinner"></span>
                        </Show>
                    </div>
                </div>
                <div class="hero-avatar">
                    <div class="avatar-circle">
                        <span>"01"</span>
                    </div>
                </div>
            </section>

            {move || {
                if search_results.get().is_empty() {
                    fallback_content().into_any()
                } else {
                    results_content().into_any()
                }
            }}
        </main>
    }
}

fn render_search_results(
    results: Vec<SongResult>,
    ps: PlayerState,
) -> impl IntoView {
    results.into_iter().map(|song| {
        let ps = ps.clone();
        let t = song.title.clone();
        let a = song.artist.clone();
        let s = song.source.clone();
        let score_txt = if song.score >= 0 { format!("+{}", song.score) } else { format!("{}", song.score) };
        let cb = {
            let t = t.clone();
            let a = a.clone();
            let s = s.clone();
            move |_| queue_song(&ps, &t, &a, &s)
        };
        view! {
            <div class="search-result-item">
                <span class="sr-icon">"♪"</span>
                <div class="sr-info">
                    <span class="sr-title">{t}</span>
                    <span class="sr-artist">{a}</span>
                </div>
                <span class="sr-source">{s}</span>
                <span class="sr-score">{score_txt}</span>
                <button class="play-btn-small" on:click=cb>"▶"</button>
            </div>
        }
    }).collect_view()
}

// ========================================================================
// RightPanel
// ========================================================================
#[component]
fn RightPanel(
    sources: RwSignal<Vec<SourceInfo>>,
    active_source: RwSignal<Option<SourceInfo>>,
    hot_keywords: RwSignal<Vec<HotItem>>,
    player_state: PlayerState,
) -> impl IntoView {
    let ps_rc = player_state.clone();
    let total_score = Signal::derive(move || {
        sources.get().iter().map(|s| s.score).sum::<i32>()
    });
    let queue_tracks = move || {
        let q = player_state.queue.get();
        q.iter().rev().take(10).map(|track| {
            let title = track.title.clone();
            view! {
                <div class="recent-item">
                    <span class="recent-icon">"♪"</span>
                    <span class="recent-name">{title}</span>
                </div>
            }
        }).collect_view()
    };
    view! {
        <aside class="right-panel">
            <section class="panel-section">
                <h3 class="panel-title">"🔥 热搜推荐"</h3>
                <div class="rec-list">
                    {move || hot_keywords.get().into_iter().take(10).map(|item| {
                        let t = item.title.clone();
                        let s = item.source.clone();
                        let ps = player_state.clone();
                        let a = String::new();
                        let cb = move |_| queue_song(&ps, &t, &a, &s);
                        let title_display = item.title.clone();
                        let source_display = item.source.clone();
                        view! {
                            <div class="rec-item">
                                <div class="rec-info">
                                    <span class="rec-title">{title_display}</span>
                                    <span class="rec-artist">{source_display}</span>
                                </div>
                                <span class="rec-duration">"热搜"</span>
                                <button class="rec-play" on:click=cb>"🔍"</button>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </section>

            <section class="panel-section">
                <h3 class="panel-title">"📊 音源状态"</h3>
                <div class="shows-list">
                    <div class="show-card">
                        <div class="show-info">
                            <span class="show-name">"活跃音源"</span>
                            <span class="show-time">{move || format!("{} 个", sources.get().len())}</span>
                        </div>
                    </div>
                    <div class="show-card">
                        <div class="show-info">
                            <span class="show-name">"总积分"</span>
                            <span class="show-time">{move || format!("{}", total_score.get())}</span>
                        </div>
                    </div>
                    <div class="show-card">
                        <div class="show-info">
                            <span class="show-name">"当前音源"</span>
                            <span class="show-time">{move || active_source.get().map(|a| a.name).unwrap_or_default()}</span>
                        </div>
                    </div>
                </div>
            </section>

            <section class="panel-section">
                <h3 class="panel-title">"🎶 播放队列"</h3>
                <div class="recent-list">
                    {queue_tracks()}
                </div>
            </section>
        </aside>
    }
}

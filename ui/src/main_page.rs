use crate::player::*;
use crate::tauri_utils::{invoke, listen};
use crate::types::*;
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

fn event_is_composing(ev: &leptos::ev::Event) -> bool {
    ev.dyn_ref::<web_sys::InputEvent>()
        .map(|event| event.is_composing())
        .unwrap_or(false)
}

#[derive(Clone, PartialEq)]
enum Page {
    Home,
    Rankings,
    Playlists,
    Search,
    Favorites,
    Parser,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchSortMode {
    Source,
    Quality,
    Match,
    Duration,
    Title,
}

impl SearchSortMode {
    fn label(self) -> &'static str {
        match self {
            Self::Source => "音源排序",
            Self::Quality => "音质优先",
            Self::Match => "匹配优先",
            Self::Duration => "时长排序",
            Self::Title => "歌名排序",
        }
    }
}

const FAVORITES_KEY: &str = "miku_tunes_favorites_v1";
#[component]
pub fn MainPage() -> impl IntoView {
    let current_page = RwSignal::new(Page::Home);
    let sources = RwSignal::new(Vec::<SourceInfo>::new());
    let active_source = RwSignal::new(Option::<SourceInfo>::None);
    let hot_keywords = RwSignal::new(Vec::<HotItem>::new());
    let rankings = RwSignal::new(Vec::<RankingCategory>::new());
    let ranking_songs = RwSignal::new(Vec::<SongDetail>::new());
    let selected_ranking = RwSignal::new(Option::<RankingCategory>::None);
    let playlists = RwSignal::new(Vec::<PlaylistInfo>::new());
    let playlist_songs = RwSignal::new(Vec::<SongDetail>::new());
    let selected_playlist = RwSignal::new(Option::<PlaylistInfo>::None);
    let loading_home = RwSignal::new(true);
    let loading_detail = RwSignal::new(false);

    let search_query = RwSignal::new(String::new());
    let search_results = RwSignal::new(Vec::<SongResult>::new());
    let search_attempted = RwSignal::new(false);
    let is_searching = RwSignal::new(false);
    let search_token = RwSignal::new(0_u64);
    let debounce_token = RwSignal::new(0_u64);
    let ime_composing = RwSignal::new(false);

    let favorites = RwSignal::new(load_favorites());
    let shared_playlist = RwSignal::new(Option::<SharedPlaylist>::None);
    let shared_loading = RwSignal::new(false);
    let shared_error = RwSignal::new(Option::<String>::None);
    let shared_url = RwSignal::new(String::new());

    let show_source_menu = RwSignal::new(false);
    let show_source_help = RwSignal::new(false);
    let orb_left = RwSignal::new(Option::<f64>::None);
    let orb_top = RwSignal::new(Option::<f64>::None);
    let orb_dragging = RwSignal::new(false);
    let orb_moved = RwSignal::new(false);
    let player_state = PlayerState::new();
    setup_audio_events(player_state.clone());

    {
        let search_results = search_results.clone();
        let search_token = search_token.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let handler = Closure::<dyn FnMut(JsValue)>::wrap(Box::new(move |event: JsValue| {
                let payload = js_sys::Reflect::get(&event, &JsValue::from_str("payload"))
                    .ok()
                    .filter(|value| !value.is_undefined() && !value.is_null())
                    .unwrap_or(event);
                if let Ok(batch) = serde_wasm_bindgen::from_value::<SearchBatchEvent>(payload) {
                    if search_token.get_untracked() == batch.token {
                        search_results.update(|current| {
                            merge_search_results(current, batch.results);
                        });
                    }
                }
            }));
            let _ = listen(
                "search_music_batch",
                handler.as_ref().unchecked_ref::<js_sys::Function>(),
            )
            .await;
            handler.forget();
        });
    }

    let toggle_favorite_song: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync> = {
        let favorites = favorites.clone();
        std::sync::Arc::new(move |song| {
            let mut data = favorites.get_untracked();
            if let Some(index) = data
                .songs
                .iter()
                .position(|item| same_song_favorite(item, &song))
            {
                data.songs.remove(index);
            } else {
                data.songs.insert(0, song);
            }
            save_favorites(&data);
            favorites.set(data);
        })
    };

    let toggle_favorite_playlist: std::sync::Arc<dyn Fn(FavoritePlaylist) + Send + Sync> =
        {
            let favorites = favorites.clone();
            std::sync::Arc::new(move |playlist| {
                let mut data = favorites.get_untracked();
                if let Some(index) = data.playlists.iter().position(|item| {
                    item.id == playlist.id && item.source_name == playlist.source_name
                }) {
                    data.playlists.remove(index);
                } else {
                    data.playlists.insert(0, playlist);
                }
                save_favorites(&data);
                favorites.set(data);
            })
        };

    let on_play_song: std::sync::Arc<dyn Fn(String, String, String, usize, String) + Send + Sync> = {
        let state = player_state.clone();
        std::sync::Arc::new(move |title, artist, song_id, source_id, platform| {
            if song_id.trim().is_empty() {
                return;
            }
            let state = state.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut queue = state.queue.get_untracked();
                let mut track = TrackInfo::from_song(&title, &artist);
                track.song_id = song_id.clone();
                track.source_id = source_id;
                track.platform = platform.clone();
                queue.push(track);
                let index = queue.len().saturating_sub(1);
                state.queue.set(queue);
                state.current_index.set(Some(index));
                state.show_full_player.set(true);
                play_song_by_id(&state, index, source_id, &song_id, &platform).await;
            });
        })
    };

    let on_play_search_result: std::sync::Arc<dyn Fn(Vec<SongResult>, usize) + Send + Sync> = {
        let state = player_state.clone();
        std::sync::Arc::new(move |songs, index| replace_queue_and_play(state.clone(), songs, index))
    };

    let do_search: std::sync::Arc<dyn Fn(String) + Send + Sync> = {
        let search_query = search_query.clone();
        let search_results = search_results.clone();
        let search_attempted = search_attempted.clone();
        let is_searching = is_searching.clone();
        let search_token = search_token.clone();
        std::sync::Arc::new(move |keyword| {
            let keyword = keyword.trim().to_string();
            search_query.set(keyword.clone());
            if keyword.is_empty() {
                search_results.set(Vec::new());
                search_attempted.set(false);
                is_searching.set(false);
                return;
            }
            let token = search_token.get_untracked().wrapping_add(1);
            search_token.set(token);
            search_results.set(Vec::new());
            search_attempted.set(true);
            is_searching.set(true);
            let results_signal = search_results.clone();
            let attempted_signal = search_attempted.clone();
            let searching_signal = is_searching.clone();
            let token_signal = search_token.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let args = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("keyword"),
                    &JsValue::from_str(&keyword),
                );
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("token"),
                    &JsValue::from_f64(token as f64),
                );
                let mut results = Vec::new();
                if let Ok(value) = invoke("search_music_stream", Some(&args)).await {
                    if let Ok(resp) = serde_wasm_bindgen::from_value::<SearchResponse>(value) {
                        results = resp.results;
                    }
                }
                if token_signal.get_untracked() == token {
                    results_signal.set(dedupe_search_results(results));
                    attempted_signal.set(true);
                    searching_signal.set(false);
                }
            });
        })
    };

    let schedule_search: std::sync::Arc<dyn Fn(String) + Send + Sync> = {
        let do_search = do_search.clone();
        let current_page = current_page.clone();
        let debounce_token = debounce_token.clone();
        std::sync::Arc::new(move |keyword| {
            let keyword = keyword.trim().to_string();
            let token = debounce_token.get_untracked().wrapping_add(1);
            debounce_token.set(token);
            if keyword.is_empty() {
                do_search(String::new());
                return;
            }
            let do_search = do_search.clone();
            let debounce_token = debounce_token.clone();
            let current_page = current_page.clone();
            wasm_bindgen_futures::spawn_local(async move {
                TimeoutFuture::new(120).await;
                if debounce_token.get_untracked() == token {
                    current_page.set(Page::Search);
                    do_search(keyword);
                }
            });
        })
    };

    let on_select_ranking: std::sync::Arc<dyn Fn(RankingCategory) + Send + Sync> = {
        let selected_ranking = selected_ranking.clone();
        let ranking_songs = ranking_songs.clone();
        let loading_detail = loading_detail.clone();
        let current_page = current_page.clone();
        std::sync::Arc::new(move |ranking| {
            selected_ranking.set(Some(ranking.clone()));
            ranking_songs.set(Vec::new());
            loading_detail.set(true);
            current_page.set(Page::Rankings);
            let ranking_songs = ranking_songs.clone();
            let loading_detail = loading_detail.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let args = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("sourceId"),
                    &JsValue::from_f64(ranking.source_id as f64),
                );
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("rankingId"),
                    &JsValue::from_str(&ranking.id),
                );
                if let Ok(value) = invoke("get_ranking_songs", Some(&args)).await {
                    if let Ok(list) = serde_wasm_bindgen::from_value::<Vec<SongDetail>>(value) {
                        ranking_songs.set(list);
                    }
                }
                loading_detail.set(false);
            });
        })
    };

    let on_select_playlist: std::sync::Arc<dyn Fn(PlaylistInfo) + Send + Sync> = {
        let selected_playlist = selected_playlist.clone();
        let playlist_songs = playlist_songs.clone();
        let loading_detail = loading_detail.clone();
        let current_page = current_page.clone();
        std::sync::Arc::new(move |playlist| {
            selected_playlist.set(Some(playlist.clone()));
            playlist_songs.set(Vec::new());
            loading_detail.set(true);
            current_page.set(Page::Playlists);
            let playlist_songs = playlist_songs.clone();
            let loading_detail = loading_detail.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let args = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("sourceId"),
                    &JsValue::from_f64(playlist.source_id as f64),
                );
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("playlistId"),
                    &JsValue::from_str(&playlist.id),
                );
                if let Ok(value) = invoke("get_playlist_songs", Some(&args)).await {
                    if let Ok(list) = serde_wasm_bindgen::from_value::<Vec<SongDetail>>(value) {
                        playlist_songs.set(list);
                    }
                }
                loading_detail.set(false);
            });
        })
    };

    let move_source_action: std::sync::Arc<dyn Fn(usize, String) + Send + Sync> = {
        let sources = sources.clone();
        let active_source = active_source.clone();
        std::sync::Arc::new(move |source_id, action| {
            let sources = sources.clone();
            let active_source = active_source.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let args = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("sourceId"),
                    &JsValue::from_f64(source_id as f64),
                );
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("action"),
                    &JsValue::from_str(&action),
                );
                if let Ok(value) = invoke("move_source", Some(&args)).await {
                    if let Ok(list) = serde_wasm_bindgen::from_value::<Vec<SourceInfo>>(value) {
                        sources.set(list);
                    }
                }
                if let Ok(value) = invoke("get_active_source", None).await {
                    if let Ok(active) = serde_wasm_bindgen::from_value::<Option<SourceInfo>>(value)
                    {
                        active_source.set(active);
                    }
                }
            });
        })
    };

    let set_source_enabled: std::sync::Arc<dyn Fn(usize, bool) + Send + Sync> = {
        let sources = sources.clone();
        let active_source = active_source.clone();
        std::sync::Arc::new(move |source_id, enabled| {
            let sources = sources.clone();
            let active_source = active_source.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let args = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("sourceId"),
                    &JsValue::from_f64(source_id as f64),
                );
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("enabled"),
                    &JsValue::from_bool(enabled),
                );
                if let Ok(value) = invoke("set_source_enabled", Some(&args)).await {
                    if let Ok(list) = serde_wasm_bindgen::from_value::<Vec<SourceInfo>>(value) {
                        sources.set(list);
                    }
                }
                if let Ok(value) = invoke("get_active_source", None).await {
                    if let Ok(active) = serde_wasm_bindgen::from_value::<Option<SourceInfo>>(value)
                    {
                        active_source.set(active);
                    }
                }
            });
        })
    };

    {
        let sources = sources.clone();
        let active_source = active_source.clone();
        let hot_keywords = hot_keywords.clone();
        let rankings = rankings.clone();
        let playlists = playlists.clone();
        let loading_home = loading_home.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(value) = invoke("get_sources", None).await {
                if let Ok(list) = serde_wasm_bindgen::from_value::<Vec<SourceInfo>>(value) {
                    sources.set(list);
                    if let Ok(value) = invoke("get_active_source", None).await {
                        if let Ok(active) =
                            serde_wasm_bindgen::from_value::<Option<SourceInfo>>(value)
                        {
                            active_source.set(active);
                        }
                    }
                    if let Ok(value) = invoke("get_hot_keywords", None).await {
                        if let Ok(items) = serde_wasm_bindgen::from_value::<Vec<HotItem>>(value) {
                            hot_keywords.set(items);
                        }
                    }
                    if let Ok(value) = invoke("get_all_rankings", None).await {
                        if let Ok(items) =
                            serde_wasm_bindgen::from_value::<Vec<RankingCategory>>(value)
                        {
                            rankings.set(items);
                        }
                    }
                    if let Ok(value) = invoke("get_all_playlists", None).await {
                        if let Ok(items) =
                            serde_wasm_bindgen::from_value::<Vec<PlaylistInfo>>(value)
                        {
                            playlists.set(items);
                        }
                    }
                }
            }
            loading_home.set(false);
        });
    }

    if false {
        let shared_playlist = shared_playlist.clone();
        let shared_loading = shared_loading.clone();
        let shared_error = shared_error.clone();
        wasm_bindgen_futures::spawn_local(async move {
            shared_loading.set(true);
            let args = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&args, &JsValue::from_str("url"), &JsValue::from_str(""));
            match invoke("parse_kugou_playlist", Some(&args)).await {
                Ok(value) => match serde_wasm_bindgen::from_value::<SharedPlaylist>(value) {
                    Ok(playlist) => shared_playlist.set(Some(playlist)),
                    Err(err) => shared_error.set(Some(format!("歌单数据解析失败: {err}"))),
                },
                Err(err) => shared_error.set(Some(
                    err.as_string()
                        .unwrap_or_else(|| "酷狗分享歌单解析失败".to_string()),
                )),
            }
            shared_loading.set(false);
        });
    }

    let parse_shared_url: std::sync::Arc<dyn Fn(String) + Send + Sync> = {
        let shared_url = shared_url.clone();
        let shared_playlist = shared_playlist.clone();
        let shared_loading = shared_loading.clone();
        let shared_error = shared_error.clone();
        let current_page = current_page.clone();
        std::sync::Arc::new(move |url| {
            let url = url.trim().to_string();
            shared_url.set(url.clone());
            shared_playlist.set(None);
            shared_error.set(None);
            current_page.set(Page::Parser);
            if url.is_empty() {
                shared_error.set(Some("请输入分享链接".to_string()));
                return;
            }
            shared_loading.set(true);
            let shared_playlist = shared_playlist.clone();
            let shared_loading = shared_loading.clone();
            let shared_error = shared_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let args = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &args,
                    &JsValue::from_str("url"),
                    &JsValue::from_str(&url),
                );
                match invoke("parse_kugou_playlist", Some(&args)).await {
                    Ok(value) => match serde_wasm_bindgen::from_value::<SharedPlaylist>(value) {
                        Ok(playlist) => shared_playlist.set(Some(playlist)),
                        Err(err) => shared_error.set(Some(format!("歌单数据解析失败: {err}"))),
                    },
                    Err(err) => shared_error.set(Some(
                        err.as_string()
                            .unwrap_or_else(|| "分享链接解析失败".to_string()),
                    )),
                }
                shared_loading.set(false);
            });
        })
    };

    let nav_to = std::sync::Arc::new(move |page| current_page.set(page));
    let floating_move_source = StoredValue::new(move_source_action.clone());
    let floating_set_source_enabled = StoredValue::new(set_source_enabled.clone());
    let navigate_search: std::sync::Arc<dyn Fn(String) + Send + Sync> = {
        let current_page = current_page.clone();
        let do_search = do_search.clone();
        std::sync::Arc::new(move |keyword| {
            current_page.set(Page::Search);
            do_search(keyword);
        })
    };

    let right_panel_search = navigate_search.clone();
    let player_bar_toggle_favorite = toggle_favorite_song.clone();
    let full_player_toggle_favorite = toggle_favorite_song.clone();
    let content_do_search = navigate_search.clone();
    let content_schedule_search = schedule_search.clone();
    let content_play_song = on_play_song.clone();
    let content_play_search = on_play_search_result.clone();
    let content_toggle_song = toggle_favorite_song.clone();
    let content_toggle_playlist = toggle_favorite_playlist.clone();
    let content_select_ranking = on_select_ranking.clone();
    let content_select_playlist = on_select_playlist.clone();

    view! {
        <div class="app-layout" on:contextmenu=move |ev| ev.prevent_default()>
            <Sidebar
                current_page
                on_navigate=nav_to
                sources
                playlists
                on_select_playlist=on_select_playlist.clone()
            />
            <main class="main-content">
                {move || match current_page.get() {
                    Page::Home => view! {
                        <HomeView
                            search_query
                            is_searching
                            ime_composing
                            schedule_search=content_schedule_search.clone()
                            do_search=content_do_search.clone()
                            hot_keywords=hot_keywords.get()
                            rankings=rankings.get()
                            playlists=playlists.get()
                            loading=loading_home.get()
                            sources=sources.get()
                            favorites=favorites.get()
                            on_search=content_do_search.clone()
                            on_more_rankings={
                                let current_page = current_page.clone();
                                std::sync::Arc::new(move || current_page.set(Page::Rankings))
                            }
                            on_more_recommendations={
                                let current_page = current_page.clone();
                                std::sync::Arc::new(move || current_page.set(Page::Search))
                            }
                            on_more_playlists={
                                let current_page = current_page.clone();
                                std::sync::Arc::new(move || current_page.set(Page::Playlists))
                            }
                            on_select_ranking=content_select_ranking.clone()
                            on_select_playlist=content_select_playlist.clone()
                            on_toggle_favorite_playlist=content_toggle_playlist.clone()
                        />
                    }.into_any(),
                    Page::Rankings => view! {
                        <RankingView
                            rankings
                            ranking_songs
                            selected_ranking
                            loading=loading_detail
                            favorites
                            on_select_ranking=content_select_ranking.clone()
                            on_play_song=content_play_song.clone()
                            on_toggle_favorite_song=content_toggle_song.clone()
                        />
                    }.into_any(),
                    Page::Playlists => view! {
                        <PlaylistView
                            playlists
                            playlist_songs
                            selected_playlist
                            loading=loading_detail
                            favorites
                            on_select_playlist=content_select_playlist.clone()
                            on_play_song=content_play_song.clone()
                            on_toggle_favorite_song=content_toggle_song.clone()
                            on_toggle_favorite_playlist=content_toggle_playlist.clone()
                        />
                    }.into_any(),
                    Page::Search => view! {
                        <SearchView
                            search_query
                            search_results
                            search_attempted
                            is_searching
                            ime_composing
                            schedule_search=content_schedule_search.clone()
                            do_search=content_do_search.clone()
                            favorites=favorites.get()
                            on_play_search_result=content_play_search.clone()
                            on_toggle_favorite_song=content_toggle_song.clone()
                        />
                    }.into_any(),
                    Page::Favorites => view! {
                        {render_favorites_view(
                            favorites.get(),
                            content_play_song.clone(),
                            content_toggle_song.clone(),
                            content_toggle_playlist.clone(),
                        )}
                    }.into_any(),
                    Page::Parser => view! {
                        <ParserView
                            shared_url
                            shared_playlist=shared_playlist.get()
                            shared_loading=shared_loading.get()
                            shared_error=shared_error.get()
                            favorites=favorites.get()
                            on_parse=parse_shared_url.clone()
                            on_play_song=content_play_song.clone()
                            on_toggle_favorite_song=content_toggle_song.clone()
                            on_toggle_favorite_playlist=content_toggle_playlist.clone()
                        />
                    }.into_any(),
                    Page::Settings => view! {
                        <section class="section settings-page">
                            <h2>"设置"</h2>
                            <p>"设置功能即将上线"</p>
                        </section>
                    }.into_any(),
                }}
            </main>
            <RightPanel
                sources
                active_source
                hot_keywords
                on_search=right_panel_search
            />
            <PlayerBar state=player_state.clone() favorites on_toggle_favorite=player_bar_toggle_favorite />
        </div>

        <Show when=move || player_state.show_full_player.get()>
            <FullPlayer state=player_state.clone() favorites on_toggle_favorite=full_player_toggle_favorite.clone() />
        </Show>

        <div
            class="floating-orb-container"
            class:dragging=move || orb_dragging.get()
            style=move || match (orb_left.get(), orb_top.get()) {
                (Some(left), Some(top)) => format!("left:{left}px; top:{top}px; right:auto; bottom:auto;"),
                _ => String::new(),
            }
            on:pointerdown=move |ev| {
                orb_dragging.set(true);
                orb_moved.set(false);
                let target = event_target::<web_sys::Element>(&ev);
                let _ = target.set_pointer_capture(ev.pointer_id());
            }
            on:pointermove=move |ev| {
                if !orb_dragging.get_untracked() {
                    return;
                }
                orb_moved.set(true);
                let margin = 40.0;
                let width = web_sys::window()
                    .and_then(|window| window.inner_width().ok())
                    .and_then(|value| value.as_f64())
                    .unwrap_or(1200.0);
                let height = web_sys::window()
                    .and_then(|window| window.inner_height().ok())
                    .and_then(|value| value.as_f64())
                    .unwrap_or(800.0);
                let left = (ev.client_x() as f64 - 34.0).clamp(margin, width - margin - 68.0);
                let top = (ev.client_y() as f64 - 34.0).clamp(margin, height - margin - 68.0);
                orb_left.set(Some(left));
                orb_top.set(Some(top));
            }
            on:pointerup=move |ev| {
                if !orb_dragging.get_untracked() {
                    return;
                }
                orb_dragging.set(false);
                let margin = 40.0;
                let width = web_sys::window()
                    .and_then(|window| window.inner_width().ok())
                    .and_then(|value| value.as_f64())
                    .unwrap_or(1200.0);
                let current_left = orb_left.get_untracked().unwrap_or(width - margin - 68.0);
                let snapped_left = if current_left + 34.0 < width / 2.0 { margin } else { width - margin - 68.0 };
                orb_left.set(Some(snapped_left));
                let target = event_target::<web_sys::Element>(&ev);
                let _ = target.release_pointer_capture(ev.pointer_id());
            }
        >
            <button class="floating-orb" on:click=move |_| {
                if orb_moved.get_untracked() {
                    orb_moved.set(false);
                    return;
                }
                show_source_menu.update(|show| *show = !*show);
            }>
                "+"
            </button>
            <div class="saturn-ring"></div>
            <div class="saturn-ring saturn-ring-2"></div>
            <div class="saturn-ring saturn-ring-3"></div>
        </div>
        <Show when=move || show_source_menu.get()>
            <div class="floating-menu-overlay" on:click=move |_| show_source_menu.set(false)></div>
            <div class="floating-menu">
                <div class="floating-menu-header">
                    <span>"音源管理"</span>
                    <button class="source-help-btn" on:click=move |ev| {
                        ev.stop_propagation();
                        show_source_help.update(|show| *show = !*show);
                    }>"?"</button>
                    <button class="floating-menu-close" on:click=move |_| show_source_menu.set(false)>"x"</button>
                </div>
                <Show when=move || show_source_help.get()>
                    <div class="source-help-popover">
                        "音源用于搜索、解析和播放音乐。默认全部启用，建议只在音源异常时禁用或调整顺序。"
                    </div>
                </Show>
                <div class="floating-source-list">
                    <For each=move || sources.get() key=|source| source.id let:source>
                        {let source_id = source.id;
                        let enabled = source.enabled;
                        let move_up = floating_move_source.get_value();
                        let move_down = floating_move_source.get_value();
                        let move_top = floating_move_source.get_value();
                        let set_enabled = floating_set_source_enabled.get_value();
                        view! {
                            <div class="floating-source-item" class:disabled=move || !enabled>
                                <span class="fs-dot"></span>
                                <span class="fs-name">{source.name.clone()}</span>
                                <span class="fs-status">{if enabled { "启用" } else { "禁用" }}</span>
                                <div class="fs-actions">
                                    <button on:click=move |_| move_up(source_id, "up".to_string())>"上移"</button>
                                    <button on:click=move |_| move_down(source_id, "down".to_string())>"下移"</button>
                                    <button on:click=move |_| move_top(source_id, "top".to_string())>"置顶"</button>
                                    <button on:click=move |_| set_enabled(source_id, !enabled)>{if enabled { "禁用" } else { "启用" }}</button>
                                </div>
                            </div>
                        }}
                    </For>
                </div>
            </div>
        </Show>
    }
}

#[component]
fn Sidebar(
    current_page: RwSignal<Page>,
    on_navigate: std::sync::Arc<dyn Fn(Page) + Send + Sync>,
    sources: RwSignal<Vec<SourceInfo>>,
    playlists: RwSignal<Vec<PlaylistInfo>>,
    on_select_playlist: std::sync::Arc<dyn Fn(PlaylistInfo) + Send + Sync>,
) -> impl IntoView {
    let nav_items = vec![
        (Page::Home, "首页"),
        (Page::Rankings, "排行榜"),
        (Page::Playlists, "歌单"),
        (Page::Search, "搜索"),
        (Page::Favorites, "收藏"),
        (Page::Parser, "解析"),
        (Page::Settings, "设置"),
    ];
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
                {nav_items.into_iter().map(|(page, label)| {
                    let page_for_active = page.clone();
                    let page_for_click = page.clone();
                    let callback = on_navigate.clone();
                    view! {
                        <a
                            class=move || if current_page.get() == page_for_active { "nav-item active" } else { "nav-item" }
                            href="#"
                            on:click=move |ev| {
                                ev.prevent_default();
                                callback(page_for_click.clone());
                            }
                        >
                            <span class="nav-label">{label}</span>
                        </a>
                    }
                }).collect_view()}
            </nav>
            <div class="sidebar-divider"></div>
            <div class="sidebar-section-title">"播放列表"</div>
            <div class="sidebar-playlists">
                <For each={move || playlists.get().into_iter().take(3).collect::<Vec<_>>()} key=|playlist| playlist.id.clone() let:playlist>
                    {let on_select = on_select_playlist.clone();
                    let playlist_for_click = playlist.clone();
                    view! {
                        <button class="playlist-item" on:click=move |_| on_select(playlist_for_click.clone())>
                            <span class="playlist-icon">"♫"</span>
                            <span class="playlist-name">{playlist.name}</span>
                        </button>
                    }}
                </For>
            </div>
            <div class="sidebar-footer">
                <div class="scan-status">
                    <span class="scan-dot"></span>
                    <span>{move || format!("{} 个音源", sources.get().len())}</span>
                </div>
            </div>
        </aside>
    }
}

#[component]
fn HomeView(
    search_query: RwSignal<String>,
    is_searching: RwSignal<bool>,
    ime_composing: RwSignal<bool>,
    schedule_search: std::sync::Arc<dyn Fn(String) + Send + Sync>,
    do_search: std::sync::Arc<dyn Fn(String) + Send + Sync>,
    hot_keywords: Vec<HotItem>,
    rankings: Vec<RankingCategory>,
    playlists: Vec<PlaylistInfo>,
    loading: bool,
    sources: Vec<SourceInfo>,
    favorites: FavoritesData,
    on_search: std::sync::Arc<dyn Fn(String) + Send + Sync>,
    on_more_rankings: std::sync::Arc<dyn Fn() + Send + Sync>,
    on_more_recommendations: std::sync::Arc<dyn Fn() + Send + Sync>,
    on_more_playlists: std::sync::Arc<dyn Fn() + Send + Sync>,
    on_select_ranking: std::sync::Arc<dyn Fn(RankingCategory) + Send + Sync>,
    on_select_playlist: std::sync::Arc<dyn Fn(PlaylistInfo) + Send + Sync>,
    on_toggle_favorite_playlist: std::sync::Arc<dyn Fn(FavoritePlaylist) + Send + Sync>,
) -> impl IntoView {
    let on_input = {
        let schedule_search = schedule_search.clone();
        move |ev: leptos::ev::Event| {
            let value = event_target_value(&ev);
            search_query.set(value.clone());
            if ime_composing.get_untracked() || event_is_composing(&ev) {
                return;
            }
            schedule_search(value);
        }
    };
    let on_keydown = {
        let do_search = do_search.clone();
        move |ev: leptos::ev::KeyboardEvent| {
            if ev.key() == "Enter" && !ev.is_composing() {
                do_search(event_target_value(&ev));
            }
        }
    };

    view! {
        <section class="home-discovery">
            <div class="home-search-panel">
                <div class="search-bar">
                    <span class="search-icon">"Search"</span>
                    <input
                        class="search-input"
                        type="text"
                        placeholder="搜索音乐..."
                        prop:value=move || search_query.get()
                        on:input=on_input
                        on:compositionstart=move |_| ime_composing.set(true)
                        on:compositionend=move |ev: leptos::ev::CompositionEvent| {
                            ime_composing.set(false);
                            do_search(event_target_value(&ev));
                        }
                        on:keydown=on_keydown
                    />
                    <Show when=move || is_searching.get()>
                        <span class="search-spinner"></span>
                    </Show>
                </div>
            </div>
            <div class="home-shelf-grid">
                <section class="home-shelf">
                    <div class="home-shelf-header">
                        <button class="section-more-btn" on:click=move |_| on_more_rankings()>"查看更多"</button>
                        <h2>"排行榜"</h2>
                        <span>{if loading {
                            "加载中".to_string()
                        } else {
                            let enabled = sources.iter().filter(|source| source.enabled).count();
                            format!("{enabled}/{} 个音源", sources.len())
                        }}</span>
                    </div>
                    <div class="home-list compact">
                        {if rankings.is_empty() {
                            view! { <div class="home-empty">"暂无排行榜数据"</div> }.into_any()
                        } else {
                            rankings.into_iter().take(6).enumerate().map(|(index, ranking)| {
                                let on_select = on_select_ranking.clone();
                                let item = ranking.clone();
                                view! {
                                    <button class="home-rank-item" on:click=move |_| on_select(item.clone())>
                                        <span class="home-rank-no">{index + 1}</span>
                                        <span class="home-item-main">
                                            <strong>{ranking.name}</strong>
                                            <small>{ranking.source_name}</small>
                                        </span>
                                    </button>
                                }.into_any()
                            }).collect::<Vec<_>>().into_any()
                        }}
                    </div>
                </section>
                <section class="home-shelf">
                    <div class="home-shelf-header">
                        <button class="section-more-btn" on:click=move |_| on_more_recommendations()>"查看更多"</button>
                        <h2>"推荐歌曲"</h2>
                        <span>"来自热门搜索"</span>
                    </div>
                    <div class="home-list">
                        {if hot_keywords.is_empty() {
                            view! { <div class="home-empty">"暂无推荐歌曲"</div> }.into_any()
                        } else {
                            hot_keywords.into_iter().take(6).map(|item| {
                                let keyword = item.title.clone();
                                let on_search = on_search.clone();
                                view! {
                                    <button class="home-song-item" on:click=move |_| on_search(keyword.clone())>
                                        <span class="home-song-cover hot">"热"</span>
                                        <span class="home-item-main">
                                            <strong>{item.title}</strong>
                                            <small>{item.source}</small>
                                        </span>
                                    </button>
                                }.into_any()
                            }).collect::<Vec<_>>().into_any()
                        }}
                    </div>
                </section>
            </div>
            <section class="home-shelf home-playlists">
                <div class="home-shelf-header">
                    <button class="section-more-btn" on:click=move |_| on_more_playlists()>"查看更多"</button>
                    <h2>"歌单"</h2>
                    <span>"精选歌单"</span>
                </div>
                <div class="home-playlist-grid">
                    {if playlists.is_empty() {
                        view! { <div class="home-empty">"暂无歌单数据"</div> }.into_any()
                    } else {
                        playlists.into_iter().take(10).map(|playlist| {
                            let on_select = on_select_playlist.clone();
                            let toggle = on_toggle_favorite_playlist.clone();
                            let fav = favorite_from_playlist(&playlist, None);
                            let fav_for_click = fav.clone();
                            let is_fav = favorites.playlists.iter().any(|item| item.id == fav.id && item.source_name == fav.source_name);
                            let item = playlist.clone();
                            let cover_url = playlist.cover.clone().unwrap_or_default();
                            let has_cover = !cover_url.trim().is_empty();
                            view! {
                                <button class="home-playlist-card" on:click=move |_| on_select(item.clone())>
                                    <span class="favorite-chip card" class:active=is_fav on:click=move |ev| {
                                        ev.stop_propagation();
                                        toggle(fav_for_click.clone());
                                    }>"♥"</span>
                                    <span class="home-playlist-cover">
                                        <Show
                                            when=move || has_cover
                                            fallback=move || view! { <span>"♫"</span> }
                                        >
                                            <img src=cover_url.clone() alt="playlist" />
                                        </Show>
                                    </span>
                                    <span class="home-playlist-name">{playlist.name}</span>
                                    <span class="home-playlist-meta">{playlist.song_count.map(|count| format!("{count} 首")).unwrap_or_else(|| "歌单".to_string())}" · "{playlist.source_name}</span>
                                </button>
                            }.into_any()
                        }).collect::<Vec<_>>().into_any()
                    }}
                </div>
            </section>
        </section>
    }
}

#[component]
fn ParserView(
    shared_url: RwSignal<String>,
    shared_playlist: Option<SharedPlaylist>,
    shared_loading: bool,
    shared_error: Option<String>,
    favorites: FavoritesData,
    on_parse: std::sync::Arc<dyn Fn(String) + Send + Sync>,
    on_play_song: std::sync::Arc<dyn Fn(String, String, String, usize, String) + Send + Sync>,
    on_toggle_favorite_song: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync>,
    on_toggle_favorite_playlist: std::sync::Arc<dyn Fn(FavoritePlaylist) + Send + Sync>,
) -> impl IntoView {
    let parse_click = {
        let on_parse = on_parse.clone();
        move |_| on_parse(shared_url.get_untracked())
    };
    let parse_keydown = {
        let on_parse = on_parse.clone();
        move |ev: leptos::ev::KeyboardEvent| {
            if ev.key() == "Enter" {
                on_parse(event_target_value(&ev));
            }
        }
    };
    view! {
        <section class="section parser-page">
            <div class="section-header">
                <h2 class="section-title">"解析分享链接"</h2>
                <span class="section-more">"支持歌单分享链接，解析后可收藏歌单"</span>
            </div>
            <div class="parser-card">
                <div class="search-bar parser-input">
                    <span class="search-icon">"Link"</span>
                    <input
                        class="search-input"
                        type="text"
                        placeholder="粘贴分享链接..."
                        prop:value=move || shared_url.get()
                        on:input=move |ev| shared_url.set(event_target_value(&ev))
                        on:keydown=parse_keydown
                    />
                </div>
                <button class="favorite-action parser-submit" on:click=parse_click>"开始解析"</button>
            </div>
            {render_shared_playlist_panel(
                shared_playlist,
                shared_loading,
                shared_error,
                favorites,
                on_play_song,
                on_toggle_favorite_song,
                on_toggle_favorite_playlist,
            )}
        </section>
    }
}

#[component]
fn RankingView(
    rankings: RwSignal<Vec<RankingCategory>>,
    ranking_songs: RwSignal<Vec<SongDetail>>,
    selected_ranking: RwSignal<Option<RankingCategory>>,
    loading: RwSignal<bool>,
    favorites: RwSignal<FavoritesData>,
    on_select_ranking: std::sync::Arc<dyn Fn(RankingCategory) + Send + Sync>,
    on_play_song: std::sync::Arc<dyn Fn(String, String, String, usize, String) + Send + Sync>,
    on_toggle_favorite_song: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync>,
) -> impl IntoView {
    let show_detail = move || selected_ranking.get().is_some();
    view! {
        <section class="section">
            <div class="section-header">
                <Show
                    when=show_detail
                    fallback=move || view! { <h2 class="section-title">"排行榜"</h2> }
                >
                    <button class="back-btn" on:click=move |_| selected_ranking.set(None)>"返回"</button>
                    <h2 class="section-title">{move || selected_ranking.get().map(|item| item.name).unwrap_or_default()}</h2>
                </Show>
            </div>
            {move || {
                if loading.get() && ranking_songs.get().is_empty() {
                    view! { <div class="loading-container"><div class="loading-spinner"></div><p>"加载中..."</p></div> }.into_any()
                } else if show_detail() {
                    render_song_list(
                        ranking_songs.get(),
                        on_play_song.clone(),
                        favorites.get(),
                        on_toggle_favorite_song.clone(),
                    ).into_any()
                } else {
                    rankings.get().into_iter().map(|ranking| {
                        let on_select = on_select_ranking.clone();
                        let item = ranking.clone();
                        view! {
                            <button class="ranking-card" on:click=move |_| on_select(item.clone())>
                                <div class="ranking-card-icon">"🏆"</div>
                                <div class="ranking-card-name">{ranking.name}</div>
                                <div class="ranking-card-source">{ranking.source_name}</div>
                            </button>
                        }.into_any()
                    }).collect::<Vec<_>>().into_any()
                }
            }}
        </section>
    }
}

#[component]
fn PlaylistView(
    playlists: RwSignal<Vec<PlaylistInfo>>,
    playlist_songs: RwSignal<Vec<SongDetail>>,
    selected_playlist: RwSignal<Option<PlaylistInfo>>,
    loading: RwSignal<bool>,
    favorites: RwSignal<FavoritesData>,
    on_select_playlist: std::sync::Arc<dyn Fn(PlaylistInfo) + Send + Sync>,
    on_play_song: std::sync::Arc<dyn Fn(String, String, String, usize, String) + Send + Sync>,
    on_toggle_favorite_song: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync>,
    on_toggle_favorite_playlist: std::sync::Arc<dyn Fn(FavoritePlaylist) + Send + Sync>,
) -> impl IntoView {
    let show_detail = move || selected_playlist.get().is_some();
    view! {
        <section class="section">
            <div class="section-header">
                <Show
                    when=show_detail
                    fallback=move || view! { <h2 class="section-title">"歌单"</h2> }
                >
                    <button class="back-btn" on:click=move |_| selected_playlist.set(None)>"返回"</button>
                    <h2 class="section-title">{move || selected_playlist.get().map(|item| item.name).unwrap_or_default()}</h2>
                </Show>
            </div>
            {move || {
                if loading.get() && playlist_songs.get().is_empty() {
                    view! { <div class="loading-container"><div class="loading-spinner"></div><p>"加载中..."</p></div> }.into_any()
                } else if show_detail() {
                    render_song_list(
                        playlist_songs.get(),
                        on_play_song.clone(),
                        favorites.get(),
                        on_toggle_favorite_song.clone(),
                    ).into_any()
                } else {
                    playlists.get().into_iter().map(|playlist| {
                        let on_select = on_select_playlist.clone();
                        let toggle = on_toggle_favorite_playlist.clone();
                        let fav = favorite_from_playlist(&playlist, None);
                        let fav_for_click = fav.clone();
                        let is_fav = favorites.get().playlists.iter().any(|item| item.id == fav.id && item.source_name == fav.source_name);
                        let item = playlist.clone();
                        let cover_url = playlist.cover.clone().unwrap_or_default();
                        let has_cover = !cover_url.trim().is_empty();
                        view! {
                            <div class="playlist-card" on:click=move |_| on_select(item.clone())>
                                <button class="favorite-chip" class:active=is_fav on:click=move |ev| {
                                    ev.stop_propagation();
                                    toggle(fav_for_click.clone());
                                }>"♥"</button>
                                <div class="playlist-card-cover">
                                    <Show
                                        when=move || has_cover
                                        fallback=move || view! { <span>"♫"</span> }
                                    >
                                        <img src=cover_url.clone() alt="playlist" />
                                    </Show>
                                </div>
                                <div class="playlist-card-name">{playlist.name}</div>
                                <div class="playlist-card-meta">
                                    <span>{playlist.source_name}</span>
                                    <span>{playlist.song_count.map(|count| format!("{count} 首")).unwrap_or_default()}</span>
                                </div>
                            </div>
                        }.into_any()
                    }).collect::<Vec<_>>().into_any()
                }
            }}
        </section>
    }
}

#[component]
fn SearchView(
    search_query: RwSignal<String>,
    search_results: RwSignal<Vec<SongResult>>,
    search_attempted: RwSignal<bool>,
    is_searching: RwSignal<bool>,
    ime_composing: RwSignal<bool>,
    schedule_search: std::sync::Arc<dyn Fn(String) + Send + Sync>,
    do_search: std::sync::Arc<dyn Fn(String) + Send + Sync>,
    favorites: FavoritesData,
    on_play_search_result: std::sync::Arc<dyn Fn(Vec<SongResult>, usize) + Send + Sync>,
    on_toggle_favorite_song: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync>,
) -> impl IntoView {
    let sort_mode = RwSignal::new(SearchSortMode::Source);
    let sort_desc = RwSignal::new(false);
    let on_input = {
        let schedule_search = schedule_search.clone();
        move |ev: leptos::ev::Event| {
            let value = event_target_value(&ev);
            search_query.set(value.clone());
            if ime_composing.get_untracked() || event_is_composing(&ev) {
                return;
            }
            schedule_search(value);
        }
    };
    let on_keydown = {
        let do_search = do_search.clone();
        move |ev: leptos::ev::KeyboardEvent| {
            if ev.key() == "Enter" && !ev.is_composing() {
                do_search(event_target_value(&ev));
            }
        }
    };
    let sorted_results = move || {
        let mut results = sort_search_results(search_results.get(), sort_mode.get());
        if sort_desc.get() {
            results.reverse();
        }
        results
    };
    view! {
        <section class="search-page-hero">
            <div class="hero-text">
                <h1 class="hero-greeting">"搜索音乐"</h1>
                <div class="search-bar">
                    <span class="search-icon">"Search"</span>
                    <input
                        class="search-input"
                        type="text"
                        placeholder="搜索音乐..."
                        prop:value=move || search_query.get()
                        on:input=on_input
                        on:compositionstart=move |_| ime_composing.set(true)
                        on:compositionend=move |ev: leptos::ev::CompositionEvent| {
                            ime_composing.set(false);
                            do_search(event_target_value(&ev));
                        }
                        on:keydown=on_keydown
                    />
                    <Show when=move || is_searching.get()>
                        <span class="search-spinner"></span>
                    </Show>
                </div>
                <div class="search-sort-strip">
                    <For
                        each=move || vec![
                            SearchSortMode::Source,
                            SearchSortMode::Quality,
                            SearchSortMode::Match,
                            SearchSortMode::Duration,
                            SearchSortMode::Title,
                        ]
                        key=|mode| *mode as u8
                        let:mode
                    >
                        <button
                            class="search-sort-btn"
                            class:active=move || sort_mode.get() == mode
                            on:click=move |_| {
                                if sort_mode.get_untracked() == mode {
                                    sort_desc.update(|desc| *desc = !*desc);
                                } else {
                                    sort_mode.set(mode);
                                    sort_desc.set(false);
                                }
                            }
                        >
                            {move || {
                                if sort_mode.get() == mode && sort_desc.get() {
                                    format!("{} ↓", mode.label())
                                } else {
                                    mode.label().to_string()
                                }
                            }}
                        </button>
                    </For>
                </div>
            </div>
        </section>
        <section class="section search-results-section">
            <div class="section-header">
                <h2 class="section-title">{move || {
                    let keyword = search_query.get();
                    if keyword.trim().is_empty() {
                        "输入关键词开始搜索".to_string()
                    } else {
                        format!("搜索结果: {keyword}")
                    }
                }}</h2>
                <span class="section-more">{move || {
                    let total = search_results.get().len();
                    if total >= 300 {
                        "共 300+ 首".to_string()
                    } else {
                        format!("共 {total} 首")
                    }
                }}</span>
            </div>
            <div class="search-results-list">
                {move || render_search_list(
                    sorted_results(),
                    search_attempted.get(),
                    is_searching.get(),
                    on_play_search_result.clone(),
                    favorites.clone(),
                    on_toggle_favorite_song.clone(),
                )}
            </div>
        </section>
    }
}

fn render_song_list(
    songs: Vec<SongDetail>,
    on_play: std::sync::Arc<dyn Fn(String, String, String, usize, String) + Send + Sync>,
    favorites: FavoritesData,
    on_toggle_favorite: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync>,
) -> Vec<leptos::prelude::AnyView> {
    songs
        .into_iter()
        .map(|song| {
            let play = on_play.clone();
            let play_song = song.clone();
            let fav = favorite_from_song_detail(&song);
            let fav_for_click = fav.clone();
            let is_fav = favorites
                .songs
                .iter()
                .any(|item| same_song_favorite(item, &fav));
            let toggle = on_toggle_favorite.clone();
            let cover_url = song.cover_url.clone().unwrap_or_default();
            let has_cover_url = cover_url.clone();
            let cover_text = song.title.chars().next().unwrap_or('♪').to_string();
            view! {
                <div class="song-list-item" on:click=move |_| play(play_song.title.clone(), play_song.artist.clone(), play_song.id.clone(), play_song.source_id, play_song.platform.clone())>
                    <span class="sli-play">"▶"</span>
                    <span class="sli-cover">
                        <Show
                            when=move || !has_cover_url.is_empty()
                            fallback=move || view! { <span>{cover_text.clone()}</span> }
                        >
                            <img
                                src=cover_url.clone()
                                alt="cover"
                                on:error=move |ev| {
                                    let element = event_target::<web_sys::HtmlElement>(&ev);
                                    let _ = element.style().set_property("display", "none");
                                }
                            />
                        </Show>
                    </span>
                    <div class="sli-info">
                        <span class="sli-title">{song.title}</span>
                        <span class="sli-artist">{song.artist}</span>
                    </div>
                    <span class="sli-album">{song.album.unwrap_or_default()}</span>
                    <span class="sli-duration">{song.duration.map(format_time).unwrap_or_default()}</span>
                    <button class="favorite-chip" class:active=is_fav on:click=move |ev| {
                        ev.stop_propagation();
                        toggle(fav_for_click.clone());
                    }>"♥"</button>
                </div>
            }.into_any()
        })
        .collect()
}

fn sort_search_results(mut results: Vec<SongResult>, mode: SearchSortMode) -> Vec<SongResult> {
    match mode {
        SearchSortMode::Source => {}
        SearchSortMode::Quality => {
            results.sort_by(|a, b| {
                quality_rank(b)
                    .cmp(&quality_rank(a))
                    .then_with(|| b.score.cmp(&a.score))
                    .then_with(|| a.source_id.cmp(&b.source_id))
            });
        }
        SearchSortMode::Match => {
            results.sort_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then_with(|| a.source_id.cmp(&b.source_id))
                    .then_with(|| a.title.cmp(&b.title))
            });
        }
        SearchSortMode::Duration => {
            results.sort_by(|a, b| {
                let left = a.duration.unwrap_or(f64::MAX);
                let right = b.duration.unwrap_or(f64::MAX);
                left.partial_cmp(&right)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.title.cmp(&b.title))
            });
        }
        SearchSortMode::Title => {
            results.sort_by(|a, b| {
                a.title
                    .cmp(&b.title)
                    .then_with(|| a.artist.cmp(&b.artist))
                    .then_with(|| a.source_id.cmp(&b.source_id))
            });
        }
    }
    results
}

fn quality_rank(song: &SongResult) -> i32 {
    match song.platform.as_str() {
        "kw" | "kg" | "tx" => 3,
        "wy" | "mg" => 2,
        _ => 1,
    }
}

fn search_result_key(song: &SongResult) -> String {
    if song.id.trim().is_empty() {
        format!("fallback::{}::{}::{}", song.title, song.artist, song.platform)
    } else {
        format!("{}::{}::{}", song.source_id, song.platform, song.id)
    }
}

fn merge_search_results(current: &mut Vec<SongResult>, incoming: Vec<SongResult>) {
    let mut seen = current
        .iter()
        .map(search_result_key)
        .collect::<std::collections::HashSet<_>>();
    for song in incoming {
        if seen.insert(search_result_key(&song)) {
            current.push(song);
        }
    }
}

fn dedupe_search_results(results: Vec<SongResult>) -> Vec<SongResult> {
    let mut out = Vec::new();
    merge_search_results(&mut out, results);
    out
}

fn render_search_list(
    results: Vec<SongResult>,
    attempted: bool,
    searching: bool,
    on_play: std::sync::Arc<dyn Fn(Vec<SongResult>, usize) + Send + Sync>,
    favorites: FavoritesData,
    on_toggle_favorite: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync>,
) -> Vec<leptos::prelude::AnyView> {
    if results.is_empty() {
        if searching {
            return vec![view! { <div class="loading-container"><div class="loading-spinner"></div><p>"正在搜索音源..."</p></div> }.into_any()];
        }
        if attempted {
            return vec![view! { <div class="loading-container"><p>"没有搜索到可展示的歌曲，请换个关键词或稍后重试"</p></div> }.into_any()];
        }
        return Vec::new();
    }
    let queue = results.clone();
    results
        .into_iter()
        .enumerate()
        .map(|(index, song)| {
            let queue_for_click = queue.clone();
            let play = on_play.clone();
            let fav = favorite_from_song_result(&song);
            let fav_for_click = fav.clone();
            let is_fav = favorites
                .songs
                .iter()
                .any(|item| same_song_favorite(item, &fav));
            let toggle = on_toggle_favorite.clone();
            let duration = song.duration.map(format_time).unwrap_or_else(|| "--:--".to_string());
            let cover_url = song.cover_url.clone().unwrap_or_default();
            let has_cover_url = cover_url.clone();
            let cover_text = song.title.chars().next().unwrap_or('♪').to_string();
            let quality = song.quality.clone().unwrap_or_else(|| "未知".to_string());
            view! {
                <div class="song-list-item" on:click=move |_| play(queue_for_click.clone(), index)>
                    <span class="sli-play">"▶"</span>
                    <span class="sli-cover">
                        <Show
                            when=move || !has_cover_url.is_empty()
                            fallback=move || view! { <span>{cover_text.clone()}</span> }
                        >
                            <img
                                src=cover_url.clone()
                                alt="cover"
                                on:error=move |ev| {
                                    let element = event_target::<web_sys::HtmlElement>(&ev);
                                    let _ = element.style().set_property("display", "none");
                                }
                            />
                        </Show>
                    </span>
                    <div class="sli-info">
                        <span class="sli-title">{song.title}</span>
                        <span class="sli-artist">{song.artist}</span>
                    </div>
                    <span class="sli-duration">{duration}</span>
                    <div class="sli-tags">
                        <span class="sli-tag">{quality}</span>
                        <span class="sli-source">{song.source}</span>
                        <span class="sli-tag">{song.platform}</span>
                    </div>
                    <button class="favorite-chip" class:active=is_fav on:click=move |ev| {
                        ev.stop_propagation();
                        toggle(fav_for_click.clone());
                    }>"♥"</button>
                </div>
            }
            .into_any()
        })
        .collect()
}

fn render_shared_playlist_panel(
    shared: Option<SharedPlaylist>,
    loading: bool,
    error: Option<String>,
    favorites: FavoritesData,
    on_play_song: std::sync::Arc<dyn Fn(String, String, String, usize, String) + Send + Sync>,
    on_toggle_song: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync>,
    on_toggle_playlist: std::sync::Arc<dyn Fn(FavoritePlaylist) + Send + Sync>,
) -> leptos::prelude::AnyView {
    if loading {
        return view! {
            <section class="home-shelf shared-playlist-panel">
                <div class="home-shelf-header"><h2>"酷狗分享歌单"</h2><span>"解析中"</span></div>
                <div class="home-empty">"正在解析你提供的分享链接..."</div>
            </section>
        }
        .into_any();
    }
    let Some(shared) = shared else {
        return view! {
            <section class="home-shelf shared-playlist-panel">
                <div class="home-shelf-header"><h2>"酷狗分享歌单"</h2><span>"未加载"</span></div>
                <div class="home-empty">{error.unwrap_or_else(|| "暂时没有解析到分享歌单".to_string())}</div>
            </section>
        }
        .into_any();
    };
    let playlist_fav = favorite_from_playlist(&shared.playlist, Some(shared.external_url.clone()));
    let playlist_for_click = playlist_fav.clone();
    let is_playlist_fav = favorites
        .playlists
        .iter()
        .any(|item| item.id == playlist_fav.id && item.source_name == playlist_fav.source_name);
    let toggle_playlist = on_toggle_playlist.clone();
    let note_for_check = shared.note.clone();
    let note_for_text = shared.note.clone().unwrap_or_default();
    let songs = shared.songs.clone();
    view! {
        <section class="home-shelf shared-playlist-panel">
            <div class="home-shelf-header">
                <h2>{shared.playlist.name}</h2>
                <button class="favorite-action" class:active=is_playlist_fav on:click=move |_| toggle_playlist(playlist_for_click.clone())>
                    {if is_playlist_fav { "已收藏歌单" } else { "收藏此歌单" }}
                </button>
            </div>
            <Show when=move || note_for_check.is_some()>
                <div class="favorite-note">{note_for_text.clone()}</div>
            </Show>
            <div class="search-results-list shared-list">
                {if songs.is_empty() {
                    view! { <div class="home-empty">"已识别歌单链接，但暂时没有拿到歌曲列表。"</div> }.into_any()
                } else {
                    render_song_list(songs, on_play_song, favorites, on_toggle_song).into_any()
                }}
            </div>
        </section>
    }
    .into_any()
}

fn render_favorites_view(
    data: FavoritesData,
    on_play_song: std::sync::Arc<dyn Fn(String, String, String, usize, String) + Send + Sync>,
    on_toggle_song: std::sync::Arc<dyn Fn(FavoriteSong) + Send + Sync>,
    on_toggle_playlist: std::sync::Arc<dyn Fn(FavoritePlaylist) + Send + Sync>,
) -> impl IntoView {
    let songs = data.songs.clone();
    let playlists = data.playlists.clone();
    view! {
        <section class="section favorites-page">
            <div class="section-header">
                <h2 class="section-title">"我的收藏"</h2>
                <span class="section-more">"缓存保存在本地，删除缓存后收藏会失效"</span>
            </div>
            <div class="favorites-grid">
                <section class="home-shelf">
                    <div class="home-shelf-header"><h2>"收藏歌曲"</h2><span>{format!("{} 首", songs.len())}</span></div>
                    <div class="home-list">
                        {if songs.is_empty() {
                            view! { <div class="home-empty">"还没有收藏歌曲"</div> }.into_any()
                        } else {
                            songs.into_iter().map(|song| {
                                let play = on_play_song.clone();
                                let remove = on_toggle_song.clone();
                                let play_song = song.clone();
                                let remove_song = song.clone();
                                view! {
                                    <div class="favorite-row">
                                        <button class="favorite-main" on:click=move |_| play(play_song.title.clone(), play_song.artist.clone(), play_song.id.clone(), play_song.source_id, play_song.platform.clone())>
                                            <span class="favorite-cover">"♪"</span>
                                            <span class="home-item-main"><strong>{song.title}</strong><small>{song.artist}</small></span>
                                        </button>
                                        <button class="favorite-remove" on:click=move |_| remove(remove_song.clone())>"取消收藏"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>().into_any()
                        }}
                    </div>
                </section>
                <section class="home-shelf">
                    <div class="home-shelf-header"><h2>"收藏歌单"</h2><span>{format!("{} 个", playlists.len())}</span></div>
                    <div class="home-list">
                        {if playlists.is_empty() {
                            view! { <div class="home-empty">"还没有收藏歌单"</div> }.into_any()
                        } else {
                            playlists.into_iter().map(|playlist| {
                                let remove = on_toggle_playlist.clone();
                                let remove_playlist = playlist.clone();
                                view! {
                                    <div class="favorite-row">
                                        <div class="favorite-main static">
                                            <span class="favorite-cover playlist">"♫"</span>
                                            <span class="home-item-main"><strong>{playlist.name}</strong><small>{playlist.source_name}</small></span>
                                        </div>
                                        <button class="favorite-remove" on:click=move |_| remove(remove_playlist.clone())>"取消收藏"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>().into_any()
                        }}
                    </div>
                </section>
            </div>
        </section>
    }
}

#[component]
fn RightPanel(
    sources: RwSignal<Vec<SourceInfo>>,
    active_source: RwSignal<Option<SourceInfo>>,
    hot_keywords: RwSignal<Vec<HotItem>>,
    on_search: std::sync::Arc<dyn Fn(String) + Send + Sync>,
) -> impl IntoView {
    let sources_open = RwSignal::new(false);
    view! {
        <aside class="right-panel">
            <div class="right-panel-section">
                <h3 class="rp-title">"音源状态"</h3>
                <button class="rp-source-summary" on:click=move |_| sources_open.update(|open| *open = !*open)>
                    <span class="rp-summary-main"><span class="rp-dot"></span><span>{move || active_source.get().map(|source| source.name).unwrap_or_else(|| "全部音源".to_string())}</span></span>
                    <span class="rp-summary-meta">{move || format!("{} 个启用", sources.get().into_iter().filter(|source| source.enabled).count())}</span>
                    <span class="rp-chevron">{move || if sources_open.get() { "收起" } else { "展开" }}</span>
                </button>
                <div class="rp-sources" class:open=move || sources_open.get()>
                    <For each=move || sources.get() key=|source| source.id let:source>
                        <div class="rp-source-item" class:active=move || active_source.get().is_some_and(|active| active.id == source.id) class:disabled=move || !source.enabled>
                            <span class="rp-dot"></span>
                            <span class="rp-name">{source.name}</span>
                            <span class="rp-state">{if source.enabled { "启用" } else { "禁用" }}</span>
                        </div>
                    </For>
                </div>
            </div>
            <div class="right-panel-section">
                <h3 class="rp-title">"热门搜索"</h3>
                <div class="rp-hot-list">
                    {move || hot_keywords.get().into_iter().take(8).map(|item| {
                        let keyword = item.title.clone();
                        let on_search = on_search.clone();
                        view! { <button class="rp-hot-item" on:click=move |_| on_search(keyword.clone())>{item.title}</button> }
                    }).collect_view()}
                </div>
            </div>
            <div class="right-panel-section">
                <h3 class="rp-title">"快捷操作"</h3>
                <div class="rp-actions">
                    <button class="rp-action-btn">"刷新音源"</button>
                    <button class="rp-action-btn">"切换音源"</button>
                </div>
            </div>
        </aside>
    }
}

fn load_favorites() -> FavoritesData {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(FAVORITES_KEY).ok().flatten())
        .and_then(|raw| serde_json::from_str::<FavoritesData>(&raw).ok())
        .unwrap_or_default()
}

fn save_favorites(data: &FavoritesData) {
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        if let Ok(raw) = serde_json::to_string(data) {
            let _ = storage.set_item(FAVORITES_KEY, &raw);
        }
    }
}

fn same_song_favorite(left: &FavoriteSong, right: &FavoriteSong) -> bool {
    if !left.id.is_empty() || !right.id.is_empty() {
        left.id == right.id && left.platform == right.platform
    } else {
        left.title == right.title && left.artist == right.artist
    }
}

fn favorite_from_song_result(song: &SongResult) -> FavoriteSong {
    FavoriteSong {
        id: song.id.clone(),
        title: song.title.clone(),
        artist: song.artist.clone(),
        album: song.album.clone(),
        cover_url: song.cover_url.clone(),
        duration: song.duration,
        source_id: song.source_id,
        source: song.source.clone(),
        platform: song.platform.clone(),
    }
}

fn favorite_from_song_detail(song: &SongDetail) -> FavoriteSong {
    FavoriteSong {
        id: song.id.clone(),
        title: song.title.clone(),
        artist: song.artist.clone(),
        album: song.album.clone(),
        cover_url: song.cover_url.clone(),
        duration: song.duration,
        source_id: song.source_id,
        source: song.platform.clone(),
        platform: song.platform.clone(),
    }
}

fn favorite_from_playlist(
    playlist: &PlaylistInfo,
    external_url: Option<String>,
) -> FavoritePlaylist {
    FavoritePlaylist {
        id: playlist.id.clone(),
        name: playlist.name.clone(),
        cover: playlist.cover.clone(),
        song_count: playlist.song_count,
        source_id: playlist.source_id,
        source_name: playlist.source_name.clone(),
        external_url,
    }
}

fn format_time(secs: f64) -> String {
    let total = secs.max(0.0) as i32;
    format!("{:02}:{:02}", total / 60, total % 60)
}

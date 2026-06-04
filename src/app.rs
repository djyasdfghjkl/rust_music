use crate::home::HomePage;
use crate::main_page::MainPage;
use leptos::prelude::*;

/// 欢迎页 localStorage key
const WELCOME_SEEN_KEY: &str = "miku_tunes_welcome_seen";

fn has_seen_welcome() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(WELCOME_SEEN_KEY).ok().flatten())
        .map(|v| v == "true")
        .unwrap_or(false)
}

fn mark_welcome_seen() {
    if let Some(storage) = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
    {
        let _ = storage.set_item(WELCOME_SEEN_KEY, "true");
    }
}

#[component]
pub fn App() -> impl IntoView {
    let showing_welcome = RwSignal::new(!has_seen_welcome());

    let on_start = move || {
        mark_welcome_seen();
        showing_welcome.set(false);
    };

    view! {
        <Show
            when=move || showing_welcome.get()
            fallback=move || view! { <MainPage /> }
        >
            <HomePage on_start />
        </Show>
    }
}

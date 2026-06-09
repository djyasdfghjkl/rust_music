mod app;
mod config;
mod home;
mod main_page;
mod player;
mod tauri_utils;
mod types;

use app::*;
use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| {
        view! {
            <App/>
        }
    })
}

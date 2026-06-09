use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::window;

#[cfg(target_arch = "wasm32")]
macro_rules! log {
    ($($arg:expr),*) => {{
        $(web_sys::console::log_1(&JsValue::from($arg));)*
    }};
}

#[cfg(not(target_arch = "wasm32"))]
macro_rules! log {
    ($($arg:expr),*) => {};
}

/// Invoke a Tauri command from the frontend.
/// Tries `window.__TAURI_INTERNALS__.invoke(cmd, args)` first (Tauri 2.x),
/// falls back to `window.__TAURI__.invoke(cmd, args)` (Tauri 1.x).
pub async fn invoke(cmd: &str, args: Option<&js_sys::Object>) -> Result<JsValue, JsValue> {
    let w = window().ok_or_else(|| JsValue::from_str("no window"))?;

    let mut invoke_found = false;
    let mut invoke_fn: Option<js_sys::Function> = None;

    // Try Tauri 2.x API: __TAURI_INTERNALS__.invoke
    log!("invoke called, cmd=", cmd);

    if let Ok(internals) = js_sys::Reflect::get(&w, &JsValue::from_str("__TAURI_INTERNALS__")) {
        log!("__TAURI_INTERNALS__ found");
        if !internals.is_undefined() {
            if let Ok(f) = js_sys::Reflect::get(&internals, &JsValue::from_str("invoke")) {
                log!("__TAURI_INTERNALS__.invoke found");
                if !f.is_undefined() {
                    if let Ok(func) = f.dyn_into::<js_sys::Function>() {
                        invoke_fn = Some(func);
                        invoke_found = true;
                    }
                }
            }
        }
    }

    // Fall back to Tauri 1.x API: __TAURI__.invoke
    if !invoke_found {
        log!("falling back to __TAURI__.invoke");
        let tauri = js_sys::Reflect::get(&w, &JsValue::from_str("__TAURI__"))
            .map_err(|_| JsValue::from_str("__TAURI__ not available"))?;
        let f = js_sys::Reflect::get(&tauri, &JsValue::from_str("invoke"))
            .map_err(|_| JsValue::from_str("invoke not available"))?;
        invoke_fn = Some(f.dyn_into()?);
    }

    let f = invoke_fn.ok_or_else(|| JsValue::from_str("invoke function not found"))?;

    let cmd_val = JsValue::from_str(cmd);
    let args_val: JsValue = match args {
        Some(a) => a.into(),
        None => JsValue::null(),
    };

    let promise = if invoke_found {
        // Tauri 2.x: invoke(cmd, args, options)
        log!("calling Tauri 2.x invoke");
        f.call3(&w, &cmd_val, &args_val, &JsValue::UNDEFINED)?
    } else {
        // Tauri 1.x: invoke(cmd, args)
        log!("calling Tauri 1.x invoke");
        f.call2(&w, &cmd_val, &args_val)?
    };
    log!("invoke promise created");
    let result = JsFuture::from(js_sys::Promise::from(promise)).await;
    log!("invoke promise done");
    #[cfg(target_arch = "wasm32")]
    {
        match &result {
            Ok(value) => {
                log!("result=", value);
            }
            Err(error) => {
                log!("error=", error);
            }
        }
    }
    result
}

pub async fn listen(event: &str, handler: &js_sys::Function) -> Result<JsValue, JsValue> {
    let w = window().ok_or_else(|| JsValue::from_str("no window"))?;
    let tauri = js_sys::Reflect::get(&w, &JsValue::from_str("__TAURI__"))
        .map_err(|_| JsValue::from_str("__TAURI__ not available"))?;
    let event_api = js_sys::Reflect::get(&tauri, &JsValue::from_str("event"))
        .map_err(|_| JsValue::from_str("__TAURI__.event not available"))?;
    let listen_fn = js_sys::Reflect::get(&event_api, &JsValue::from_str("listen"))
        .map_err(|_| JsValue::from_str("listen not available"))?
        .dyn_into::<js_sys::Function>()?;
    let promise = listen_fn.call2(&event_api, &JsValue::from_str(event), handler)?;
    JsFuture::from(js_sys::Promise::from(promise)).await
}

pub fn convert_file_src(path: &str, protocol: Option<&str>) -> String {
    let w = match window() {
        Some(window) => window,
        None => return path.to_string(),
    };

    let internals = match js_sys::Reflect::get(&w, &JsValue::from_str("__TAURI_INTERNALS__")) {
        Ok(value) if !value.is_undefined() && !value.is_null() => value,
        _ => return path.to_string(),
    };

    let func = match js_sys::Reflect::get(&internals, &JsValue::from_str("convertFileSrc")) {
        Ok(value) => match value.dyn_into::<js_sys::Function>() {
            Ok(function) => function,
            Err(_) => return path.to_string(),
        },
        Err(_) => return path.to_string(),
    };

    let protocol = protocol.unwrap_or("asset");
    match func.call2(
        &internals,
        &JsValue::from_str(path),
        &JsValue::from_str(protocol),
    ) {
        Ok(value) => value.as_string().unwrap_or_else(|| path.to_string()),
        Err(_) => path.to_string(),
    }
}

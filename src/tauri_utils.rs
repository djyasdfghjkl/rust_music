use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::window;

/// Invoke a Tauri command from the frontend.
/// Calls `window.__TAURI.invoke(cmd, args)` with proper two-argument calling convention.
pub async fn invoke(cmd: &str, args: Option<&js_sys::Object>) -> Result<JsValue, JsValue> {
    let w = window().ok_or_else(|| JsValue::from_str("no window"))?;
    let tauri = js_sys::Reflect::get(&w, &JsValue::from_str("__TAURI"))
        .map_err(|_| JsValue::from_str("__TAURI not available"))?;
    let invoke_fn = js_sys::Reflect::get(&tauri, &JsValue::from_str("invoke"))
        .map_err(|_| JsValue::from_str("invoke not available"))?;
    let f: js_sys::Function = invoke_fn.dyn_into()?;

    let cmd_val = JsValue::from_str(cmd);
    let args_val: JsValue = match args {
        Some(a) => a.into(),
        None => JsValue::null(),
    };

    let promise = f.call2(&w, &cmd_val, &args_val)?;
    JsFuture::from(js_sys::Promise::from(promise)).await
}

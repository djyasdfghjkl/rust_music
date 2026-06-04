use crate::config::AppConfig;
use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

/// 粒子结构
#[derive(Clone)]
struct Particle {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    life: f64,
    max_life: f64,
    size: f64,
    hue: f64,
}

/// 欢迎页组件 - Canvas 手写动画 + 开始使用按钮
#[component]
pub fn HomePage(on_start: impl Fn() + 'static) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let description = AppConfig::NAME_ZH;

    Effect::new(move |_| {
        let canvas = canvas_ref.get();
        if canvas.is_none() {
            return;
        }
        let canvas = canvas.unwrap();
        let js_canvas: &HtmlCanvasElement = canvas.as_ref();

        let dpr = web_sys::window().unwrap().device_pixel_ratio();
        let width = 960.0;
        let height = 360.0;
        js_canvas.set_width((width * dpr) as u32);
        js_canvas.set_height((height * dpr) as u32);
        js_canvas
            .set_attribute("style", "width:960px;height:360px")
            .ok();

        let ctx = js_canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|o| o.dyn_into::<CanvasRenderingContext2d>().ok());

        if ctx.is_none() {
            return;
        }
        let ctx = ctx.unwrap();

        run_animation(ctx, "Miku Tunes", width, height, dpr);
    });

    view! {
        <div class="welcome-page">
            <canvas node_ref={canvas_ref}></canvas>

            <p class="welcome-description">{description}</p>

            <button class="start-btn" on:click=move |_| on_start()>
                "开始使用"
            </button>

            <footer class="welcome-footer">
                <a href={AppConfig::GITHUB_URL} target="_blank" rel="noopener noreferrer">
                    "GitHub"
                </a>
                <span class="footer-dot">" · "</span>
                <span>{"v"}{AppConfig::VERSION}</span>
            </footer>
        </div>
    }
}

fn run_animation(
    ctx: CanvasRenderingContext2d,
    text: &str,
    w: f64,
    h: f64,
    dpr: f64,
) {
    let font_size = 72.0;
    ctx.set_font(&format!(
        "bold {}px 'Inter','PingFang SC','Microsoft YaHei',sans-serif",
        font_size
    ));
    ctx.set_text_baseline("middle");
    ctx.set_text_align("center");

    let text_width = ctx
        .measure_text(text)
        .ok()
        .map(|m| m.width())
        .unwrap_or(400.0);

    let cx = w / 2.0;
    let cy = h / 2.0;
    let start_x = cx - text_width / 2.0;

    let animation_duration = 3000.0f64;
    let start_time = js_sys::Date::now();
    let text_owned = text.to_string();
    let particles = std::cell::RefCell::new(Vec::<Particle>::new());

    let rc_closure = std::rc::Rc::new(std::cell::RefCell::new(None::<Closure<dyn FnMut()>>));
    let rc_weak = rc_closure.clone();

    *rc_closure.borrow_mut() = Some(Closure::wrap(Box::new(move || {
        let now = js_sys::Date::now();
        let elapsed = now - start_time;
        let raw_p = (elapsed / animation_duration).min(1.0);
        let display_p = ease_out_cubic(raw_p);

        ctx.save();
        ctx.scale(dpr, dpr).ok();
        ctx.clear_rect(0.0, 0.0, w, h);
        ctx.restore();

        ctx.save();
        ctx.scale(dpr, dpr).ok();

        let gradient = ctx
            .create_linear_gradient(start_x, 0.0, start_x + text_width, 0.0);

        gradient.add_color_stop(0.0, "#39C5BB").ok();
        gradient.add_color_stop(0.5, "#6C8BFF").ok();
        gradient.add_color_stop(1.0, "#FF9EC5").ok();

        ctx.set_fill_style_canvas_gradient(&gradient);
        ctx.set_font(&format!(
            "bold {}px 'Inter','PingFang SC','Microsoft YaHei',sans-serif",
            font_size
        ));
        ctx.set_text_baseline("middle");
        ctx.set_text_align("left");

        let current_w = text_width * display_p;

        ctx.save();
        ctx.begin_path();
        ctx.rect(start_x - 10.0, cy - font_size * 0.8, current_w + 20.0, font_size * 2.0);
        ctx.clip();

        ctx.set_shadow_color("rgba(57, 197, 187, 0.3)");
        ctx.set_shadow_blur(20.0);
        ctx.fill_text(&text_owned, start_x, cy).ok();
        ctx.set_shadow_color("rgba(108, 139, 255, 0.2)");
        ctx.set_shadow_blur(35.0);
        ctx.fill_text(&text_owned, start_x, cy).ok();
        ctx.set_shadow_blur(0.0);
        ctx.fill_text(&text_owned, start_x, cy).ok();
        ctx.restore();

        if raw_p < 1.0 {
            let write_x = start_x + current_w;

            let tip_grad = ctx
                .create_radial_gradient(write_x, cy, 0.0, write_x, cy, 24.0)
                .unwrap();
            tip_grad.add_color_stop(0.0, "rgba(108, 139, 255, 0.9)").ok();
            tip_grad.add_color_stop(0.5, "rgba(57, 197, 187, 0.4)").ok();
            tip_grad.add_color_stop(1.0, "rgba(57, 197, 187, 0)").ok();
            ctx.set_fill_style_canvas_gradient(&tip_grad);
            ctx.begin_path();
            ctx.arc(write_x, cy, 24.0, 0.0, std::f64::consts::PI * 2.0).ok();
            ctx.fill();

            ctx.set_fill_style_str("rgba(255, 255, 255, 0.9)");
            ctx.begin_path();
            ctx.arc(write_x, cy, 4.0, 0.0, std::f64::consts::PI * 2.0).ok();
            ctx.fill();
        }

        {
            let mut ps = particles.borrow_mut();
            if raw_p < 1.0 {
                let write_x = start_x + current_w;
                for _ in 0..2 {
                    let angle = js_sys::Math::random() * std::f64::consts::PI * 2.0;
                    let speed = 0.5 + js_sys::Math::random() * 1.5;
                    ps.push(Particle {
                        x: write_x, y: cy,
                        vx: angle.cos() * speed, vy: angle.sin() * speed,
                        life: 1.0, max_life: 0.6 + js_sys::Math::random() * 0.8,
                        size: 1.5 + js_sys::Math::random() * 3.0,
                        hue: 170.0 + js_sys::Math::random() * 110.0,
                    });
                }
            }
            if raw_p >= 1.0 && elapsed - animation_duration < 2500.0 {
                for _ in 0..1 {
                    let angle = js_sys::Math::random() * std::f64::consts::PI * 2.0;
                    let speed = 0.3 + js_sys::Math::random() * 0.8;
                    ps.push(Particle {
                        x: cx + (js_sys::Math::random() - 0.5) * text_width * 0.8,
                        y: cy + (js_sys::Math::random() - 0.5) * 40.0,
                        vx: angle.cos() * speed, vy: angle.sin() * speed - 0.5,
                        life: 1.0, max_life: 0.8 + js_sys::Math::random() * 1.2,
                        size: 1.0 + js_sys::Math::random() * 2.0,
                        hue: 180.0 + js_sys::Math::random() * 150.0,
                    });
                }
            }
            ps.retain(|p| p.life > 0.0);
            for p in ps.iter_mut() {
                p.x += p.vx; p.y += p.vy; p.vy += 0.02;
                p.life -= 1.0 / 60.0 / p.max_life;
                let alpha = p.life.max(0.0);
                ctx.set_fill_style_str(&format!("hsla({}, 80%, 70%, {})", p.hue as i32, alpha));
                ctx.begin_path();
                ctx.arc(p.x, p.y, p.size * alpha, 0.0, std::f64::consts::PI * 2.0).ok();
                ctx.fill();
            }
        }
        ctx.restore();

        let has_particles = particles.borrow().len() > 0;
        if raw_p < 1.0 || has_particles {
            if let Some(ref cb) = *rc_weak.borrow() {
                web_sys::window()
                    .unwrap()
                    .request_animation_frame(cb.as_ref().unchecked_ref())
                    .ok();
            }
        }
    }) as Box<dyn FnMut()>));

    if let Some(ref cb) = *rc_closure.borrow() {
        web_sys::window()
            .unwrap()
            .request_animation_frame(cb.as_ref().unchecked_ref())
            .ok();
    };
}

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

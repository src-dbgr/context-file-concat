#![cfg(not(debug_assertions))]
use mime_guess::mime;
use rust_embed::RustEmbed;
use std::borrow::Cow;

/// Built UI-files (Vite `src/ui/dist`) are embedded into Binary
#[derive(RustEmbed)]
#[folder = "src/ui/dist"]
#[include = "**/*"]
pub struct UiAssets;

/// Provides (Bytes, Content-Type) for a requested resource.
/// - Normal calse: requested file
/// - SPA-Fallback: `index.html`, if path missing/404
pub fn load(path: &str) -> Option<(Cow<'static, [u8]>, String)> {
    let norm = normalize(path);
    if let Some(file) = UiAssets::get(&norm) {
        let ct = content_type(&norm);
        Some((file.data, ct))
    } else if norm != "index.html" {
        UiAssets::get("index.html").map(|f| (f.data, String::from("text/html; charset=utf-8")))
    } else {
        None
    }
}

fn normalize(raw: &str) -> String {
    let p = raw.trim_start_matches('/').trim();
    if p.is_empty() {
        "index.html".into()
    } else {
        p.to_string()
    }
}

fn content_type(path: &str) -> String {
    let guess = mime_guess::from_path(path).first_or(mime::APPLICATION_OCTET_STREAM);
    match guess.type_() {
        mime::TEXT | mime::APPLICATION if guess.subtype() == mime::JAVASCRIPT => {
            "application/javascript; charset=utf-8".into()
        }
        mime::TEXT | mime::APPLICATION if guess.subtype() == mime::JSON => {
            "application/json; charset=utf-8".into()
        }
        mime::TEXT | mime::APPLICATION if guess.subtype() == mime::XML => {
            "application/xml; charset=utf-8".into()
        }
        mime::TEXT if guess.subtype() == mime::PLAIN => "text/plain; charset=utf-8".into(),
        mime::TEXT if guess.subtype() == mime::HTML => "text/html; charset=utf-8".into(),
        mime::TEXT if guess.subtype() == "css" => "text/css; charset=utf-8".into(),
        _ => guess.essence_str().to_string(),
    }
}

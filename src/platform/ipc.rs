//! The gate every IPC message passes before it is looked at.
//!
//! wry hands the IPC handler a request whose URI is the URL of the document the
//! message was posted from, and whose body is the message. Only the UI itself may
//! send commands, so a message from any other document is dropped here, unparsed.

use super::origin::AppOrigin;
use wry::http::Request;

/// The message of `request` if it was posted by a page of `origin`; `None` (and a
/// warning in the log) for a message from anywhere else.
pub fn accepted_message(origin: &AppOrigin, request: Request<String>) -> Option<String> {
    let source = request.uri().to_string();
    if origin.is_own_page(&source) {
        Some(request.into_body())
    } else {
        tracing::warn!("Dropping an IPC message from a foreign origin: {source}");
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::origin::OriginForm;

    const MESSAGE: &str = r#"{"command":"initialize","payload":null}"#;

    /// A request as wry builds it: the URL of the posting document and the message.
    fn request_from(source: &str) -> Request<String> {
        Request::builder()
            .uri(source)
            .body(MESSAGE.to_string())
            .unwrap_or_else(|e| panic!("{source}: {e}"))
    }

    const NATIVE: AppOrigin = AppOrigin::new("cfc", "index.html", OriginForm::Native);
    const REWRITTEN: AppOrigin = AppOrigin::new(
        "cfc",
        "index.html",
        OriginForm::Rewritten { transport: "http" },
    );

    #[test]
    fn message_of_the_ui_is_handed_on_unchanged() {
        let dev = AppOrigin::dev_server();
        for (origin, source) in [
            (NATIVE, "cfc://index.html/"),
            (NATIVE, "cfc://index.html/?budget=1"),
            (REWRITTEN, "http://cfc.index.html/"),
            (dev, "http://localhost:1420/"),
        ] {
            assert_eq!(
                accepted_message(&origin, request_from(source)).as_deref(),
                Some(MESSAGE),
                "{origin:?} {source}"
            );
        }
    }

    #[test]
    fn message_of_any_other_document_is_dropped() {
        let dev = AppOrigin::dev_server();
        for (origin, source) in [
            (NATIVE, "https://example.com/"),
            (NATIVE, "http://cfc.index.html/"),
            (NATIVE, "cfc://other.html/"),
            (NATIVE, "cfc://index.html.example.com/"),
            (NATIVE, "about:blank"),
            (NATIVE, "data:text/html,x"),
            (REWRITTEN, "https://cfc.index.html/"),
            (REWRITTEN, "http://cfc.index.html.example.com/"),
            (REWRITTEN, "http://cfc.index.html@example.com/"),
            (REWRITTEN, "cfc://index.html/"),
            (REWRITTEN, "http://localhost:1420/"),
            (dev, "http://localhost:1421/"),
            (dev, "http://example.com/"),
            (dev, "cfc://index.html/"),
        ] {
            // Some of these are no URIs the `http` crate accepts; wry drops such a
            // message before the handler is called.
            let Ok(request) = Request::builder().uri(source).body(MESSAGE.to_string()) else {
                continue;
            };
            assert_eq!(
                accepted_message(&origin, request),
                None,
                "{origin:?} {source}"
            );
        }
    }

    /// The URIs the gate has to tell apart do reach it: the test above skips none of
    /// the cases that matter.
    #[test]
    fn foreign_web_and_lookalike_sources_are_valid_uris() {
        for source in [
            "https://example.com/",
            "cfc://other.html/",
            "http://cfc.index.html.example.com/",
            "http://cfc.index.html@example.com/",
            "http://localhost:1421/",
        ] {
            assert!(Request::builder().uri(source).body(()).is_ok(), "{source}");
        }
    }
}

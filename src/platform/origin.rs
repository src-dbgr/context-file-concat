//! The origin the UI is served from, and what belongs to it.
//!
//! The embedded UI is served through a custom protocol. How that protocol shows up in
//! a URL depends on the web engine:
//!
//! - macOS / Linux (WKWebView, WebKitGTK): natively, `cfc://index.html/...`
//! - Windows / Android (WebView2, Android WebView): no custom schemes, so wry rewrites
//!   `<scheme>://` to `http://<scheme>.` (or `https://<scheme>.` with
//!   `with_https_scheme(true)`), i.e. `http://cfc.index.html/...`
//!
//! Everything the engine reports (the target of a navigation, the page an IPC message
//! comes from) is in the rewritten form there. The policies built on the origin
//! (`navigation`, `csp`, the IPC check in `main.rs`) are therefore decided against the
//! form of the target platform. All of it is pure.

/// How the web engine exposes the origin of the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginForm {
    /// `<scheme>://<host>/...`
    Native,
    /// `<transport>://<scheme>.<host>/...`, where `transport` is `http` or `https`.
    Rewritten { transport: &'static str },
}

impl OriginForm {
    /// The form wry uses for a custom protocol on the platform this binary is
    /// compiled for.
    ///
    /// `Rewritten` uses `http` because `with_https_scheme` is left at its default.
    #[cfg_attr(all(debug_assertions, not(test)), allow(dead_code))]
    pub const fn for_current_platform() -> Self {
        if cfg!(any(target_os = "windows", target_os = "android")) {
            OriginForm::Rewritten { transport: "http" }
        } else {
            OriginForm::Native
        }
    }
}

/// The origin the UI is served from.
#[derive(Debug, Clone, Copy)]
pub struct AppOrigin {
    pub(super) scheme: &'static str,
    pub(super) host: &'static str,
    pub(super) form: OriginForm,
}

impl AppOrigin {
    /// `scheme` and `host` must be lowercase ASCII; `host` is the whole authority
    /// (it includes the port, if there is one).
    pub const fn new(scheme: &'static str, host: &'static str, form: OriginForm) -> Self {
        Self { scheme, host, form }
    }

    /// The origin of the UI embedded into a release build: the custom protocol `cfc`.
    #[cfg_attr(all(debug_assertions, not(test)), allow(dead_code))]
    pub const fn embedded() -> Self {
        Self::new("cfc", "index.html", OriginForm::for_current_platform())
    }

    /// The origin of the Vite dev server a debug build loads the UI from.
    #[cfg_attr(not(any(test, debug_assertions)), allow(dead_code))]
    pub const fn dev_server() -> Self {
        Self::new("http", "localhost:1420", OriginForm::Native)
    }

    /// Name of the custom protocol to register with the WebView.
    #[cfg_attr(all(debug_assertions, not(test)), allow(dead_code))]
    pub const fn scheme(&self) -> &'static str {
        self.scheme
    }

    /// URL to hand to the WebView on start. Always the native form: wry applies the
    /// platform rewrite itself.
    pub fn start_url(&self) -> String {
        format!("{}://{}", self.scheme, self.host)
    }

    /// True if `url`, as the web engine reports it, is a page of this origin.
    ///
    /// This is what decides whether an IPC message is accepted: only the UI itself may
    /// send commands, not a document of another origin that ended up in the WebView
    /// (`about:blank`, `data:`, `blob:`, a web page, a frame).
    ///
    /// Scheme and authority have to match exactly: a prefix test would also accept
    /// `http://cfc.index.html.example.com/` or `http://cfc.index.html@example.com/`.
    pub fn is_own_page(&self, url: &str) -> bool {
        let Some((scheme, rest)) = url.split_once(':') else {
            return false;
        };
        let Some(authority) = authority(rest) else {
            return false;
        };
        match self.form {
            OriginForm::Native => {
                scheme.eq_ignore_ascii_case(self.scheme)
                    && authority.eq_ignore_ascii_case(self.host)
            }
            OriginForm::Rewritten { transport } => {
                scheme.eq_ignore_ascii_case(transport)
                    && authority.eq_ignore_ascii_case(&format!("{}.{}", self.scheme, self.host))
            }
        }
    }
}

/// The authority (`[userinfo@]host[:port]`) of the part of a URL that follows
/// `<scheme>:`; `None` if the URL has none.
fn authority(after_scheme: &str) -> Option<&str> {
    after_scheme
        .strip_prefix("//")?
        .split(['/', '?', '#', '\\'])
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NATIVE: AppOrigin = AppOrigin::new("cfc", "index.html", OriginForm::Native);
    const HTTP: AppOrigin = AppOrigin::new(
        "cfc",
        "index.html",
        OriginForm::Rewritten { transport: "http" },
    );
    const HTTPS: AppOrigin = AppOrigin::new(
        "cfc",
        "index.html",
        OriginForm::Rewritten { transport: "https" },
    );

    #[test]
    fn start_url_is_always_the_native_form() {
        for origin in [NATIVE, HTTP, HTTPS] {
            assert_eq!(origin.start_url(), "cfc://index.html");
            assert_eq!(origin.scheme(), "cfc");
        }
        assert_eq!(AppOrigin::dev_server().start_url(), "http://localhost:1420");
    }

    #[test]
    fn current_platform_form_matches_wry() {
        let expected = if cfg!(any(target_os = "windows", target_os = "android")) {
            OriginForm::Rewritten { transport: "http" }
        } else {
            OriginForm::Native
        };
        assert_eq!(OriginForm::for_current_platform(), expected);
        assert_eq!(AppOrigin::embedded().form, expected);
        assert_eq!(AppOrigin::embedded().start_url(), "cfc://index.html");
    }

    /// (origin, URL as the engine reports it, is it a page of the origin?)
    #[test]
    fn own_pages_are_told_from_everything_else() {
        let dev = AppOrigin::dev_server();
        let cases: &[(AppOrigin, &str, bool)] = &[
            // macOS / Linux
            (NATIVE, "cfc://index.html", true),
            (NATIVE, "cfc://index.html/", true),
            (NATIVE, "cfc://index.html/assets/app.js?v=1#top", true),
            (NATIVE, "CFC://Index.HTML/", true),
            (NATIVE, "cfc://other.html/", false),
            (NATIVE, "cfc://index.html.example.com/", false),
            (NATIVE, "cfc://user@index.html/", false),
            (NATIVE, "cfc://index.html:80/", false),
            (NATIVE, "cfc:index.html", false),
            (NATIVE, "cfcx://index.html/", false),
            (NATIVE, "http://cfc.index.html/", false),
            (NATIVE, "https://cfc.index.html/", false),
            // Windows / Android
            (HTTP, "http://cfc.index.html", true),
            (HTTP, "http://cfc.index.html/", true),
            (HTTP, "http://cfc.index.html/a/b?c#d", true),
            (HTTP, "HTTP://CFC.Index.Html/", true),
            (HTTP, "https://cfc.index.html/", false),
            (HTTP, "cfc://index.html/", false),
            (HTTP, "http://cfc.index.html.example.com/", false),
            (HTTP, "http://cfc.index.html@example.com/", false),
            (HTTP, "http://user@cfc.index.html/", false),
            (HTTP, "http://cfc.index.html:8080/", false),
            (HTTP, "http://example.com/cfc.index.html", false),
            (HTTP, "http://example.com/?u=http://cfc.index.html/", false),
            (HTTP, "http:cfc.index.html", false),
            (HTTP, "http:/cfc.index.html/", false),
            (HTTPS, "https://cfc.index.html/", true),
            (HTTPS, "http://cfc.index.html/", false),
            // Debug builds
            (dev, "http://localhost:1420/", true),
            (dev, "http://localhost:1420/?budget=1", true),
            (dev, "http://localhost:1421/", false),
            (dev, "http://localhost/", false),
            (dev, "https://localhost:1420/", false),
            (dev, "http://localhost:1420.example.com/", false),
            (dev, "http://localhost:1420@example.com/", false),
        ];
        for (origin, url, expected) in cases {
            assert_eq!(origin.is_own_page(url), *expected, "{origin:?} {url}");
        }
    }

    #[test]
    fn documents_without_an_origin_of_their_own_are_never_own_pages() {
        for origin in [NATIVE, HTTP, HTTPS, AppOrigin::dev_server()] {
            for url in [
                "",
                "no-scheme",
                "about:blank",
                "about:srcdoc",
                "data:text/html,<script>window.ipc.postMessage('x')</script>",
                "blob:cfc://index.html/1b2c",
                "blob:http://cfc.index.html/1b2c",
                "blob:http://localhost:1420/1b2c",
                "file:///index.html",
                "javascript:alert(1)",
                "https://example.com/",
            ] {
                assert!(!origin.is_own_page(url), "{origin:?} {url:?}");
            }
        }
    }
}

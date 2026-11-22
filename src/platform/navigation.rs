//! Navigation policy for the embedded WebView.
//!
//! The UI is served through a custom protocol. How that protocol shows up in a URL
//! depends on the web engine:
//!
//! - macOS / Linux (WKWebView, WebKitGTK): natively, `cfc://index.html/...`
//! - Windows / Android (WebView2, Android WebView): no custom schemes, so wry rewrites
//!   `<scheme>://` to `http://<scheme>.` (or `https://<scheme>.` with
//!   `with_https_scheme(true)`), i.e. `http://cfc.index.html/...`
//!
//! The navigation handler sees the rewritten form. Classifying every `http(s)` URL as
//! external would therefore block the app's own page on Windows, so the decision is
//! made here against the origin form of the target platform. The function is pure;
//! acting on the decision (opening the browser) is left to the caller.

/// What to do with a navigation request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDecision {
    /// Load inside the WebView.
    Allow,
    /// Do not load; hand the URL to the system browser instead.
    OpenExternally,
    /// Do not load at all (e.g. `file://`).
    Block,
}

/// How the web engine exposes the app's custom protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginForm {
    /// `<scheme>://<host>/...`
    Native,
    /// `<transport>://<scheme>.<host>/...`, where `transport` is `http` or `https`.
    Rewritten { transport: &'static str },
}

impl OriginForm {
    /// The form wry uses on the platform this binary is compiled for.
    ///
    /// `Rewritten` uses `http` because `with_https_scheme` is left at its default.
    pub const fn for_current_platform() -> Self {
        if cfg!(any(target_os = "windows", target_os = "android")) {
            OriginForm::Rewritten { transport: "http" }
        } else {
            OriginForm::Native
        }
    }
}

/// The origin the embedded UI is served from.
#[derive(Debug, Clone, Copy)]
pub struct AppOrigin {
    scheme: &'static str,
    host: &'static str,
    form: OriginForm,
}

impl AppOrigin {
    /// `scheme` and `host` must be lowercase ASCII.
    pub const fn new(scheme: &'static str, host: &'static str, form: OriginForm) -> Self {
        Self { scheme, host, form }
    }

    /// Name of the custom protocol to register with the WebView.
    pub const fn scheme(&self) -> &'static str {
        self.scheme
    }

    /// URL to hand to the WebView on start. Always the native form: wry applies the
    /// platform rewrite itself.
    pub fn start_url(&self) -> String {
        format!("{}://{}", self.scheme, self.host)
    }

    /// Decides what should happen with a navigation to `url`.
    pub fn classify(&self, url: &str) -> NavigationDecision {
        let Some((scheme, rest)) = url.split_once(':') else {
            return NavigationDecision::Block;
        };

        if scheme.eq_ignore_ascii_case(self.scheme) {
            return NavigationDecision::Allow;
        }

        match scheme.to_ascii_lowercase().as_str() {
            "about" | "blob" | "data" => NavigationDecision::Allow,
            "http" | "https" if self.is_rewritten_own_origin(scheme, rest) => {
                NavigationDecision::Allow
            }
            "http" | "https" => NavigationDecision::OpenExternally,
            _ => NavigationDecision::Block,
        }
    }

    /// True if `<scheme>:<rest>` is the app's own origin in its rewritten form.
    ///
    /// The host has to match exactly: a prefix test would also accept
    /// `http://cfc.index.html.example.com/` or `http://cfc.index.html@example.com/`.
    fn is_rewritten_own_origin(&self, scheme: &str, rest: &str) -> bool {
        let OriginForm::Rewritten { transport } = self.form else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case(transport) {
            return false;
        }
        let Some(after_slashes) = rest.strip_prefix("//") else {
            return false;
        };
        let authority = after_slashes
            .split(['/', '?', '#', '\\'])
            .next()
            .unwrap_or_default();

        authority.eq_ignore_ascii_case(&format!("{}.{}", self.scheme, self.host))
    }
}

#[cfg(test)]
mod tests {
    use super::NavigationDecision::{Allow, Block, OpenExternally};
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
    const ALL: [AppOrigin; 3] = [NATIVE, HTTP, HTTPS];

    #[test]
    fn start_url_is_always_the_native_form() {
        for origin in ALL {
            assert_eq!(origin.start_url(), "cfc://index.html");
            assert_eq!(origin.scheme(), "cfc");
        }
    }

    #[test]
    fn current_platform_form_matches_wry() {
        let expected = if cfg!(any(target_os = "windows", target_os = "android")) {
            OriginForm::Rewritten { transport: "http" }
        } else {
            OriginForm::Native
        };
        assert_eq!(OriginForm::for_current_platform(), expected);
    }

    #[test]
    fn native_form_allows_the_custom_scheme() {
        // macOS / Linux
        for url in [
            "cfc://index.html",
            "cfc://index.html/",
            "cfc://index.html/assets/app.js?v=1#top",
            "CFC://index.html/",
        ] {
            assert_eq!(NATIVE.classify(url), Allow, "{url}");
        }
    }

    #[test]
    fn native_form_treats_the_rewritten_host_as_external() {
        assert_eq!(NATIVE.classify("http://cfc.index.html/"), OpenExternally);
        assert_eq!(NATIVE.classify("https://cfc.index.html/"), OpenExternally);
    }

    #[test]
    fn http_rewrite_allows_the_own_origin() {
        // Windows / Android default
        for url in [
            "http://cfc.index.html",
            "http://cfc.index.html/",
            "http://cfc.index.html/assets/app.js",
            "http://cfc.index.html?x=1",
            "http://cfc.index.html#top",
            "HTTP://CFC.Index.Html/",
        ] {
            assert_eq!(HTTP.classify(url), Allow, "{url}");
        }
        // Only the configured transport is the app; the other one is a real web URL.
        assert_eq!(HTTP.classify("https://cfc.index.html/"), OpenExternally);
    }

    #[test]
    fn https_rewrite_allows_the_own_origin() {
        // Windows / Android with `with_https_scheme(true)`
        assert_eq!(HTTPS.classify("https://cfc.index.html/"), Allow);
        assert_eq!(HTTPS.classify("https://cfc.index.html/a/b?c#d"), Allow);
        assert_eq!(HTTPS.classify("http://cfc.index.html/"), OpenExternally);
    }

    #[test]
    fn lookalike_hosts_are_external() {
        for url in [
            "http://cfc.index.html.example.com/",
            "http://cfc.index.html@example.com/",
            "http://user@cfc.index.html/",
            "http://cfc.index.html:8080/",
            "http://cfc.example.com/",
            "http://cfcxindex.html/",
            "http://xcfc.index.html/",
            "http://example.com/cfc.index.html",
            "http://example.com/?u=http://cfc.index.html/",
            "http:cfc.index.html",
        ] {
            assert_eq!(HTTP.classify(url), OpenExternally, "{url}");
        }
    }

    #[test]
    fn external_links_open_in_the_browser_on_every_platform() {
        for origin in ALL {
            for url in [
                "https://example.com/",
                "http://example.com/page?x=1",
                "HTTPS://EXAMPLE.COM/",
                "http://localhost:1420/",
            ] {
                assert_eq!(origin.classify(url), OpenExternally, "{url}");
            }
        }
    }

    #[test]
    fn engine_internal_schemes_are_allowed_on_every_platform() {
        for origin in ALL {
            for url in [
                "about:blank",
                "about:srcdoc",
                "blob:cfc://index.html/1b2c",
                "blob:http://cfc.index.html/1b2c",
                "data:text/html,hi",
                "cfc://index.html/",
            ] {
                assert_eq!(origin.classify(url), Allow, "{url}");
            }
        }
    }

    #[test]
    fn everything_else_is_blocked_on_every_platform() {
        for origin in ALL {
            for url in [
                "file:///etc/passwd",
                "FILE:///C:/Windows/win.ini",
                "javascript:alert(1)",
                "ftp://example.com/",
                "mailto:someone@example.com",
                "cfcx://index.html",
                "no-scheme",
                "",
            ] {
                assert_eq!(origin.classify(url), Block, "{url:?}");
            }
        }
    }
}

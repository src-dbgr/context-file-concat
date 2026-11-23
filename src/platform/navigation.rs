//! Navigation policy for the embedded WebView.
//!
//! The navigation handler sees URLs in the form of the target platform (see
//! [`super::origin`]). Classifying every `http(s)` URL as external would block the
//! app's own page on Windows, so the decision is made against that form. The function
//! is pure; acting on the decision (opening the browser) is left to the caller.

use super::origin::{AppOrigin, OriginForm};

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

impl AppOrigin {
    /// Decides what should happen with a navigation to `url`.
    ///
    /// Only the UI itself and `about:` (the blank document the engine starts from) are
    /// loaded. `data:` and `blob:` documents are blocked: the UI never navigates to
    /// one, and such a document would run whatever markup it was made from. (The
    /// workers of the editor are no navigations and are not affected; what they may
    /// load is the business of the content security policy, see [`super::csp`].)
    pub fn classify(&self, url: &str) -> NavigationDecision {
        let Some((scheme, _)) = url.split_once(':') else {
            return NavigationDecision::Block;
        };

        // The start URL is handed to the engine in the native form on every platform.
        let native = AppOrigin::new(self.scheme, self.host, OriginForm::Native);
        if self.is_own_page(url) || native.is_own_page(url) {
            return NavigationDecision::Allow;
        }

        match scheme.to_ascii_lowercase().as_str() {
            "about" => NavigationDecision::Allow,
            "http" | "https" => NavigationDecision::OpenExternally,
            _ => NavigationDecision::Block,
        }
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
    fn the_ui_and_the_blank_document_are_allowed_on_every_platform() {
        for origin in ALL {
            for url in [
                "about:blank",
                "about:srcdoc",
                "ABOUT:blank",
                "cfc://index.html",
                "cfc://index.html/",
            ] {
                assert_eq!(origin.classify(url), Allow, "{url}");
            }
        }
    }

    /// Documents made from markup (`data:`, `blob:`) are no part of the UI, wherever
    /// the blob was created.
    #[test]
    fn data_and_blob_documents_are_blocked_on_every_platform() {
        for origin in ALL {
            for url in [
                "data:text/html,hi",
                "DATA:text/html;base64,PHNjcmlwdD4=",
                "blob:cfc://index.html/1b2c",
                "blob:http://cfc.index.html/1b2c",
                "BLOB:https://example.com/1b2c",
            ] {
                assert_eq!(origin.classify(url), Block, "{url}");
            }
        }
    }

    /// The custom protocol serves the UI from one host only.
    #[test]
    fn other_hosts_of_the_custom_scheme_are_blocked() {
        for origin in ALL {
            for url in [
                "cfc://other.html/",
                "cfc://index.html.example.com/",
                "cfc://user@index.html/",
                "cfc:index.html",
            ] {
                assert_eq!(origin.classify(url), Block, "{url}");
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

//! The content security policy of the embedded UI.
//!
//! The policy is sent as a header with every response of the custom protocol (see
//! [`super::asset_protocol`]), which makes it the one source of truth: it covers the
//! document and the workers, and nothing in the markup can weaken it. The dev build
//! loads the UI from the Vite dev server, which needs inline scripts and a WebSocket
//! for hot reloading, and carries no policy.
//!
//! What the UI needs was read off the built bundle (`src/ui/dist`):
//!
//! | Directive     | Value                    | Why                                                       |
//! | ------------- | ------------------------ | --------------------------------------------------------- |
//! | `default-src` | `'none'`                 | Whatever is not listed below is not loaded                |
//! | `script-src`  | own origin               | Bundled modules only: no inline script, no `eval`         |
//! | `style-src`   | own origin + inline      | Monaco creates `<style>` elements and style attributes    |
//! | `img-src`     | own origin + `data:`     | Icons embedded into the style sheets as `data:` URIs      |
//! | `font-src`    | own origin               | The icon font of Monaco (`codicon.ttf`)                   |
//! | `connect-src` | own origin               | Vite's preload helper fetches the UI's own chunks         |
//! | `worker-src`  | own origin               | The Monaco workers are files of the bundle, not `blob:`s  |
//!
//! `object-src`, `base-uri`, `form-action` and `frame-ancestors` are `'none'`: the UI
//! embeds no plugins, has no `<base>`, submits no forms and is never framed.

use super::origin::{AppOrigin, OriginForm};

impl AppOrigin {
    /// The value of the `Content-Security-Policy` header for the UI of this origin.
    pub fn content_security_policy(&self) -> String {
        let own = self.csp_source();
        [
            "default-src 'none'".to_string(),
            format!("script-src {own}"),
            format!("style-src {own} 'unsafe-inline'"),
            format!("img-src {own} data:"),
            format!("font-src {own}"),
            format!("connect-src {own}"),
            format!("worker-src {own}"),
            "object-src 'none'".to_string(),
            "base-uri 'none'".to_string(),
            "form-action 'none'".to_string(),
            "frame-ancestors 'none'".to_string(),
        ]
        .join("; ")
    }

    /// The source expression for "this origin".
    ///
    /// Where the custom scheme is used natively, the scheme is named as well: engines
    /// differ in whether `'self'` matches an origin that is neither `http` nor
    /// `https`. Everything of that scheme is served by the application itself.
    fn csp_source(&self) -> String {
        match self.form {
            OriginForm::Native => format!("'self' {}:", self.scheme),
            OriginForm::Rewritten { .. } => "'self'".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NATIVE: AppOrigin = AppOrigin::new("cfc", "index.html", OriginForm::Native);
    const REWRITTEN: AppOrigin = AppOrigin::new(
        "cfc",
        "index.html",
        OriginForm::Rewritten { transport: "http" },
    );

    #[test]
    fn policy_of_the_native_form() {
        assert_eq!(
            NATIVE.content_security_policy(),
            "default-src 'none'; \
             script-src 'self' cfc:; \
             style-src 'self' cfc: 'unsafe-inline'; \
             img-src 'self' cfc: data:; \
             font-src 'self' cfc:; \
             connect-src 'self' cfc:; \
             worker-src 'self' cfc:; \
             object-src 'none'; \
             base-uri 'none'; \
             form-action 'none'; \
             frame-ancestors 'none'"
        );
    }

    #[test]
    fn policy_of_the_rewritten_form() {
        assert_eq!(
            REWRITTEN.content_security_policy(),
            "default-src 'none'; \
             script-src 'self'; \
             style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; \
             font-src 'self'; \
             connect-src 'self'; \
             worker-src 'self'; \
             object-src 'none'; \
             base-uri 'none'; \
             form-action 'none'; \
             frame-ancestors 'none'"
        );
    }

    /// The directive `name` of `policy`, without the name.
    fn directive<'a>(policy: &'a str, name: &str) -> &'a str {
        policy
            .split("; ")
            .find_map(|directive| directive.strip_prefix(name)?.strip_prefix(' '))
            .unwrap_or_else(|| panic!("no {name} in {policy}"))
    }

    #[test]
    fn scripts_are_never_inline_evaluated_or_remote() {
        for origin in [NATIVE, REWRITTEN] {
            let policy = origin.content_security_policy();
            for directive in [
                directive(&policy, "script-src"),
                directive(&policy, "worker-src"),
            ] {
                for forbidden in ["unsafe", "data:", "blob:", "http", "*"] {
                    assert!(!directive.contains(forbidden), "{directive}");
                }
            }
            assert_eq!(directive(&policy, "default-src"), "'none'");
            // Nothing leaves the application: the only origin named is its own.
            for forbidden in ["http:", "https:", "ws:", "wss:", "*"] {
                assert!(!policy.contains(forbidden), "{policy}");
            }
        }
    }

    /// Fails if `html` relies on something `script-src` does not allow: every
    /// `<script>` has to load a file, and there are no inline event handlers.
    #[track_caller]
    fn assert_no_inline_script(html: &str) {
        let scripts: Vec<&str> = html
            .match_indices("<script")
            .map(|(start, _)| {
                let tag_end = html[start..].find('>').expect("closed tag") + start;
                &html[start..=tag_end]
            })
            .collect();
        assert!(!scripts.is_empty());
        for tag in scripts {
            assert!(tag.contains(" src=\""), "inline script: {tag}");
        }
        for handler in [" onload=", " onclick=", " onerror=", "javascript:"] {
            assert!(!html.contains(handler), "{handler}");
        }
        // A policy in the markup would only add a second place to maintain.
        assert!(!html.contains("Content-Security-Policy"));
    }

    const UI_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui");

    /// `script-src` allows no inline script, so the page must not rely on one.
    #[test]
    fn index_html_has_no_inline_script() {
        let html = std::fs::read_to_string(format!("{UI_DIR}/index.html")).expect("index.html");
        assert_no_inline_script(&html);
    }

    /// The page as the release build embeds it: the bundler must not have turned
    /// anything into an inline script either. The built UI is not part of the
    /// repository; without it there is nothing to check.
    #[test]
    fn built_index_html_has_no_inline_script() {
        let built = format!("{UI_DIR}/dist/index.html");
        let Ok(html) = std::fs::read_to_string(&built) else {
            eprintln!("SKIPPED: {built} does not exist (run `npm run build` in src/ui).");
            return;
        };
        assert_no_inline_script(&html);
    }

    #[test]
    fn inline_scripts_and_handlers_are_recognised() {
        let rejects = |html: &str| {
            let html = html.to_string();
            std::panic::catch_unwind(move || assert_no_inline_script(&html)).is_err()
        };
        assert!(!rejects(r#"<script src="./a.js"></script>"#));
        assert!(!rejects(
            r#"<script type="module" crossorigin src="./a.js"></script>"#
        ));
        assert!(rejects("<script>alert(1)</script>"));
        assert!(rejects(
            r#"<script src="./a.js"></script><script type="module">x()</script>"#
        ));
        assert!(rejects(
            r#"<script src="./a.js"></script><body onload="x()">"#
        ));
        assert!(rejects("<p>no script at all</p>"));
    }
}

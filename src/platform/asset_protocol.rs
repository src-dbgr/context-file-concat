//! Responses of the custom protocol the embedded UI is served through.
//!
//! Building the response is separated from finding the asset, so the headers every
//! response has to carry can be tested without a WebView and without a built UI.

use std::borrow::Cow;
use wry::http::{header, Response, StatusCode};

/// The body of a response: the bytes of an embedded asset.
pub type Body = Cow<'static, [u8]>;

/// Builds the response for a requested asset: `asset` is its content and content
/// type, or `None` if there is no such asset.
///
/// Every response, found or not, carries the content security policy `csp` and
/// forbids content type sniffing. The policy has to come with the scripts of the
/// workers too, which is why it is not tied to the document.
pub fn respond(asset: Option<(Body, String)>, csp: &str) -> Response<Body> {
    let (status, content_type, body) = match asset {
        Some((bytes, content_type)) => (StatusCode::OK, content_type, bytes),
        None => (
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8".to_string(),
            Cow::Borrowed(b"Not Found".as_slice()),
        ),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_SECURITY_POLICY, csp)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(body)
        .unwrap_or_else(|error| {
            // Only a header value that is no valid header can get here.
            tracing::error!("Failed to build the response for an asset: {error}");
            let mut response = Response::new(Cow::Borrowed(b"".as_slice()));
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::origin::AppOrigin;

    fn header_of<'a>(response: &'a Response<Body>, name: &str) -> &'a str {
        response
            .headers()
            .get(name)
            .unwrap_or_else(|| panic!("no {name} header"))
            .to_str()
            .unwrap()
    }

    #[test]
    fn found_asset_is_served_with_its_type_and_the_policy() {
        let csp = AppOrigin::embedded().content_security_policy();
        let asset = (
            Cow::Borrowed(b"<!doctype html>".as_slice()),
            "text/html; charset=utf-8".to_string(),
        );

        let response = respond(Some(asset), &csp);

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body().as_ref(), b"<!doctype html>");
        assert_eq!(
            header_of(&response, "content-type"),
            "text/html; charset=utf-8"
        );
        assert_eq!(header_of(&response, "content-security-policy"), csp);
        assert_eq!(header_of(&response, "x-content-type-options"), "nosniff");
    }

    /// Scripts (the workers among them) get the policy as well as the document.
    #[test]
    fn every_content_type_carries_the_policy() {
        let csp = AppOrigin::embedded().content_security_policy();
        for content_type in ["application/javascript; charset=utf-8", "font/ttf"] {
            let asset = (Cow::Borrowed(b"x".as_slice()), content_type.to_string());
            let response = respond(Some(asset), &csp);
            assert_eq!(header_of(&response, "content-security-policy"), csp);
        }
    }

    #[test]
    fn missing_asset_is_a_404_that_still_carries_the_policy() {
        let response = respond(None, "default-src 'none'");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.body().as_ref(), b"Not Found");
        assert_eq!(
            header_of(&response, "content-security-policy"),
            "default-src 'none'"
        );
        assert_eq!(header_of(&response, "x-content-type-options"), "nosniff");
    }

    #[test]
    fn unusable_header_value_yields_an_error_response_instead_of_a_panic() {
        let response = respond(None, "line\nbreak");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// The policy of every platform form is a valid header value.
    #[test]
    fn the_real_policy_is_a_valid_header_value() {
        use crate::platform::origin::OriginForm;
        for form in [
            OriginForm::Native,
            OriginForm::Rewritten { transport: "http" },
        ] {
            let csp = AppOrigin::new("cfc", "index.html", form).content_security_policy();
            assert_eq!(respond(None, &csp).status(), StatusCode::NOT_FOUND);
        }
    }
}

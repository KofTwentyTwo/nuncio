//! HTML email sandboxing, tracking beacon defense, and `nuncio-mail://` asset scheme protocol.

/// Security sanitizer constructing sandboxed `<iframe srcdoc="...">` wrappers for email body rendering.
pub struct HtmlSanitizer;

impl HtmlSanitizer {
    /// Content Security Policy (CSP) blocking remote image tracking pixels and disabling external scripts.
    pub const SECURE_CSP: &'static str =
        "default-src 'none'; img-src nuncio-mail: data:; style-src 'unsafe-inline';";

    /// Build a sandboxed HTML iframe wrapper with CSP and JS disabled.
    pub fn build_sandboxed_iframe(raw_html: &str) -> String {
        let escaped_html = raw_html
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");

        format!(
            r#"<iframe sandbox="allow-same-origin" csp="{}" srcdoc="{}"></iframe>"#,
            Self::SECURE_CSP,
            escaped_html
        )
    }

    /// Resolve attachment asset URI via custom `nuncio-mail://` scheme.
    pub fn format_attachment_uri(attachment_id: &str) -> String {
        format!("nuncio-mail://attachment/{}", attachment_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_sandboxed_iframe_disables_js_and_enforces_csp() {
        let raw = "<p>Hello <script>alert(1)</script></p>";
        let iframe = HtmlSanitizer::build_sandboxed_iframe(raw);

        assert!(iframe.contains(r#"sandbox="allow-same-origin""#));
        assert!(iframe.contains("img-src nuncio-mail:"));
        assert!(iframe.contains("&lt;script&gt;"));
    }

    #[test]
    fn format_attachment_uri_scheme() {
        let uri = HtmlSanitizer::format_attachment_uri("att-100");
        assert_eq!(uri, "nuncio-mail://attachment/att-100");
    }
}

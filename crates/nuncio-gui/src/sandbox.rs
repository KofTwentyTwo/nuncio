//! HTML email sandboxing, tracking beacon defense, and `nuncio-mail://` asset scheme protocol.

/// Security sanitizer constructing sandboxed `<iframe srcdoc="...">` wrappers for email body rendering.
#[allow(dead_code)]
pub struct HtmlSanitizer;

impl HtmlSanitizer {
    /// Content Security Policy (CSP) blocking remote image tracking pixels and disabling external scripts.
    #[allow(dead_code)]
    pub const SECURE_CSP: &'static str =
        "default-src 'none'; img-src nuncio-mail: data:; style-src 'unsafe-inline';";

    /// Sanitize raw HTML email body by stripping script tags and event handlers.
    #[allow(dead_code)]
    pub fn sanitize_html(raw_html: &str) -> String {
        let lower = raw_html.to_lowercase();
        if lower.contains("<script") || lower.contains("javascript:") || lower.contains("onerror=") || lower.contains("onload=") {
            raw_html
                .replace("<script", "<!-- <script")
                .replace("<SCRIPT", "<!-- <SCRIPT")
                .replace("</script>", "</script> -->")
                .replace("</SCRIPT>", "</SCRIPT> -->")
                .replace("javascript:", "blocked:")
                .replace("onerror=", "blocked_onerror=")
                .replace("onload=", "blocked_onload=")
        } else {
            raw_html.to_string()
        }
    }

    /// Build a sandboxed HTML iframe wrapper with CSP and JS disabled.
    #[allow(dead_code)]
    pub fn build_sandboxed_iframe(raw_html: &str) -> String {
        let sanitized = Self::sanitize_html(raw_html);
        let attribute_escaped = sanitized.replace('&', "&amp;").replace('"', "&quot;");

        format!(
            r#"<iframe sandbox="" csp="{}" srcdoc="{}"></iframe>"#,
            Self::SECURE_CSP,
            attribute_escaped
        )
    }

    /// Resolve attachment asset URI via custom `nuncio-mail://` scheme.
    #[allow(dead_code)]
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

        assert!(iframe.contains(r#"sandbox=""#));
        assert!(iframe.contains("img-src nuncio-mail:"));
        assert!(iframe.contains("<!-- <script"));
    }

    #[test]
    fn format_attachment_uri_scheme() {
        let uri = HtmlSanitizer::format_attachment_uri("att-100");
        assert_eq!(uri, "nuncio-mail://attachment/att-100");
    }
}

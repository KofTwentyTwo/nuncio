//! HTML to ANSI terminal plaintext renderer wrapper for email preview.

/// Renderer converting HTML body strings into wrapped terminal plaintext.
pub struct HtmlRenderer;

impl HtmlRenderer {
    /// Render HTML body string into plaintext formatted for terminal reader width.
    pub fn render_html(html: &str, width: usize) -> String {
        let max_width = if width == 0 { 80 } else { width };
        html2text::from_read(html.as_bytes(), max_width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_html_paragraph_and_links() {
        let html = "<h1>Meeting Notes</h1><p>Discussing <b>Nuncio</b> release.</p>";
        let rendered = HtmlRenderer::render_html(html, 60);

        assert!(rendered.contains("Meeting Notes"));
        assert!(rendered.contains("Nuncio"));
    }

    #[test]
    fn render_empty_or_zero_width_uses_default() {
        let rendered = HtmlRenderer::render_html("<p>test</p>", 0);
        assert!(rendered.contains("test"));
    }
}

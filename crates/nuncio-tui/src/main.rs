//! Nuncio Terminal UI application entry point.

mod html;
mod keybindings;
mod layout;

use html::HtmlRenderer;
use keybindings::{KeybindingEngine, UserAction};
use layout::{ActivePane, AppLayout};
use ratatui::layout::Rect;

fn main() {
    let area = Rect::new(0, 0, 120, 40);
    let (sidebar, list, reader) = AppLayout::compute_layout(area);
    let focused = ActivePane::Sidebar;

    let sample_rendered = HtmlRenderer::render_html("<h1>Nuncio</h1>", 40);

    println!(
        "Nuncio TUI Layout Active: sidebar={}px, list={}px, reader={}px (focused: {:?}, rendered: {})",
        sidebar.width, list.width, reader.width, focused, sample_rendered.trim()
    );
}

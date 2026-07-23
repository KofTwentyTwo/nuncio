//! Nuncio Terminal UI application entry point.

mod app;
mod html;
mod keybindings;
mod layout;

use app::TuiApp;
use html::HtmlRenderer;
use keybindings::{KeybindingEngine, UserAction};
use layout::{ActivePane, AppLayout};
use nuncio_core::EventBus;
use ratatui::layout::Rect;

fn main() {
    let bus = EventBus::new();
    let app = TuiApp::new(bus);

    let area = Rect::new(0, 0, 120, 40);
    let (sidebar, list, reader) = AppLayout::compute_layout(area);
    let sample_rendered = HtmlRenderer::render_html("<h1>Nuncio</h1>", 40);

    println!(
        "Nuncio TUI Shell Active: sidebar={}px, list={}px, reader={}px (focused: {:?}, running: {}, rendered: {})",
        sidebar.width, list.width, reader.width, app.active_pane(), app.is_running(), sample_rendered.trim()
    );
}

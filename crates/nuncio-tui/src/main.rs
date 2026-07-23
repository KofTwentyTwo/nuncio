//! Nuncio Terminal UI application entry point.

mod app;
mod html;
mod keybindings;
mod layout;

use app::TuiApp;
use keybindings::{KeybindingEngine, UserAction};
use layout::AppLayout;
use nuncio_core::EventBus;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = EventBus::new();
    let mut app = TuiApp::new(bus);

    println!("Nuncio TUI Application Shell initialized.");

    // Terminal widget frame rendering demo
    let area = ratatui::layout::Rect::new(0, 0, 120, 40);
    let (sidebar, list, reader) = AppLayout::compute_layout(area);

    println!(
        "Layout Computed: Sidebar: {}px, List: {}px, Reader: {}px (Active Focus: {:?})",
        sidebar.width, list.width, reader.width, app.active_pane()
    );

    Ok(())
}

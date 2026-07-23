//! Nuncio Terminal UI application entry point.

mod layout;

use layout::{ActivePane, AppLayout};
use ratatui::layout::Rect;

fn main() {
    let area = Rect::new(0, 0, 120, 40);
    let (sidebar, list, reader) = AppLayout::compute_layout(area);
    let focused = ActivePane::Sidebar;

    println!(
        "Nuncio TUI Layout Active: sidebar={}px, list={}px, reader={}px (focused: {:?})",
        sidebar.width, list.width, reader.width, focused
    );
}

//! Multi-pane Ratatui layout split and focus management for terminal UI.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};

/// Active focused pane in the terminal UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivePane {
    /// Folder navigation sidebar (left 20%).
    #[default]
    Sidebar,
    /// Message list table (middle 35%).
    MessageList,
    /// Envelope preview reader (right 45%).
    Reader,
}

impl ActivePane {
    /// Cycle focus to the next pane (tab/right key).
    pub fn next(self) -> Self {
        match self {
            Self::Sidebar => Self::MessageList,
            Self::MessageList => Self::Reader,
            Self::Reader => Self::Sidebar,
        }
    }

    /// Cycle focus to the previous pane (shift-tab/left key).
    pub fn previous(self) -> Self {
        match self {
            Self::Sidebar => Self::Reader,
            Self::MessageList => Self::Sidebar,
            Self::Reader => Self::MessageList,
        }
    }
}

/// Layout engine computing 3-pane responsive terminal view bounds.
pub struct AppLayout;

impl AppLayout {
    /// Compute 3-pane horizontal split: (sidebar, message_list, reader).
    pub fn compute_layout(area: Rect) -> (Rect, Rect, Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(35),
                Constraint::Percentage(45),
            ])
            .split(area);

        (chunks[0], chunks[1], chunks[2])
    }

    /// Compute block border style for active vs inactive panes.
    pub fn border_style(pane: ActivePane, current: ActivePane) -> Style {
        if pane == current {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_pane_focus_cycling() {
        let pane = ActivePane::Sidebar;
        assert_eq!(pane.next(), ActivePane::MessageList);
        assert_eq!(pane.next().next(), ActivePane::Reader);
        assert_eq!(pane.next().next().next(), ActivePane::Sidebar);

        assert_eq!(pane.previous(), ActivePane::Reader);
        assert_eq!(pane.previous().previous(), ActivePane::MessageList);
    }

    #[test]
    fn compute_layout_splits_area_three_ways() {
        let area = Rect::new(0, 0, 100, 40);
        let (sidebar, list, reader) = AppLayout::compute_layout(area);

        assert_eq!(sidebar.width, 20);
        assert_eq!(list.width, 35);
        assert_eq!(reader.width, 45);
    }

    #[test]
    fn border_style_highlights_focused_pane() {
        let focused = AppLayout::border_style(ActivePane::Sidebar, ActivePane::Sidebar);
        let unfocused = AppLayout::border_style(ActivePane::Sidebar, ActivePane::Reader);

        assert_eq!(focused.fg, Some(Color::Yellow));
        assert_eq!(unfocused.fg, Some(Color::DarkGray));
    }
}

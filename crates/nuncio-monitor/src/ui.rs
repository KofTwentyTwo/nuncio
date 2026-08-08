//! The rendered panes: status header, accounts table, and log table.
//!
//! This module is a pure renderer of [`AppState`] -- it mutates only the
//! trivial view-local fields a widget owns by construction (the log level
//! filter, the search text, and the clicked `request_id` filter), and it
//! never decides what a value MEANS (that belongs to `state.rs`'s
//! `AppState::account_rows`) or how to format a value that can be unknown
//! (that belongs to `fmt.rs`). `egui` is impractical to assert against in a
//! unit test, so nothing here has branching worth testing on its own; the
//! logic that does need tests -- the account-row join and every
//! unknown-vs-zero formatting decision -- lives in `state.rs`/`fmt.rs` and
//! is exercised there.

use egui_extras::{Column, TableBuilder};

use crate::fmt;
use crate::log_tail::{LogAvailability, LogRecord};
use crate::state::AppState;

/// Row height used by both tables.
const ROW_HEIGHT: f32 = 20.0;

/// Draws every pane into `ui`, which is the app's root [`egui::Ui`] for this
/// frame (see `eframe::App::ui`'s doc comment: it already IS the content
/// area, with no panel wrapping applied yet).
pub fn draw(ui: &mut egui::Ui, state: &mut AppState) {
    egui::Panel::top("status_header").show(ui, |ui| {
        render_status_header(ui, state);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        ui.heading("Accounts");
        render_accounts_table(ui, state);
        ui.separator();
        ui.heading("Logs");
        render_log_availability_banner(ui, state.log_availability);
        render_log_controls(ui, state);
        render_log_table(ui, state);
    });
}

fn render_status_header(ui: &mut egui::Ui, state: &AppState) {
    ui.horizontal(|ui| {
        let [r, g, b, a] = crate::tray::rgba_for_state(state.engine);
        ui.label(
            egui::RichText::new(format!("{:?}", state.engine))
                .color(egui::Color32::from_rgba_unmultiplied(r, g, b, a))
                .strong(),
        );
        ui.separator();

        match &state.status {
            Some(status) => {
                ui.label(format!("v{}", status.version));
                let uptime_secs = status
                    .uptime
                    .as_ref()
                    .map(nuncio_proto::time::duration_to_secs);
                ui.label(format!("uptime: {}", fmt::uptime_or_unknown(uptime_secs)));
                ui.label(format!("ready: {}", status.ready));
                ui.label(format!("accounts: {}", status.accounts_loaded));
                ui.label(format!("unread: {}", status.unread_count));
                ui.label(format!("outbox: {}", status.outbox_depth));
                if let Some(err) = &status.last_error {
                    ui.colored_label(egui::Color32::RED, format!("last error: {err}"));
                }
            }
            None => {
                ui.label(format!("version: {}", fmt::UNKNOWN));
                ui.label(format!("uptime: {}", fmt::UNKNOWN));
                ui.label(format!("ready: {}", fmt::UNKNOWN));
                ui.label(format!("accounts: {}", fmt::UNKNOWN));
                ui.label(format!("unread: {}", fmt::UNKNOWN));
                ui.label(format!("outbox: {}", fmt::UNKNOWN));
            }
        }

        ui.separator();
        if state.stream_stale {
            ui.colored_label(egui::Color32::RED, "STREAM STALE");
        } else {
            ui.colored_label(egui::Color32::from_rgb(40, 180, 80), "STREAM LIVE");
        }
    });

    // The monitor's OWN last error -- background-thread startup, tray
    // creation/update, or a Start/Stop Engine command -- distinct from
    // `status.last_error` above, which is the DAEMON's own last error.
    // `tracing::error!`/`tracing::warn!` alone never reaches this window
    // (there is no visible console), so this is the only place the user
    // ever sees these failures.
    if let Some(err) = &state.last_error {
        ui.colored_label(egui::Color32::RED, format!("monitor error: {err}"));
    }
}

fn render_accounts_table(ui: &mut egui::Ui, state: &AppState) {
    let rows = state.account_rows();

    TableBuilder::new(ui)
        .id_salt("accounts-table")
        .striped(true)
        .resizable(true)
        .column(Column::auto().at_least(140.0))
        .column(Column::auto().at_least(80.0))
        .column(Column::auto().at_least(120.0))
        .column(Column::remainder().at_least(150.0))
        .column(Column::auto().at_least(70.0))
        .column(Column::auto().at_least(70.0))
        .header(ROW_HEIGHT, |mut header| {
            header.col(|ui| {
                ui.strong("Account");
            });
            header.col(|ui| {
                ui.strong("Sync State");
            });
            header.col(|ui| {
                ui.strong("Last Sync");
            });
            header.col(|ui| {
                ui.strong("Last Error");
            });
            header.col(|ui| {
                ui.strong("Pending");
            });
            header.col(|ui| {
                ui.strong("Failed");
            });
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, rows.len(), |mut row_ui| {
                let row = &rows[row_ui.index()];
                row_ui.col(|ui| {
                    ui.label(&row.account.name);
                });
                row_ui.col(|ui| {
                    ui.label(fmt::sync_state_label(row.sync_state));
                });
                row_ui.col(|ui| {
                    ui.label(fmt::last_synced_label(row.sync_state));
                });
                row_ui.col(|ui| {
                    ui.label(fmt::last_error_label(row.sync_state));
                });
                row_ui.col(|ui| {
                    ui.label(fmt::u64_or_unknown(row.pending));
                });
                row_ui.col(|ui| {
                    ui.label(fmt::u64_or_unknown(row.failed));
                });
            });
        });
}

/// Warns when the log pane cannot be trusted: no log directory yet, or the
/// daemon is writing plain-text (not JSON) logs, in which case filtering by
/// level/`request_id` cannot work at all. Silent when everything is fine.
fn render_log_availability_banner(ui: &mut egui::Ui, availability: LogAvailability) {
    match availability {
        LogAvailability::Ok => {}
        LogAvailability::DirectoryMissing => {
            ui.colored_label(
                egui::Color32::from_rgb(230, 200, 40),
                "No log directory found yet -- nunciod may not be running, or hasn't \
                 created its logs directory.",
            );
        }
        LogAvailability::NotJson => {
            ui.colored_label(
                egui::Color32::from_rgb(230, 200, 40),
                "nunciod is logging plain text, not JSON -- restart it with \
                 NUNCIO_LOG_FORMAT=json to enable level/request-id filtering.",
            );
        }
    }
}

fn render_log_controls(ui: &mut egui::Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.label("Level:");
        let mut selected = state.log_filter.level.clone();
        egui::ComboBox::from_id_salt("log-level-filter")
            .selected_text(selected.clone().unwrap_or_else(|| "All".to_string()))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selected, None, "All");
                for level in ["TRACE", "DEBUG", "INFO", "WARN", "ERROR", "RAW"] {
                    ui.selectable_value(&mut selected, Some(level.to_string()), level);
                }
            });
        state.log_filter.level = selected;

        ui.label("Search:");
        let mut text = state.log_filter.text.clone().unwrap_or_default();
        if ui.text_edit_singleline(&mut text).changed() {
            state.log_filter.text = if text.is_empty() { None } else { Some(text) };
        }

        if let Some(request_id) = state.log_filter.request_id.clone() {
            ui.label(format!("request_id = {request_id}"));
            if ui.button("Clear filter").clicked() {
                state.log_filter.request_id = None;
            }
        }
    });
}

fn render_log_table(ui: &mut egui::Ui, state: &mut AppState) {
    // Borrowed, never cloned: `visible_logs()` already returns
    // `Vec<&LogRecord>` for exactly this reason -- at the `MAX_LOG_LINES`
    // cap with a repaint every `IDLE_REPAINT_INTERVAL` (plus every
    // input-driven one), cloning all 5000 six-`String` records here would
    // mean thousands of heap allocations per frame for no reason. The
    // click handler below assigns into a local (`clicked_request_id`), not
    // into `state`, so this borrow of `state` can end (at `logs`'s last
    // use, inside the table closures) before `state.log_filter` is
    // mutated afterward.
    let logs: Vec<&LogRecord> = state.visible_logs();
    let mut clicked_request_id: Option<String> = None;

    TableBuilder::new(ui)
        .id_salt("logs-table")
        .striped(true)
        .resizable(true)
        .column(Column::auto().at_least(160.0))
        .column(Column::auto().at_least(60.0))
        .column(Column::auto().at_least(120.0))
        .column(Column::auto().at_least(100.0))
        .column(Column::remainder().at_least(200.0))
        .header(ROW_HEIGHT, |mut header| {
            header.col(|ui| {
                ui.strong("Timestamp");
            });
            header.col(|ui| {
                ui.strong("Level");
            });
            header.col(|ui| {
                ui.strong("Target");
            });
            header.col(|ui| {
                ui.strong("Request Id");
            });
            header.col(|ui| {
                ui.strong("Message");
            });
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, logs.len(), |mut row_ui| {
                let record: &LogRecord = logs[row_ui.index()];
                row_ui.col(|ui| {
                    ui.label(&record.timestamp);
                });
                row_ui.col(|ui| {
                    ui.label(&record.level);
                });
                row_ui.col(|ui| {
                    ui.label(&record.target);
                });
                row_ui.col(|ui| match &record.request_id {
                    Some(id) => {
                        if ui.link(id).clicked() {
                            clicked_request_id = Some(id.clone());
                        }
                    }
                    None => {
                        ui.label(fmt::UNKNOWN);
                    }
                });
                row_ui.col(|ui| {
                    let text = if record.level == "RAW" {
                        record.raw.as_str()
                    } else {
                        record.message.as_str()
                    };
                    ui.label(text);
                });
            });
        });

    if let Some(id) = clicked_request_id {
        state.log_filter.request_id = Some(id);
    }
}

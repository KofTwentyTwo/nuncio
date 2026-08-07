//! Pure formatting helpers for rendering daemon-reported values.
//!
//! This is where the "unknown must never look like zero" rule from the UI
//! spec actually lives: every numeric cell that can legitimately be
//! `None` -- because the poll cycle that would have supplied it failed --
//! renders as [`UNKNOWN`], never as `"0"`. `ui.rs` calls these rather than
//! formatting values inline so this rule is exercised by ordinary unit
//! tests instead of only by manually watching the window.

use nuncio_proto::v1::{AccountSyncState, SyncState};

/// Placeholder for any cell whose source value is unknown this poll cycle.
/// Deliberately distinct from `"0"`, which [`u64_or_unknown`] renders for a
/// genuine, known zero.
pub const UNKNOWN: &str = "—";

/// Renders an optional count. `None` means the poll cycle that would have
/// supplied it failed; `Some(0)` is a real, known zero.
#[must_use]
pub fn u64_or_unknown(value: Option<u64>) -> String {
    match value {
        Some(v) => v.to_string(),
        None => UNKNOWN.to_string(),
    }
}

/// Renders whole-second uptime, or [`UNKNOWN`] if the whole `GetStatus`
/// cycle that would supply it failed.
#[must_use]
pub fn uptime_or_unknown(uptime_secs: Option<u64>) -> String {
    match uptime_secs {
        Some(secs) => format_duration_secs(secs),
        None => UNKNOWN.to_string(),
    }
}

/// Renders a whole-second duration as `HhMMmSSs`, dropping leading
/// zero-valued units (e.g. `90` -> `"1m30s"`, `30` -> `"30s"`).
#[must_use]
pub fn format_duration_secs(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m{secs:02}s")
    } else if minutes > 0 {
        format!("{minutes}m{secs:02}s")
    } else {
        format!("{secs}s")
    }
}

/// Renders one account's live sync state, or [`UNKNOWN`] if `sync_state` is
/// `None` -- i.e. the whole `GetStatus` poll cycle that would carry it
/// failed. Never fabricates `"Idle"` for a daemon that simply hasn't been
/// heard from this cycle.
#[must_use]
pub fn sync_state_label(sync_state: Option<&AccountSyncState>) -> &'static str {
    let Some(sync_state) = sync_state else {
        return UNKNOWN;
    };
    match SyncState::try_from(sync_state.state).unwrap_or(SyncState::Unspecified) {
        SyncState::Unspecified | SyncState::Idle => "Idle",
        SyncState::Syncing => "Syncing",
        SyncState::Error => "Error",
    }
}

/// Renders an account's last-successful-sync timestamp as raw unix seconds,
/// `"never"` if the account has not completed a sync since daemon start, or
/// [`UNKNOWN`] if `sync_state` itself is `None` (whole poll cycle failed).
#[must_use]
pub fn last_synced_label(sync_state: Option<&AccountSyncState>) -> String {
    let Some(sync_state) = sync_state else {
        return UNKNOWN.to_string();
    };
    match &sync_state.last_synced {
        Some(ts) => nuncio_proto::time::timestamp_to_unix_secs(ts).to_string(),
        None => "never".to_string(),
    }
}

/// Renders an account's last sync error, empty if there was none, or
/// [`UNKNOWN`] if `sync_state` itself is `None` (whole poll cycle failed).
#[must_use]
pub fn last_error_label(sync_state: Option<&AccountSyncState>) -> String {
    let Some(sync_state) = sync_state else {
        return UNKNOWN.to_string();
    };
    sync_state.last_error.clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_proto::time::timestamp_from_unix_secs;

    #[test]
    fn a_missing_count_renders_the_unknown_placeholder_not_zero() {
        assert_eq!(u64_or_unknown(None), UNKNOWN);
    }

    #[test]
    fn a_genuine_zero_count_renders_as_zero() {
        assert_eq!(u64_or_unknown(Some(0)), "0");
    }

    #[test]
    fn a_present_count_renders_verbatim() {
        assert_eq!(u64_or_unknown(Some(42)), "42");
    }

    #[test]
    fn missing_uptime_renders_unknown() {
        assert_eq!(uptime_or_unknown(None), UNKNOWN);
    }

    #[test]
    fn duration_formatting_drops_leading_zero_units() {
        assert_eq!(format_duration_secs(5), "5s");
        assert_eq!(format_duration_secs(65), "1m05s");
        assert_eq!(format_duration_secs(3665), "1h01m05s");
    }

    #[test]
    fn sync_state_label_is_unknown_when_the_whole_poll_cycle_failed() {
        assert_eq!(sync_state_label(None), UNKNOWN);
    }

    #[test]
    fn sync_state_label_maps_every_known_variant() {
        let make = |state: i32| AccountSyncState {
            account_id: "a".to_string(),
            state,
            last_synced: None,
            last_error: None,
        };
        assert_eq!(sync_state_label(Some(&make(1))), "Idle");
        assert_eq!(sync_state_label(Some(&make(2))), "Syncing");
        assert_eq!(sync_state_label(Some(&make(3))), "Error");
        // An out-of-range value must not panic -- it degrades to Idle via
        // `SyncState::Unspecified`, never crashing the render pass.
        assert_eq!(sync_state_label(Some(&make(99))), "Idle");
    }

    #[test]
    fn last_synced_label_distinguishes_unknown_never_and_a_real_timestamp() {
        assert_eq!(last_synced_label(None), UNKNOWN);

        let never = AccountSyncState {
            account_id: "a".to_string(),
            state: 1,
            last_synced: None,
            last_error: None,
        };
        assert_eq!(last_synced_label(Some(&never)), "never");

        let synced = AccountSyncState {
            account_id: "a".to_string(),
            state: 1,
            last_synced: Some(timestamp_from_unix_secs(1_700_000_000)),
            last_error: None,
        };
        assert_eq!(last_synced_label(Some(&synced)), "1700000000");
    }

    #[test]
    fn last_error_label_distinguishes_unknown_from_no_error() {
        assert_eq!(last_error_label(None), UNKNOWN);

        let no_error = AccountSyncState {
            account_id: "a".to_string(),
            state: 1,
            last_synced: None,
            last_error: None,
        };
        assert_eq!(last_error_label(Some(&no_error)), "");

        let errored = AccountSyncState {
            account_id: "a".to_string(),
            state: 3,
            last_synced: None,
            last_error: Some("boom".to_string()),
        };
        assert_eq!(last_error_label(Some(&errored)), "boom");
    }
}

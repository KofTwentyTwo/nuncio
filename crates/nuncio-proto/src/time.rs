//! Conversions between the store's native integer instants/durations and the
//! well-known `google.protobuf.Timestamp`/`google.protobuf.Duration` wire
//! types used at the `nuncio.v1` proto boundary.
//!
//! The daemon's core/store models keep unix-seconds `i64`/`u64` instants and
//! plain scalar durations; these helpers are the single place that maps them
//! to/from the well-known types so every RPC mapper does it the same way.
//! Re-exporting `Timestamp`/`Duration` here lets callers name the wire types
//! without depending on `prost-types` directly.

pub use prost_types::{Duration, Timestamp};

/// Converts a unix-seconds instant into a `Timestamp` with a zero `nanos`
/// component. Use this for store fields that only ever carry second
/// granularity (e.g. message/event/rule timestamps).
#[must_use]
pub fn timestamp_from_unix_secs(unix_secs: i64) -> Timestamp {
    Timestamp {
        seconds: unix_secs,
        nanos: 0,
    }
}

/// Extracts the unix-seconds instant from a `Timestamp`, truncating any
/// sub-second `nanos` component. Use this for fields mapped via
/// [`timestamp_from_unix_secs`], where `nanos` is always zero.
#[must_use]
pub fn timestamp_to_unix_secs(ts: &Timestamp) -> i64 {
    ts.seconds
}

/// Converts a unix-nanoseconds instant into a `Timestamp`, splitting it
/// losslessly into whole seconds plus a sub-second `nanos` remainder. Use
/// this for fields that carry sub-second precision (e.g. the WORM audit
/// ledger's `timestamp_ns`).
#[must_use]
pub fn timestamp_from_unix_nanos(unix_nanos: i64) -> Timestamp {
    let seconds = unix_nanos.div_euclid(1_000_000_000);
    let nanos = unix_nanos.rem_euclid(1_000_000_000);
    Timestamp {
        seconds,
        nanos: nanos as i32,
    }
}

/// Recovers the exact unix-nanoseconds instant a `Timestamp` was built from
/// via [`timestamp_from_unix_nanos`].
#[must_use]
pub fn timestamp_to_unix_nanos(ts: &Timestamp) -> i64 {
    ts.seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(i64::from(ts.nanos))
}

/// Converts a whole-second duration into a `Duration` with a zero `nanos`
/// component (e.g. `AccountConfig`'s sync polling interval).
#[must_use]
pub fn duration_from_secs(secs: u64) -> Duration {
    Duration {
        seconds: secs as i64,
        nanos: 0,
    }
}

/// Converts a `std::time::Duration` (e.g. a process-uptime measurement taken
/// with `Instant::elapsed`) into a wire `Duration`, preserving sub-second
/// precision. Saturates rather than overflowing on an implausibly large span.
#[must_use]
pub fn duration_from_std(d: std::time::Duration) -> Duration {
    Duration {
        seconds: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        nanos: d.subsec_nanos() as i32,
    }
}

/// Extracts a whole-second duration from a `Duration`, truncating any
/// sub-second `nanos` component. A negative `seconds` (never produced by
/// this crate's own mappers) reads back as zero rather than panicking.
#[must_use]
pub fn duration_to_secs(d: &Duration) -> u64 {
    u64::try_from(d.seconds).unwrap_or(0)
}

/// Converts a microsecond duration into a `Duration`, splitting it
/// losslessly into whole seconds plus a sub-second `nanos` remainder (e.g.
/// filter rule preview evaluation timing).
#[must_use]
pub fn duration_from_micros(micros: u64) -> Duration {
    let seconds = micros / 1_000_000;
    let nanos = (micros % 1_000_000) * 1_000;
    Duration {
        seconds: seconds as i64,
        nanos: nanos as i32,
    }
}

/// Recovers the exact microsecond duration a `Duration` was built from via
/// [`duration_from_micros`].
#[must_use]
pub fn duration_to_micros(d: &Duration) -> u64 {
    let seconds = u64::try_from(d.seconds).unwrap_or(0);
    let nanos = u64::try_from(d.nanos).unwrap_or(0);
    seconds
        .saturating_mul(1_000_000)
        .saturating_add(nanos / 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_round_trip_through_timestamp() {
        let ts = timestamp_from_unix_secs(1_700_000_000);
        assert_eq!(ts.nanos, 0);
        assert_eq!(timestamp_to_unix_secs(&ts), 1_700_000_000);
    }

    #[test]
    fn nanos_round_trip_through_timestamp() {
        let original_ns = 1_700_000_000_123_456_789_i64;
        let ts = timestamp_from_unix_nanos(original_ns);
        assert_eq!(timestamp_to_unix_nanos(&ts), original_ns);
    }

    #[test]
    fn negative_epoch_nanos_round_trip() {
        // Regression: pre-epoch instants must still split into a
        // non-negative `nanos` remainder per the Timestamp wire contract.
        let original_ns = -1_500_000_000_i64;
        let ts = timestamp_from_unix_nanos(original_ns);
        assert!(ts.nanos >= 0);
        assert_eq!(timestamp_to_unix_nanos(&ts), original_ns);
    }

    #[test]
    fn secs_round_trip_through_duration() {
        let d = duration_from_secs(300);
        assert_eq!(d.nanos, 0);
        assert_eq!(duration_to_secs(&d), 300);
    }

    #[test]
    fn micros_round_trip_through_duration() {
        let original_us = 1_234_567_u64;
        let d = duration_from_micros(original_us);
        assert_eq!(duration_to_micros(&d), original_us);
    }
}

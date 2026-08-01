//! Secret-safe value wrapper and the logging redaction policy it enforces.
//!
//! # Logging redaction policy
//!
//! Telemetry (`tracing` spans, events, and any `Debug`/`Display`/`serde`
//! rendering that can reach a log sink) is a plaintext side channel. The
//! following values MUST NEVER appear in it, in any encoding:
//!
//! - message bodies, subjects, and snippets;
//! - sender/recipient email addresses and any other message correspondents;
//! - account credentials (passwords, app-specific passwords/tokens);
//! - gRPC bearer tokens;
//! - keyring secret *values* (the material stored in the OS vault) and any
//!   derived key material (encryption keys, HMAC signing keys);
//! - vCard / iCal personal data (contact names, phones, addresses; event
//!   summaries, locations, attendees, descriptions).
//!
//! The following identifying, non-sensitive values ARE allowed in telemetry,
//! because operating the daemon is impossible without them and none of them
//! reveals content or grants access:
//!
//! - opaque ids (account id, message id, rule id, folder id) and UIDs;
//! - counts, sizes, and durations;
//! - operation / rule / transport / action *kinds* (the discriminant, never
//!   its payload);
//! - folder names, hostnames, ports, and the per-request `request_id`;
//! - the keyring *lookup key* (e.g. `nuncio/<account-id>`) — an identifier
//!   that names a vault entry but is not itself the stored secret.
//!
//! [`Redacted<T>`] is the mechanical enforcement point for the first list: a
//! field whose type is `Redacted<T>` cannot leak through a derived `Debug`, a
//! `Display`, or `serde` serialization, because each of those renders a fixed
//! placeholder instead of the inner value. The secret is reachable only via an
//! explicit [`Redacted::expose_secret`] / [`Redacted::into_inner`] call, which
//! is easy to audit and never happens implicitly.

use std::fmt;

use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};

/// The single placeholder rendered wherever a redacted value would otherwise
/// appear (`Debug`, `Display`, and `serde`).
const PLACEHOLDER: &str = "<redacted>";

/// A wrapper for a sensitive value that never renders its contents through
/// `Debug`, `Display`, or `serde`.
///
/// Wrapping a credential/token/key-material field in `Redacted<T>` makes it
/// impossible to leak that field into telemetry by accident: the compiler still
/// lets a struct derive `Debug`/`Serialize`, but the wrapped field renders as
/// `<redacted>`. Reaching the real value is deliberate and greppable — it
/// requires calling [`expose_secret`](Self::expose_secret) or
/// [`into_inner`](Self::into_inner).
///
/// Equality is derived over the inner value so wrapped fields remain comparable
/// in tests and config diffs; it is intentionally not constant-time and is not
/// meant for authenticating a secret against an attacker-supplied guess.
#[derive(Clone, PartialEq, Eq)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Wrap `value` so it is protected from accidental disclosure in telemetry.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Borrow the wrapped secret. Naming the call `expose_secret` keeps every
    /// deliberate disclosure of the value greppable and reviewable.
    pub fn expose_secret(&self) -> &T {
        &self.0
    }

    /// Consume the wrapper and return the owned secret.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for Redacted<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

/// Renders `<redacted>` regardless of the inner value, so a derived `Debug` on
/// any containing struct can never print the secret.
impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PLACEHOLDER)
    }
}

/// Renders `<redacted>`, so interpolating the value with `{}` (e.g. into a
/// log message or error) discloses nothing.
impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PLACEHOLDER)
    }
}

/// Serializes the placeholder, never the inner value. This closes the
/// `serde_json`/structured-logging leak path: even serializing a whole struct
/// that contains a `Redacted<T>` field emits `<redacted>` for it. As a result
/// serialization is deliberately lossy — a `Redacted<T>` must never be used for
/// a field whose real value has to survive a serialization round trip.
impl<T> Serialize for Redacted<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(PLACEHOLDER)
    }
}

/// Deserializes transparently into `Redacted<T>` by deserializing the inner
/// `T`. This exists so structs that hold a `Redacted<T>` can still derive
/// `Deserialize`; it does not reverse [`Serialize`], which is lossy by design.
impl<'de, T> Deserialize<'de> for Redacted<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self(T::deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: &str = "super-secret-app-token-9f3a";

    #[test]
    fn debug_renders_placeholder_not_the_secret() {
        let wrapped = Redacted::new(SENTINEL.to_string());
        let rendered = format!("{wrapped:?}");
        assert_eq!(rendered, PLACEHOLDER);
        assert!(!rendered.contains(SENTINEL));
    }

    #[test]
    fn display_renders_placeholder_not_the_secret() {
        let wrapped = Redacted::new(SENTINEL.to_string());
        let rendered = format!("{wrapped}");
        assert_eq!(rendered, PLACEHOLDER);
        assert!(!rendered.contains(SENTINEL));
    }

    #[test]
    fn derived_debug_of_a_containing_struct_does_not_leak() {
        // Fields are exercised only through the derived `Debug`, which
        // dead-code analysis deliberately ignores; the test asserts on that
        // rendering, so silence the false-positive unread-field lint.
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Config {
            username: String,
            auth_token: Redacted<String>,
        }
        let cfg = Config {
            username: "jmaes".to_string(),
            auth_token: Redacted::new(SENTINEL.to_string()),
        };
        let rendered = format!("{cfg:?}");
        assert!(rendered.contains("jmaes"));
        assert!(!rendered.contains(SENTINEL));
        assert!(rendered.contains(PLACEHOLDER));
    }

    #[test]
    fn serialize_emits_placeholder_not_the_secret() {
        let wrapped = Redacted::new(SENTINEL.to_string());
        let json = serde_json::to_string(&wrapped).expect("serialize");
        assert!(!json.contains(SENTINEL));
        assert_eq!(json, format!("\"{PLACEHOLDER}\""));
    }

    #[test]
    fn expose_and_into_inner_return_the_real_value() {
        let wrapped = Redacted::new(SENTINEL.to_string());
        assert_eq!(wrapped.expose_secret(), SENTINEL);
        assert_eq!(wrapped.into_inner(), SENTINEL);
    }

    #[test]
    fn deserialize_wraps_the_inner_value() {
        let wrapped: Redacted<String> =
            serde_json::from_str(&format!("\"{SENTINEL}\"")).expect("deserialize");
        assert_eq!(wrapped.expose_secret(), SENTINEL);
    }

    #[test]
    fn from_and_equality() {
        let a: Redacted<String> = SENTINEL.to_string().into();
        let b = Redacted::new(SENTINEL.to_string());
        assert_eq!(a, b);
        assert_ne!(a, Redacted::new("other".to_string()));
    }
}

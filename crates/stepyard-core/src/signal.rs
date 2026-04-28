//! Typed signal name carried through `Event::SignalReceived` and
//! `TerminationReason::SignalReceived` (#41).
//!
//! The wire format is the lowercase snake_case string the pre-typed era
//! emitted byte-for-byte — `{"signal":"sigterm"}`, never an externally
//! tagged enum or an object. Existing session logs round-trip through
//! the new type without rewrite, and producers (`src/signal.rs`,
//! `stepyard_harness::startup`) keep emitting the same bytes.
//!
//! `Other(String)` exists for forward-compat. A future signal name like
//! `"sigquit"` deserializes into `Other("sigquit".into())`, preserves
//! its original spelling through `Display`, and re-serializes to the
//! same byte sequence — so a replay → emit cycle never silently
//! collapses unknown values into "unknown". `serde`'s `#[serde(other)]`
//! attribute only works on a unit variant (no payload), which is why
//! this module hand-writes `Serialize` + `Deserialize`.

use std::fmt;
use std::str::FromStr;

/// Lowercase snake_case signal name. Variants cover every value the
/// engine itself produces today; [`Signal::Other`] holds any other
/// string the deserializer encounters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Signal {
    /// `SIGINT` — Ctrl-C / interactive interrupt.
    Sigint,
    /// `SIGTERM` — orchestrator-issued graceful termination.
    Sigterm,
    /// Synthetic signal emitted by the startup reconciler (Story 2.4)
    /// when a session was found `running` but its container is gone.
    CrashRecovery,
    /// Forward-compat. Holds the original snake_case bytes verbatim so
    /// `Display` and re-serialization stay faithful to the source log.
    Other(String),
}

impl Signal {
    /// Wire / display string. `&str` lets [`Display`] and
    /// [`Serialize`] share a single zero-copy projection.
    fn as_wire(&self) -> &str {
        match self {
            Signal::Sigint => "sigint",
            Signal::Sigterm => "sigterm",
            Signal::CrashRecovery => "crash_recovery",
            Signal::Other(s) => s.as_str(),
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_wire())
    }
}

impl From<&str> for Signal {
    fn from(s: &str) -> Self {
        match s {
            "sigint" => Signal::Sigint,
            "sigterm" => Signal::Sigterm,
            "crash_recovery" => Signal::CrashRecovery,
            other => Signal::Other(other.to_string()),
        }
    }
}

impl From<String> for Signal {
    fn from(s: String) -> Self {
        // Match on `&str` so the known-variant branches don't drag
        // their incoming `String` along — only `Other(_)` keeps the
        // original allocation.
        match s.as_str() {
            "sigint" => Signal::Sigint,
            "sigterm" => Signal::Sigterm,
            "crash_recovery" => Signal::CrashRecovery,
            _ => Signal::Other(s),
        }
    }
}

impl FromStr for Signal {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Signal::from(s))
    }
}

impl serde::Serialize for Signal {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_wire())
    }
}

impl<'de> serde::Deserialize<'de> for Signal {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        Ok(Signal::from(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_wire_format_for_every_variant() {
        assert_eq!(Signal::Sigint.to_string(), "sigint");
        assert_eq!(Signal::Sigterm.to_string(), "sigterm");
        assert_eq!(Signal::CrashRecovery.to_string(), "crash_recovery");
        assert_eq!(Signal::Other("sigquit".into()).to_string(), "sigquit");
    }

    #[test]
    fn serializes_as_bare_string_not_tagged_enum() {
        // The pre-typed era stored `Event.signal` as a JSON string. New
        // code MUST emit the same bytes — no `{"type":"sigterm"}`
        // wrapper, no PascalCase. Replay tests in stepyard-harness
        // assert against these literals.
        assert_eq!(
            serde_json::to_string(&Signal::Sigint).unwrap(),
            "\"sigint\""
        );
        assert_eq!(
            serde_json::to_string(&Signal::Sigterm).unwrap(),
            "\"sigterm\""
        );
        assert_eq!(
            serde_json::to_string(&Signal::CrashRecovery).unwrap(),
            "\"crash_recovery\""
        );
        assert_eq!(
            serde_json::to_string(&Signal::Other("sigquit".into())).unwrap(),
            "\"sigquit\""
        );
    }

    #[test]
    fn deserializes_known_strings_into_named_variants() {
        assert_eq!(
            serde_json::from_str::<Signal>("\"sigint\"").unwrap(),
            Signal::Sigint
        );
        assert_eq!(
            serde_json::from_str::<Signal>("\"sigterm\"").unwrap(),
            Signal::Sigterm
        );
        assert_eq!(
            serde_json::from_str::<Signal>("\"crash_recovery\"").unwrap(),
            Signal::CrashRecovery
        );
    }

    #[test]
    fn deserializes_unknown_string_into_other_preserving_payload() {
        assert_eq!(
            serde_json::from_str::<Signal>("\"sigquit\"").unwrap(),
            Signal::Other("sigquit".into())
        );
    }

    #[test]
    fn round_trip_emits_input_bytes_verbatim_for_unknown_values() {
        // The forward-compat invariant: a replay → emit cycle through an
        // unknown signal name produces an identical JSON string. If this
        // test ever flips, an old log entry would silently mutate on
        // re-emit.
        let raw = "\"sigquit\"";
        let value: Signal = serde_json::from_str(raw).unwrap();
        let re_emitted = serde_json::to_string(&value).unwrap();
        assert_eq!(re_emitted, raw);
    }

    #[test]
    fn from_str_and_from_string_agree_on_known_variants() {
        for name in ["sigint", "sigterm", "crash_recovery", "sigquit"] {
            assert_eq!(Signal::from(name), Signal::from(name.to_string()));
            assert_eq!(Signal::from(name), Signal::from_str(name).unwrap());
        }
    }
}

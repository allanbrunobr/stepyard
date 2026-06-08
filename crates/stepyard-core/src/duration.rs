//! Typed workflow duration parsing — Round 3 rule-text enforcement.
//!
//! Round 3 of the sandcastle-features plan locks a narrow grammar for any
//! workflow YAML field that accepts a duration (today: `Step.timeout`):
//!
//! ```text
//! duration := segment+
//! segment  := integer unit
//! unit     := "ms" | "s" | "m" | "h"
//! ```
//!
//! Segments must appear strictly in order `h`, `m`, `s`, `ms`; each unit
//! appears at most once; no whitespace, no decimals, no signs, no uppercase,
//! no empty input. Bare integers (`timeout: 30`) are rejected at deserialize
//! time — the engine never silently reinterprets a number as milliseconds.
//!
//! Non-canonical but grammatical inputs (e.g. `90s`, `60000ms`) parse
//! successfully; the canonical serialization is a greedy high-to-low
//! decomposition emitting only non-zero segments, so round-tripping
//! `timeout: 90s` through serde produces `timeout: 1m30s`.
//! [`Duration::ZERO`] canonicalizes as `0s`.
//!
//! # Public surface
//!
//! The module exposes exactly four items (narrow audit surface — Story 5
//! will pin this with a CI grep):
//!
//! * [`parse_duration`] — `&str` → [`Duration`] (infallible reverse via
//!   [`serialize_optional`] is lossy for non-canonical inputs).
//! * [`serialize_optional`] — serde hook emitting canonical string form.
//! * [`deserialize_optional`] — serde hook rejecting non-string YAML
//!   scalars (integers / floats / booleans) with a typed error the
//!   workflow-load boundary maps into
//!   [`crate::EngineError::InvalidWorkflowField`].
//! * [`DurationParseError`] — parse error taxonomy.
//!
//! # Usage
//!
//! ```ignore
//! use std::time::Duration;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct Step {
//!     #[serde(
//!         default,
//!         serialize_with = "stepyard_core::duration::serialize_optional",
//!         deserialize_with = "stepyard_core::duration::deserialize_optional",
//!         skip_serializing_if = "Option::is_none"
//!     )]
//!     timeout: Option<Duration>,
//! }
//! ```

use std::fmt;
use std::time::Duration;

use serde::de::{self, Deserializer, Visitor};
use serde::Serializer;
use thiserror::Error;

/// The `expected:` phrase paired with any duration-field parse failure.
///
/// Exposed so the workflow-load boundary (stepyard-harness
/// `Workflow::try_from_yaml`, landed in commit 2) can populate
/// [`crate::EngineError::InvalidWorkflowField::expected`] with the same
/// canonical wording the deserializer advertised.
pub const EXPECTED: &str = "duration string (e.g. 30s, 500ms, 1h30m)";

/// Errors raised while parsing a workflow duration field.
#[non_exhaustive]
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DurationParseError {
    /// The input string was empty.
    #[error("duration must not be empty")]
    Empty,
    /// The input contained a byte outside the allowed set
    /// (ASCII digits + lowercase letters).
    #[error("duration `{got}` contains whitespace, punctuation, or uppercase characters")]
    InvalidCharacter {
        /// Raw input for operator-facing messages.
        got: String,
    },
    /// A segment began without any digits before its unit.
    #[error("duration `{got}` has no digits before unit")]
    MissingDigits { got: String },
    /// The input ended with digits but no unit.
    #[error("duration `{got}` ends without a unit")]
    MissingUnit { got: String },
    /// The unit was alphabetic but not one of `ms`, `s`, `m`, `h`.
    #[error("duration `{got}`: unknown unit `{unit}` (expected ms, s, m, or h)")]
    UnknownUnit { got: String, unit: String },
    /// A unit repeated or appeared out of the required high-to-low order.
    #[error(
        "duration `{got}`: unit `{unit}` repeats or is out of order (expected h > m > s > ms)"
    )]
    OrderViolation { got: String, unit: String },
    /// Total duration overflowed the representable u64 millisecond range.
    #[error("duration `{got}` overflows representable milliseconds")]
    Overflow { got: String },
}

/// Parse a workflow duration string into a [`Duration`].
///
/// See the module-level docs for the accepted grammar. Non-canonical
/// inputs (e.g. `90s`, `60000ms`) parse successfully; normalization
/// happens at serialization time, not here.
pub fn parse_duration(s: &str) -> Result<Duration, DurationParseError> {
    if s.is_empty() {
        return Err(DurationParseError::Empty);
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
    {
        return Err(DurationParseError::InvalidCharacter { got: s.into() });
    }

    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut total_ms: u128 = 0;
    let mut last_rank: i32 = i32::MAX;

    while i < bytes.len() {
        let digit_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == digit_start {
            return Err(DurationParseError::MissingDigits { got: s.into() });
        }
        let number: u128 = s[digit_start..i]
            .parse()
            .map_err(|_| DurationParseError::Overflow { got: s.into() })?;

        let unit_start = i;
        while i < bytes.len() && bytes[i].is_ascii_lowercase() {
            i += 1;
        }
        if i == unit_start {
            return Err(DurationParseError::MissingUnit { got: s.into() });
        }
        let unit = &s[unit_start..i];
        let (rank, mult_ms): (i32, u128) = match unit {
            "ms" => (1, 1),
            "s" => (2, 1_000),
            "m" => (3, 60_000),
            "h" => (4, 3_600_000),
            _ => {
                return Err(DurationParseError::UnknownUnit {
                    got: s.into(),
                    unit: unit.into(),
                });
            }
        };
        if rank >= last_rank {
            return Err(DurationParseError::OrderViolation {
                got: s.into(),
                unit: unit.into(),
            });
        }
        last_rank = rank;

        let product = number
            .checked_mul(mult_ms)
            .ok_or_else(|| DurationParseError::Overflow { got: s.into() })?;
        total_ms = total_ms
            .checked_add(product)
            .ok_or_else(|| DurationParseError::Overflow { got: s.into() })?;
    }

    let total_ms_u64: u64 = total_ms
        .try_into()
        .map_err(|_| DurationParseError::Overflow { got: s.into() })?;
    Ok(Duration::from_millis(total_ms_u64))
}

/// Canonical serialization: greedy high-to-low decomposition, only non-zero
/// segments emitted. [`Duration::ZERO`] serializes as `0s`.
fn canonical(d: Duration) -> String {
    let mut total_ms = d.as_millis();
    if total_ms == 0 {
        return "0s".to_string();
    }
    let hours = total_ms / 3_600_000;
    total_ms %= 3_600_000;
    let minutes = total_ms / 60_000;
    total_ms %= 60_000;
    let seconds = total_ms / 1_000;
    total_ms %= 1_000;
    let ms = total_ms;

    use std::fmt::Write;
    let mut out = String::new();
    if hours > 0 {
        write!(out, "{hours}h").unwrap();
    }
    if minutes > 0 {
        write!(out, "{minutes}m").unwrap();
    }
    if seconds > 0 {
        write!(out, "{seconds}s").unwrap();
    }
    if ms > 0 {
        write!(out, "{ms}ms").unwrap();
    }
    out
}

/// Serde hook: serialize `Option<Duration>` as the canonical string form
/// or as `null` for [`None`]. Pair with
/// `#[serde(skip_serializing_if = "Option::is_none")]` to keep `None`
/// entirely absent from the emitted document.
pub fn serialize_optional<S>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match d {
        // Must still honour `None` correctly even when callers forget
        // `skip_serializing_if` — serde will invoke this hook regardless.
        None => s.serialize_none(),
        Some(dur) => s.serialize_str(&canonical(*dur)),
    }
}

struct OptionalDurationVisitor;

impl<'de> Visitor<'de> for OptionalDurationVisitor {
    type Value = Option<Duration>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(EXPECTED)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(None)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StringOnlyVisitor).map(Some)
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        parse_duration(v).map(Some).map_err(E::custom)
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        parse_duration(&v).map(Some).map_err(E::custom)
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::invalid_type(de::Unexpected::Unsigned(v), &self))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::invalid_type(de::Unexpected::Signed(v), &self))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::invalid_type(de::Unexpected::Float(v), &self))
    }

    fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::invalid_type(de::Unexpected::Bool(v), &self))
    }
}

struct StringOnlyVisitor;

impl<'de> Visitor<'de> for StringOnlyVisitor {
    type Value = Duration;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(EXPECTED)
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        parse_duration(v).map_err(E::custom)
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        parse_duration(&v).map_err(E::custom)
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::invalid_type(de::Unexpected::Unsigned(v), &self))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::invalid_type(de::Unexpected::Signed(v), &self))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::invalid_type(de::Unexpected::Float(v), &self))
    }

    fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::invalid_type(de::Unexpected::Bool(v), &self))
    }
}

/// Serde hook: deserialize `Option<Duration>` from a YAML/JSON string,
/// rejecting integers, floats, and booleans with the canonical
/// [`EXPECTED`] phrasing. The workflow-load boundary
/// (`Workflow::try_from_yaml` — commit 2) wraps the resulting deserialize
/// error into [`crate::EngineError::InvalidWorkflowField`] by pairing
/// the field path from `serde_path_to_error` with [`EXPECTED`].
pub fn deserialize_optional<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_option(OptionalDurationVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[test]
    fn parse_accepts_grammar_fixtures() {
        let cases: &[(&str, u64)] = &[
            ("30s", 30_000),
            ("500ms", 500),
            ("10m", 600_000),
            ("2h", 7_200_000),
            ("1h30m", 5_400_000),
            ("2h15m30s", 8_130_000),
            ("1m500ms", 60_500),
            ("0s", 0),
            ("90s", 90_000),
            ("60000ms", 60_000),
        ];
        for (input, want_ms) in cases {
            let got = parse_duration(input).unwrap_or_else(|e| panic!("{input} failed: {e}"));
            assert_eq!(got, Duration::from_millis(*want_ms), "input={input}");
        }
    }

    #[test]
    fn parse_rejects_grammar_violations() {
        let cases: &[&str] = &[
            "30",         // no unit
            "30 s",       // whitespace
            "30 seconds", // unsupported spelling + whitespace
            "30seconds",  // unsupported spelling
            "1.5h",       // decimal
            "-5s",        // sign
            "1m30m",      // repeated unit
            "30s1m",      // out of order
            "",           // empty
            "ms",         // unit without integer
            "30S",        // uppercase
            "30M",        // uppercase
        ];
        for input in cases {
            let res = parse_duration(input);
            assert!(res.is_err(), "expected `{input}` to fail, got {res:?}");
        }
    }

    #[test]
    fn canonical_round_trip() {
        let cases: &[(&str, &str)] = &[
            ("30s", "30s"),
            ("500ms", "500ms"),
            ("1h30m", "1h30m"),
            ("2h15m30s", "2h15m30s"),
            ("1m500ms", "1m500ms"),
            ("0s", "0s"),
            ("90s", "1m30s"),  // normalization
            ("60000ms", "1m"), // normalization
            ("2h", "2h"),
            ("10m", "10m"),
        ];
        for (input, want) in cases {
            let parsed = parse_duration(input).expect(input);
            assert_eq!(canonical(parsed), *want, "canonical({input})");
            // Re-parse the canonical form and confirm it survives a
            // second round trip unchanged.
            let re_parsed = parse_duration(want).expect(want);
            assert_eq!(parsed, re_parsed, "round-trip of {want}");
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Holder {
        #[serde(
            default,
            serialize_with = "serialize_optional",
            deserialize_with = "deserialize_optional",
            skip_serializing_if = "Option::is_none"
        )]
        timeout: Option<Duration>,
    }

    #[test]
    fn deserialize_accepts_canonical_string() {
        let h: Holder = serde_json::from_str(r#"{"timeout": "1h30m"}"#).unwrap();
        assert_eq!(h.timeout, Some(Duration::from_millis(5_400_000)));
    }

    #[test]
    fn deserialize_accepts_non_canonical_string() {
        let h: Holder = serde_json::from_str(r#"{"timeout": "90s"}"#).unwrap();
        assert_eq!(h.timeout, Some(Duration::from_millis(90_000)));
    }

    #[test]
    fn deserialize_accepts_null_and_missing() {
        let h: Holder = serde_json::from_str(r#"{"timeout": null}"#).unwrap();
        assert_eq!(h.timeout, None);
        let h: Holder = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(h.timeout, None);
    }

    #[test]
    fn deserialize_rejects_bare_integer() {
        let err = serde_json::from_str::<Holder>(r#"{"timeout": 30}"#).unwrap_err();
        let msg = err.to_string();
        // Error surface is serde's "invalid type: integer `30`,
        // expected <EXPECTED>" — commit 2's boundary keys on this
        // shape to populate InvalidWorkflowField.
        assert!(
            msg.contains("30") && msg.contains("duration string"),
            "expected error to mention 30 and duration string, got: {msg}"
        );
    }

    #[test]
    fn deserialize_rejects_float_and_bool() {
        assert!(serde_json::from_str::<Holder>(r#"{"timeout": 3.5}"#).is_err());
        assert!(serde_json::from_str::<Holder>(r#"{"timeout": true}"#).is_err());
    }

    #[test]
    fn deserialize_rejects_ungrammatical_string() {
        let err = serde_json::from_str::<Holder>(r#"{"timeout": "30"}"#).unwrap_err();
        assert!(err.to_string().contains("ends without a unit"));
    }

    #[test]
    fn serialize_emits_canonical_form() {
        let h = Holder {
            timeout: Some(Duration::from_millis(90_000)),
        };
        let out = serde_json::to_string(&h).unwrap();
        assert_eq!(out, r#"{"timeout":"1m30s"}"#);
    }

    #[test]
    fn serialize_emits_zero_as_0s() {
        let h = Holder {
            timeout: Some(Duration::ZERO),
        };
        let out = serde_json::to_string(&h).unwrap();
        assert_eq!(out, r#"{"timeout":"0s"}"#);
    }

    #[test]
    fn serialize_none_is_skipped() {
        let h = Holder { timeout: None };
        let out = serde_json::to_string(&h).unwrap();
        assert_eq!(out, "{}");
    }

    #[test]
    fn serialize_none_without_skip_emits_null() {
        // serialize_optional(None) must be robust — serde may call it
        // despite `skip_serializing_if` on structurally different
        // callers. Confirm the `None` arm writes `null`, not a panic.
        #[derive(Serialize)]
        struct Bare {
            #[serde(serialize_with = "serialize_optional")]
            timeout: Option<Duration>,
        }
        let out = serde_json::to_string(&Bare { timeout: None }).unwrap();
        assert_eq!(out, r#"{"timeout":null}"#);
    }

    #[test]
    fn yaml_bare_integer_surfaces_typed_deserialize_error() {
        // Proxy for commit 2's AC6 boundary test: serde_yaml routes
        // `timeout: 30` to our visit_u64 which returns invalid_type
        // with the canonical expected phrase.
        let err = serde_yaml::from_str::<Holder>("timeout: 30\n").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("30") && msg.contains("duration string"),
            "YAML bare-int error should mention 30 and duration string; got: {msg}"
        );
    }

    #[test]
    fn yaml_string_duration_round_trips() {
        let h: Holder = serde_yaml::from_str("timeout: 1m500ms\n").unwrap();
        assert_eq!(h.timeout, Some(Duration::from_millis(60_500)));
        let out = serde_yaml::to_string(&h).unwrap();
        // serde_yaml may quote or not; assert the scalar value via
        // re-parse into Value.
        let v: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(v["timeout"].as_str(), Some("1m500ms"));
    }

    #[test]
    fn yaml_normalizes_non_canonical_input() {
        let h: Holder = serde_yaml::from_str("timeout: 90s\n").unwrap();
        let out = serde_yaml::to_string(&h).unwrap();
        let v: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(v["timeout"].as_str(), Some("1m30s"));
    }
}

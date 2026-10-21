//! Human-facing display-boundary sanitization.
//!
//! Routes raw strings destined for terminal output, structured-log
//! rendering, or API response serialization through a single normalizer:
//! unprintable controls and bidi-attack format chars become visible
//! `\u{XXXX}` escapes, UTF-8 content (multibyte scripts, emoji,
//! bidi-neutral text) is preserved verbatim, and input is capped at 8192
//! bytes with an explicit dropped-byte marker.
//!
//! **Predicate scope.** The spec's short form says "any `char::is_control()`
//! char is escaped", but U+202E RIGHT-TO-LEFT OVERRIDE is Unicode category
//! `Cf` (Format), not `Cc` (Control) — `char::is_control()` returns false
//! for it. The required test for U+202E pins the real intent: escape the
//! bidi embedding/override/isolate family (U+202A–U+202E, U+2066–U+2069)
//! in addition to the `Cc` set. Benign `Cf` chars (ZWJ, ZWNJ, LRM/RLM
//! directional marks used in legitimate RTL text) are NOT escaped.
//!
//! **Call-site discipline.** Invoke at the display boundary — CLI error
//! printer, structured-log rendering, API response serializer. Do **NOT**
//! call inside `thiserror::Error` `#[error(...)]` format strings at
//! construction. The stored error value must carry the raw diagnostic;
//! only the rendered form is sanitized. See architecture.md
//! §"Display Boundary Sanitization" for the locked rule.

use std::fmt::Write;

/// Maximum byte length of input that is fully preserved. Inputs above
/// this ceiling are truncated at the nearest UTF-8 char boundary and
/// get a `… [truncated <N> bytes]` marker appended, where `<N>` is the
/// exact byte count of the dropped tail.
const MAX_INPUT_BYTES: usize = 8192;

/// Sanitize a string for safe display at a human/API boundary.
///
/// Preserves `\n`, `\r`, `\t` and all non-control chars (including
/// multibyte UTF-8) verbatim. Every other `char::is_control()` char —
/// including bidi overrides (U+202E) and ANSI escapes (U+001B) — is
/// rewritten as `\u{XXXX}` with lowercase hex padded to at least 4
/// digits. Input longer than 8192 bytes is truncated at a UTF-8 char
/// boundary and the dropped-tail byte count is surfaced explicitly.
pub fn sanitize_human(input: &str) -> String {
    let (slice, dropped) = truncate_at_char_boundary(input, MAX_INPUT_BYTES);

    let mut out = String::with_capacity(slice.len());
    for ch in slice.chars() {
        if needs_escape(ch) {
            let _ = write!(out, "\\u{{{:04x}}}", ch as u32);
        } else {
            out.push(ch);
        }
    }

    if dropped > 0 {
        let _ = write!(out, "… [truncated {dropped} bytes]");
    }

    out
}

fn needs_escape(ch: char) -> bool {
    // Preserved-whitespace escape hatch: these three control chars are
    // common in log lines and the spec wants them verbatim.
    if matches!(ch, '\n' | '\r' | '\t') {
        return false;
    }
    // Cc — C0 controls, C1 controls, and DEL. Covers the ANSI-escape
    // required test (`\x1b` → U+001B is Cc).
    if ch.is_control() {
        return true;
    }
    // Bidi-attack Cf subset — embedding (LRE/RLE), override (LRO/RLO),
    // and isolate (LRI/RLI/FSI/PDI) format chars. The required test
    // for U+202E RLO lives here; the other bidi-control chars in the
    // same block behave identically and are escaped for consistency.
    matches!(ch as u32, 0x202A..=0x202E | 0x2066..=0x2069)
}

fn truncate_at_char_boundary(input: &str, max_bytes: usize) -> (&str, usize) {
    if input.len() <= max_bytes {
        return (input, 0);
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    (&input[..end], input.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_printable_and_preserved_whitespace_pass_verbatim() {
        let input = "Hello, World!\nLine 2\tTabbed\rCR";
        assert_eq!(sanitize_human(input), input);
    }

    #[test]
    fn multibyte_and_emoji_pass_verbatim() {
        let input = "日本語 — 🎉 café naïve";
        assert_eq!(sanitize_human(input), input);
    }

    #[test]
    fn bidi_override_u202e_escapes_as_lowercase_hex() {
        // Required test: U+202E RIGHT-TO-LEFT OVERRIDE must surface as
        // an escape so a terminal can't be tricked into reversing the
        // visual order of adjacent text (classic bidi attack).
        let input = "safe\u{202e}tpirhs";
        assert_eq!(sanitize_human(input), "safe\\u{202e}tpirhs");
    }

    #[test]
    fn full_bidi_attack_family_is_escaped() {
        // Pin the broadening documented on `needs_escape`: the same
        // reasoning that demands U+202E also covers LRE/RLE/LRO/PDF
        // and the isolate family. A regression that narrowed the
        // predicate back to Cc-only would flip these through verbatim.
        for cp in [
            0x202Au32, 0x202B, 0x202C, 0x202D, 0x2066, 0x2067, 0x2068, 0x2069,
        ] {
            let ch = char::from_u32(cp).expect("valid scalar");
            let input: String = ch.to_string();
            let expected = format!("\\u{{{cp:04x}}}");
            assert_eq!(sanitize_human(&input), expected, "cp={cp:#x}");
        }
    }

    #[test]
    fn legitimate_directional_marks_pass_verbatim() {
        // LRM (U+200E) and RLM (U+200F) appear in legitimate Arabic
        // and Hebrew text; they are Cf but not part of the bidi-attack
        // family we escape. Preserve them verbatim.
        assert_eq!(sanitize_human("a\u{200e}b"), "a\u{200e}b");
        assert_eq!(sanitize_human("a\u{200f}b"), "a\u{200f}b");
    }

    #[test]
    fn ansi_csi_clear_screen_escapes_the_esc_char() {
        // Required test: ESC (U+001B) must be escaped. The body of a
        // CSI sequence is printable and is preserved — only the ESC
        // anchor is rewritten, so the reader still sees the shape of
        // the attack.
        let input = "\x1b[2J\x1b[H<fake-prompt>";
        let expected = "\\u{001b}[2J\\u{001b}[H<fake-prompt>";
        assert_eq!(sanitize_human(input), expected);
    }

    #[test]
    fn low_control_is_zero_padded_to_four_hex_digits() {
        assert_eq!(sanitize_human("\u{0001}"), "\\u{0001}");
    }

    #[test]
    fn hex_escape_is_lowercase() {
        assert_eq!(sanitize_human("\u{000f}"), "\\u{000f}");
    }

    #[test]
    fn del_and_c1_controls_are_escaped() {
        // DEL (U+007F) and NEL (U+0085) are both `is_control()` per
        // Unicode Cc, beyond the C0 range — the rule applies uniformly.
        assert_eq!(sanitize_human("\u{007f}"), "\\u{007f}");
        assert_eq!(sanitize_human("\u{0085}"), "\\u{0085}");
    }

    #[test]
    fn long_input_truncated_with_exact_dropped_byte_count() {
        // Required test: >8192 bytes → truncation marker with exact
        // dropped-byte count. 10_000 - 8_192 = 1_808 bytes dropped.
        let input = "a".repeat(10_000);
        let out = sanitize_human(&input);
        assert!(
            out.ends_with("… [truncated 1808 bytes]"),
            "marker missing or wrong count: {out}"
        );
        assert_eq!(&out[..8192], &"a".repeat(8192));
    }

    #[test]
    fn truncation_snaps_to_char_boundary_not_mid_utf8() {
        // Place a 4-byte emoji spanning the 8192 mark: 8190 a's put
        // bytes 0..8190, then a 🎉 would occupy 8190..8194 — crossing
        // the cap. The truncator must snap BEFORE the emoji at byte
        // 8190, and the dropped count must include all 4 emoji bytes
        // plus the trailing padding.
        let padding = "a".repeat(8_190);
        let emoji = "🎉";
        let tail = "b".repeat(100);
        let input = format!("{padding}{emoji}{tail}");

        let out = sanitize_human(&input);
        assert_eq!(&out[..8_190], &"a".repeat(8_190));
        assert!(
            out.ends_with("… [truncated 104 bytes]"),
            "boundary snap wrong: {out}"
        );
    }

    #[test]
    fn input_at_ceiling_is_not_truncated() {
        let input = "a".repeat(MAX_INPUT_BYTES);
        let out = sanitize_human(&input);
        assert_eq!(out, input);
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn empty_input_returns_empty_string() {
        assert_eq!(sanitize_human(""), "");
    }
}

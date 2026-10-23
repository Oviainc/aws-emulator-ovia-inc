// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of the UTC offset an RFC 3339 timestamp declares.
//!
//! Several emulated services take a timestamp as a STRING — a member the Smithy model
//! targets at a plain string shape rather than at `timestamp`, so the framework never
//! parses it — and then have to compare two of them, or order them, or report the instant
//! one names. The reader those services reach for
//! (`aws_smithy_types::DateTime::from_str` with `Format::DateTime`) accepts the `Z` form
//! only, so the numeric offset RFC 3339 also admits — and which
//! `datetime.isoformat()` produces, so a real client sends it — has to be resolved
//! separately: read the CIVIL part as UTC and shift it by the offset the text declares.
//!
//! That shift is one rule, so it is stated here once rather than per service: two
//! services asked which instant a timestamp names must not answer differently, and a
//! second hand-written copy is also a second chance to get the slicing wrong. The
//! calendar conversion (month lengths, leap years, a fractional second) still comes from
//! the maintained library — only the constant offset is applied here.
//!
//! [`split_utc_offset`] is TOTAL over arbitrary UTF-8: the text it reads comes straight
//! off the wire, so every form it does not recognise answers `None` for the caller to
//! refuse, and none of them can panic. Non-ASCII text is refused before any index is
//! taken, which is what makes every byte index below a character boundary too.
//!
//! `services/service-com-amazonaws-costexplorer-awsinsightsindexservice` carried the
//! first copy of this rule and now calls it here; nothing new should write another.

/// Split an RFC 3339 timestamp into its CIVIL date-time and the UTC offset its suffix
/// declares, in seconds.
///
/// `Some((civil, offset))` where `civil` is the text with the zone suffix removed, so a
/// caller reads the instant as `parse(format!("{civil}Z")) - offset`. The four suffix
/// forms RFC 3339 and ISO 8601 admit are read — `Z` (or lowercase `z`), `±hh`, `±hhmm`
/// and `±hh:mm` — and nothing else: `None` for text carrying no readable zone, an offset
/// whose hours or minutes are out of range, or any non-ASCII character.
///
/// The sign is looked for inside the TIME part only, because the date's own separators
/// are hyphens, so a `-` before the `T` belongs to the date and not to an offset.
#[must_use]
pub fn split_utc_offset(value: &str) -> Option<(&str, i64)> {
    // Every index below is a BYTE index, and this is what makes each of them a character
    // boundary as well: after this line the text is one byte per character.
    if !value.is_ascii() {
        return None;
    }
    if let Some(civil) = value
        .strip_suffix('Z')
        .or_else(|| value.strip_suffix('z'))
    {
        return Some((civil, 0));
    }
    let time_start = value.find(['T', 't'])? + 1;
    let sign_at = time_start + value.get(time_start..)?.rfind(['+', '-'])?;
    let (civil, suffix) = value.split_at(sign_at);
    let sign: i64 = match suffix.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits = suffix.get(1..)?;
    let (hours, minutes) = match digits.len() {
        2 => (digits.get(..2)?, "00"),
        4 => (digits.get(..2)?, digits.get(2..)?),
        5 if digits.as_bytes().get(2) == Some(&b':') => (digits.get(..2)?, digits.get(3..)?),
        _ => return None,
    };
    // `parse` alone would admit a sign inside the field (`+-123`), so the fields are
    // required to be digits before they are read as numbers.
    if !hours.bytes().all(|byte| byte.is_ascii_digit())
        || !minutes.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some((civil, sign * (hours * 3_600 + minutes * 60)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row: what a failing assertion calls it, the text read, and the split expected.
    type Row = (&'static str, &'static str, Option<(&'static str, i64)>);

    /// The rows differ along what the rule generalises over: which of the four suffix
    /// forms the zone takes, its sign and magnitude, whether the civil part carries a
    /// fractional second, the casing of the designators, and every shape of unreadable
    /// zone — including the non-ASCII text that must answer `None` rather than panic on a
    /// byte index inside a character.
    #[test]
    fn the_four_zone_forms_are_read_and_everything_else_is_refused() {
        let rows: [Row; 20] = [
            (
                "a Z instant",
                "2026-01-01T00:00:00Z",
                Some(("2026-01-01T00:00:00", 0)),
            ),
            (
                "a lowercase z instant",
                "2026-01-01T00:00:00z",
                Some(("2026-01-01T00:00:00", 0)),
            ),
            (
                "a Z instant carrying a fractional second",
                "2026-01-01T00:00:00.250Z",
                Some(("2026-01-01T00:00:00.250", 0)),
            ),
            (
                "a zero offset in the hh:mm form",
                "2026-02-03T10:15:00+00:00",
                Some(("2026-02-03T10:15:00", 0)),
            ),
            (
                "a positive offset in the hh:mm form",
                "2026-02-03T10:15:00+02:00",
                Some(("2026-02-03T10:15:00", 7_200)),
            ),
            (
                "a negative offset in the hh:mm form",
                "2026-02-03T10:15:00-05:30",
                Some(("2026-02-03T10:15:00", -19_800)),
            ),
            (
                "a positive offset in the hhmm form",
                "2026-02-03T10:15:00+0245",
                Some(("2026-02-03T10:15:00", 9_900)),
            ),
            (
                "a negative offset in the hh form",
                "2026-02-03T10:15:00-07",
                Some(("2026-02-03T10:15:00", -25_200)),
            ),
            (
                "the largest offset the hours field admits",
                "2026-02-03T10:15:00+23:59",
                Some(("2026-02-03T10:15:00", 86_340)),
            ),
            (
                "a lowercase date/time designator",
                "2026-02-03t10:15:00+01:00",
                Some(("2026-02-03t10:15:00", 3_600)),
            ),
            (
                "an offset on a civil part carrying a fractional second",
                "2026-02-03T10:15:00.125-01:00",
                Some(("2026-02-03T10:15:00.125", -3_600)),
            ),
            ("no zone at all", "2026-02-03T10:15:00", None),
            ("a date with no time part", "2026-02-03", None),
            ("epoch seconds", "1767225600", None),
            ("not a timestamp", "yesterday", None),
            (
                "an hours field past the range",
                "2026-02-03T10:15:00+24:00",
                None,
            ),
            (
                "a minutes field past the range",
                "2026-02-03T10:15:00+01:60",
                None,
            ),
            (
                "a sign inside the offset's fields",
                "2026-02-03T10:15:00+-123",
                None,
            ),
            (
                "an offset of a length no form admits",
                "2026-02-03T10:15:00+123",
                None,
            ),
            // The row the byte-vs-character rule exists for: a four-BYTE suffix whose
            // second byte is inside a character.
            (
                "a non-ASCII character where the offset goes",
                "2026-02-03T10:15:00+aéb",
                None,
            ),
        ];

        for (case, value, expected) in rows {
            let split = split_utc_offset(value);
            assert_eq!(
                split, expected,
                "case {case:?}: expected {expected:?} for {value:?}, got {split:?}"
            );
        }
    }

    /// Every prefix and suffix of a timestamp carrying a multi-byte character is read
    /// without panicking: the rule is that the reader is TOTAL over arbitrary text, not
    /// that one recorded string is handled.
    #[test]
    fn the_reader_is_total_over_arbitrary_text() {
        let base = "2026-02-03T10:15:00+aéb€1-2:3Z";
        for end in 0..=base.len() {
            if !base.is_char_boundary(end) {
                continue;
            }
            for start in 0..=end {
                if !base.is_char_boundary(start) {
                    continue;
                }
                let value = &base[start..end];
                // The assertion is that this returns at all: a panic here is the defect.
                let split = split_utc_offset(value);
                if let Some((civil, offset)) = split {
                    assert!(
                        civil.len() <= value.len() && (-86_340..=86_340).contains(&offset),
                        "case {value:?}: a readable zone must leave a shorter civil part \
                         and an in-range offset, got {split:?}"
                    );
                }
            }
        }
    }
}

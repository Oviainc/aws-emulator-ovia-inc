// SPDX-License-Identifier: CC0-1.0

//! The one transcoding CloudFormation applies to the FREE TEXT a caller hands it to store:
//! the text is held as ASCII, so a codepoint outside ASCII reads back as one `?`.
//!
//! Two stores in this service hold caller-authored text that is neither an identifier nor a
//! document this service parses — a stack's (change set's, stack set's) `Tags` and an
//! extension's type configuration — and both report such a codepoint back as a single `?`,
//! one question mark per codepoint, whatever its UTF-8 encoding takes. Nothing else about
//! the text changes: its length in codepoints, its ordering and every ASCII character in it
//! survive, so a document's structure and a tag's shape are untouched and only the
//! characters the store cannot hold differ.
//!
//! Applied at the WRITE by each of those stores, so what a read reports is what the store
//! holds rather than a second transformation a read invented.

/// `text` as the store holds it: every codepoint outside ASCII replaced by one `?`.
///
/// Per CODEPOINT, not per byte: a two-, three- or four-byte encoding is one `?`, which is
/// what keeps the length in characters (the unit the `@length` constraints count in) the
/// same as the text submitted.
pub fn ascii_text(text: &str) -> String {
    text.chars()
        .map(|character| if character.is_ascii() { character } else { '?' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_text_survives_and_everything_past_it_is_one_question_mark() {
        // Each row: the text submitted, and the text the store holds for it.
        for (case, submitted, held) in [
            ("wholly ASCII", "Owner:team-1/build+v1.0", "Owner:team-1/build+v1.0"),
            ("the first ASCII codepoint", "a\u{0}b", "a\u{0}b"),
            ("the last ASCII codepoint", "a\u{7f}b", "a\u{7f}b"),
            ("the first codepoint past ASCII", "caf\u{80}", "caf?"),
            ("a Latin-1 letter", "Bj\u{f6}rk", "Bj?rk"),
            ("a two-byte encoding", "\u{41f}\u{440}\u{438}", "???"),
            ("a three-byte encoding", "\u{65e5}\u{672c}\u{8a9e}", "???"),
            ("a four-byte encoding, one codepoint", "\u{1f642}", "?"),
            ("the empty text", "", ""),
        ] {
            let actual = ascii_text(submitted);
            assert_eq!(
                actual, held,
                "case {case:?}: expected {held:?}, got {actual:?}"
            );
            assert_eq!(
                actual.chars().count(),
                submitted.chars().count(),
                "case {case:?}: expected the length in codepoints kept, got {actual:?}"
            );
        }
    }
}

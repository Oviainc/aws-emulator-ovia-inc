// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of the published form of an IAM entity PATH.
//!
//! IAM's `pathType` publishes the form in prose — "either a forward slash (/) by itself or a
//! string that must begin and end with forward slashes", whose inner characters are the ASCII
//! range `\u0021`..`\u007E`, within a length of 1..512 (`CreateUser$Path`, and the shape's
//! own `@length`/`@pattern`) — and its `@pattern`,
//! `^(\u002F)|(\u002F[\u0021-\u007E]+\u002F)$`, is not anchored on both sides of its `|`.
//! Smithy's `@pattern` is a FIND (`vendor/smithy-schema-server/src/constraints.rs` says so of
//! the trait it implements), so that pattern admits any value merely STARTING with the
//! delimiter: the model-level walk cannot decide this form, and whoever takes a `Path` has to.
//!
//! Read here rather than in one service so the services that take an IAM path, and the ones
//! that build one, cannot disagree about what a path IS.

/// Whether `path` is in the published form of IAM's `pathType`.
///
/// ONE pass over the value's characters and nothing else: no index into the string, no slice,
/// no arithmetic on the value, no allocation and no regex — so it cannot panic on any input,
/// whatever bytes those characters take, and the member's own declared length is the bound on
/// the pass.
pub fn is_published_form(path: &str) -> bool {
    // The account root is the whole form on its own.
    if path == ROOT {
        return true;
    }
    if !(MIN_NESTED_CHARACTERS..=MAX_CHARACTERS).contains(&path.chars().count()) {
        return false;
    }
    // Delimited at BOTH ends, with at least one inner character, every one of them inside the
    // published ASCII range.
    path.strip_prefix(DELIMITER)
        .and_then(|rest| rest.strip_suffix(DELIMITER))
        .is_some_and(|inner| {
            !inner.is_empty() && inner.chars().all(|c| (FIRST_INNER..=LAST_INNER).contains(&c))
        })
}

/// The account root: the one path every entity whose request omits `Path` is filed under.
const ROOT: &str = "/";

/// The character a path is delimited by.
const DELIMITER: char = '/';

/// The first and last characters of the published inner range (`\u0021`..`\u007E`).
const FIRST_INNER: char = '\u{21}';
const LAST_INNER: char = '\u{7E}';

/// The longest path the shape admits (`pathType`, length 1..512).
const MAX_CHARACTERS: usize = 512;

/// The shortest path past the root: two delimiters around at least one character.
const MIN_NESTED_CHARACTERS: usize = 3;

#[cfg(test)]
mod tests {
    use super::is_published_form;

    /// The form admits the account root, a one-segment path and a nested one, and the whole
    /// published inner range; it refuses a value missing either delimiter, an empty segment, a
    /// character under the published range, and a value past the declared length.
    #[test]
    fn the_published_form_is_both_delimiters_and_the_published_inner_range() {
        let longest = format!("/{}/", "a".repeat(510));
        let past = format!("/{}/", "a".repeat(511));
        let rows: &[(&str, bool)] = &[
            ("/", true),
            ("/eng/", true),
            ("/eng/tools/", true),
            ("/a!~/", true),
            (&longest, true),
            ("", false),
            ("eng/", false),
            ("/eng", false),
            ("eng", false),
            ("//", false),
            ("/has space/", false),
            ("/caf\u{e9}/", false),
            (&past, false),
        ];
        for (path, inside) in rows {
            assert_eq!(
                is_published_form(path),
                *inside,
                "case {path:?}: expected inside-the-form {inside}, got {}",
                is_published_form(path)
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! What every suite in this crate's test target needs, in ONE place.
//!
//! A suite reaches it as `use super::harness::label;` — `tests/all.rs` is the test
//! target's crate root, so each suite is a sibling module of this one.

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// Everything outside RFC 3986's unreserved set (`ALPHA / DIGIT / "-" / "." / "_" /
/// "~"`), which is exactly what an AWS SDK escapes in a URI path segment or query value.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encode one `@httpLabel` / `@httpQuery` value the way an AWS SDK does, so an
/// `eventId`'s `#`, a memory ARN's `:` and `/`, and any other punctuation reach the
/// server as part of the value rather than as URI syntax.
pub(crate) fn label(value: &str) -> String {
    utf8_percent_encode(value, UNRESERVED).to_string()
}

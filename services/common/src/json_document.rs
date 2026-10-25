// SPDX-License-Identifier: CC0-1.0

//! The ONE minification of a JSON document a service stores and reports back.
//!
//! Several emulated services take a JSON document as an opaque string member — a policy,
//! a lifecycle rule set, an approval rule template — and report it back on every read.
//! Such a service does not echo the submitted formatting: what it reports is the document
//! with the insignificant whitespace between its JSON tokens removed, and nothing else
//! about it changed, so a caller that stores the reported text, diffs it against a later
//! read or hashes it to detect drift sees a difference only when the document itself
//! differs.
//!
//! The whitespace is dropped by SCANNING the text rather than by re-serializing a parsed
//! value: a re-serialization would sort the members (`serde_json`'s `Map` is a `BTreeMap`
//! unless its `preserve_order` feature is on) and respell number literals (`1e3` as
//! `1000.0`), both of which a document reported back keeps. Whitespace inside a string
//! literal is part of the value, so it survives, escapes included.
//!
//! The scan is TOTAL over arbitrary UTF-8: the text comes straight off the wire, so text
//! that is not JSON at all is simply returned with the whitespace outside its quotes
//! dropped. Deciding whether a document is acceptable is the caller's own validation,
//! which runs before this.
//!
//! Four services carried a copy of this scan before it was stated here (the ECR lifecycle
//! policy document calls it; the CodeCommit approval rule template content, the RUM
//! resource policy and the ACM PCA resource policy still hold their own). Nothing new
//! should write another.

/// `document` with the insignificant whitespace between its JSON tokens removed.
///
/// Member order, string literals (escapes and inner whitespace included) and number
/// literals are reported exactly as they arrived; only the space, tab, carriage return
/// and line feed OUTSIDE a string literal — the four characters JSON calls insignificant
/// whitespace — are dropped.
#[must_use]
pub fn minified(document: &str) -> String {
    let mut minified = String::with_capacity(document.len());
    // Where the scan is: inside a string literal, and whether the previous character was
    // the backslash of an escape (so an escaped quote does not close the literal).
    let mut in_string = false;
    let mut escaped = false;
    for character in document.chars() {
        if in_string {
            minified.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                minified.push(character);
            }
            ' ' | '\t' | '\r' | '\n' => {}
            _ => minified.push(character),
        }
    }
    minified
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row: what a failing assertion calls it, the text submitted, and the text
    /// reported back.
    type Row = (&'static str, &'static str, &'static str);

    /// The rows differ along what the rule generalises over: where the whitespace sits
    /// (between separators, as indentation, around the document), whether it is inside a
    /// string literal (where it belongs to the value), how a string's escapes are
    /// spelled, how a number is spelled, and whether the text is a JSON document at all.
    #[test]
    fn whitespace_between_tokens_is_dropped_and_nothing_else_moves() {
        let rows: [Row; 12] = [
            ("empty text", "", ""),
            (
                "a document already carrying none",
                "{\"a\":1,\"b\":[2,3]}",
                "{\"a\":1,\"b\":[2,3]}",
            ),
            (
                "a space after every separator",
                "{\"a\": 1, \"b\": [2, 3]}",
                "{\"a\":1,\"b\":[2,3]}",
            ),
            (
                "indented over several lines",
                "{\n  \"a\": 1,\n  \"b\": [\n    2,\n    3\n  ]\n}",
                "{\"a\":1,\"b\":[2,3]}",
            ),
            (
                "wrapped in whitespace a parser ignores",
                " \t\r\n{\"a\":1} \n",
                "{\"a\":1}",
            ),
            (
                "whitespace and separators inside a string",
                "{\"a\": \"one:  two, three\"}",
                "{\"a\":\"one:  two, three\"}",
            ),
            (
                "a tab inside a string, written as an escape",
                "{\"a\": \"one\\ttwo\"}",
                "{\"a\":\"one\\ttwo\"}",
            ),
            (
                "an escaped quote that does not close the string",
                "{\"a\": \"one \\\" two\", \"b\": 1}",
                "{\"a\":\"one \\\" two\",\"b\":1}",
            ),
            (
                "an escaped backslash before the closing quote",
                "{\"a\": \"one\\\\\", \"b\": 1}",
                "{\"a\":\"one\\\\\",\"b\":1}",
            ),
            (
                "members in an order no re-serialization would keep",
                "{\"z\": 1, \"a\": 2}",
                "{\"z\":1,\"a\":2}",
            ),
            (
                "numbers keeping their own spelling",
                "{\"a\": 1e3, \"b\": 1.50, \"c\": -0.0}",
                "{\"a\":1e3,\"b\":1.50,\"c\":-0.0}",
            ),
            (
                "text that is not a JSON document",
                "not { a document",
                "not{adocument",
            ),
        ];

        for (case, submitted, reported) in rows {
            let actual = minified(submitted);
            assert_eq!(
                actual, reported,
                "case {case:?}: expected {reported:?}, got {actual:?}"
            );
            // Minifying what was already minified changes nothing: the reported form is a
            // fixed point, so a document read back and put again reports the same bytes.
            let again = minified(&actual);
            assert_eq!(
                again, actual,
                "case {case:?}: the reported form must be a fixed point, got {again:?}"
            );
        }
    }
}

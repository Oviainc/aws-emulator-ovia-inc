// SPDX-License-Identifier: CC0-1.0

//! The two things every awsQuery suite in this test crate needs: the percent-encoder that
//! builds a form body, and the readers that pull one element's text — or every occurrence
//! of a repeated one — out of the XML answer.
//!
//! They live here, ONCE, because a suite that writes its own drifts from the others: an
//! encoder that leaves one more byte literal, or a reader that stops at a different tag,
//! changes what the suite is actually asserting.
//!
//! The encoding is written out rather than taken from the maintained `percent-encoding`
//! crate for one reason, and it is not `Cargo.lock`: `core/router/Cargo.toml` is a PINNED
//! path in this repository (only `services/`, `core/router/tests/`, the issue files and
//! `Cargo.lock` are editable by a fix), so no dev-dependency can be added to this test
//! crate at all. The service crates under `services/`, whose manifests a fix may change,
//! take the crate instead — see `percent-encoding` in
//! `services/service-com-amazonaws-docdb-amazonrdsv19/Cargo.toml`. What cannot be taken
//! from a crate here is at least written once, behind one name.
//!
//! `cloudfront_key_value_store_handshake.rs` and `iam_signing_name_routing.rs` still carry
//! their own element readers, written before this module existed; they are to be migrated
//! onto [`opt_element`] / [`element`] by their own suites' issues. Nothing new should write
//! another one — call this module.

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters (`ALPHA / DIGIT / "-" / "." / "_" / "~"`) literal.
///
/// Byte-wise, so a multi-byte character is escaped byte by byte, which is what the UTF-8
/// form encoding an AWS SDK sends does.
pub(crate) fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an XML answer, or `None` when the
/// answer carries no such element.
///
/// A reader, not an XML parser, and only for documents this emulator itself produced:
/// machine-generated element trees with no comments, no CDATA and no same-name nesting.
pub(crate) fn opt_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// [`opt_element`] where the element must be there; panics quoting the whole answer.
pub(crate) fn element(body: &str, name: &str) -> String {
    opt_element(body, name).unwrap_or_else(|| panic!("no <{name}> element in {body}"))
}

/// The text of EVERY `<name>…</name>` element of an XML answer, in document order, with
/// the five entities this emulator's XML serializer escapes decoded back — so a value that
/// is itself a document (an attribute carrying JSON, a policy) compares as the service
/// wrote it rather than as its escaping.
///
/// The repeated-element and decoding counterpart of [`opt_element`], for a suite reading a
/// LIST of a tag (every `<OutputValue>`, every `<key>`); the same reader, not an XML
/// parser, and under the same assumption about the documents it is pointed at.
pub(crate) fn elements(body: &str, name: &str) -> Vec<String> {
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(unescape(&after[..end]));
        rest = &after[end + close.len()..];
    }
    found
}

/// The five entities an XML serializer escapes, decoded back. `&amp;` LAST, so a value
/// that carried a literal `&lt;` is not turned into a `<`.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The encoder escapes everything outside the unreserved set and nothing inside it,
    /// over the classes an awsQuery component runs into.
    #[test]
    fn a_form_component_keeps_only_the_unreserved_characters() {
        // (case, the value, its encoding)
        let rows: [(&str, &str, &str); 6] = [
            ("the unreserved set", "aZ09-._~", "aZ09-._~"),
            ("a space", "a b", "a%20b"),
            ("an ARN's colons and slashes", "arn:aws:rds:x/y", "arn%3Aaws%3Ards%3Ax%2Fy"),
            ("the form separators", "a&b=c", "a%26b%3Dc"),
            ("XML markup", "<a>&</a>", "%3Ca%3E%26%3C%2Fa%3E"),
            ("a non-ASCII character", "é", "%C3%A9"),
        ];

        for (case, value, expected) in rows {
            let encoded = form_encode(value);
            assert_eq!(
                encoded, expected,
                "case {case:?}: expected {expected:?}, got {encoded:?}"
            );
        }
    }

    /// The reader answers the first element's text, and `None` for one that is absent.
    #[test]
    fn the_reader_finds_the_first_element_or_nothing() {
        let body = "<Root><Code>A</Code><Code>B</Code></Root>";
        assert_eq!(
            opt_element(body, "Code"),
            Some("A".to_string()),
            "case \"the first of two\": got {body}"
        );
        assert_eq!(
            opt_element(body, "Message"),
            None,
            "case \"an element the answer does not carry\": got {body}"
        );
        assert_eq!(
            element(body, "Root"),
            "<Code>A</Code><Code>B</Code>",
            "case \"a parent element\": got {body}"
        );
    }

    /// The repeated-element reader answers every one of them, in document order, with the
    /// serializer's escaping undone.
    #[test]
    fn the_repeated_element_reader_finds_every_one_decoded() {
        // (case, the answer, the tag read, the texts it carries)
        let rows: [(&str, &str, &str, &[&str]); 4] = [
            (
                "two of a tag",
                "<R><Code>A</Code><Code>B</Code></R>",
                "Code",
                &["A", "B"],
            ),
            (
                "a tag the answer does not carry",
                "<R><Code>A</Code></R>",
                "Message",
                &[],
            ),
            (
                "an escaped document value",
                "<R><value>{&quot;a&quot;:1}</value></R>",
                "value",
                &[r#"{"a":1}"#],
            ),
            (
                "every entity the serializer escapes",
                "<R><value>&lt;a&gt; &amp; &apos;b&apos;</value></R>",
                "value",
                &["<a> & 'b'"],
            ),
        ];

        for (case, body, tag, expected) in rows {
            let read = elements(body, tag);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}

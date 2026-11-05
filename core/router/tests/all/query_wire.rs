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
//!
//! [`pairs_of`] and the two readers over it ([`outputs_of`], [`physical_ids_of`]) are here
//! for the same reason: a suite that pairs a keyed list by zipping two document-ordered
//! element lists reads the wrong pair the moment one member omits half of it, so the pairing
//! is written once, over the `<member>` that carries both.

use std::collections::BTreeMap;

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

/// The inner text of the first `<name …>…</name>` element, whose own tag MAY carry
/// attributes.
///
/// [`opt_element`] matches the closing bracket as part of the tag, so it finds NOTHING for an
/// element the serializer wrote with an attribute (`<Metadata xmlns="…">…`, which is how the
/// awsQuery serializer writes a structure member). This reads from the tag's OWN closing
/// bracket instead — the technique [`members_of`] uses for a list member — so a suite
/// reading such an element scopes the read to it rather than falling back on "the first
/// `<Name>` in the whole document". The tag matches only where the NAME ends, so
/// `<MetadataEntry>` is not this element.
pub(crate) fn block_of<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let (open, close) = (format!("<{name}"), format!("</{name}>"));
    let mut rest = body;
    while let Some(at) = rest.find(&open) {
        let after = &rest[at + open.len()..];
        match after.chars().next() {
            Some(next) if next == '>' || next == '/' || next.is_whitespace() => {
                let (_, inner) = after.split_once('>')?;
                return inner.split_once(close.as_str()).map(|(text, _)| text);
            }
            _ => rest = after,
        }
    }
    None
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

/// Each `<member>` block of an awsQuery answer's LIST, in document order.
///
/// The serializer writes the FIRST member of a list with the service's namespace attribute
/// (`<member xmlns="…">`) and the rest bare, so a block starts after its tag's own closing
/// bracket and ends at its closing tag. The lists read through this carry no member inside a
/// member.
pub(crate) fn members_of(xml: &str) -> Vec<&str> {
    xml.split("<member")
        .skip(1)
        .filter_map(|block| block.split_once('>'))
        .filter_map(|(_, body)| body.split("</member>").next())
        .collect()
}

/// Each `<item>` block of an ec2Query answer's LIST, in document order.
///
/// [`members_of`] reads the awsQuery `<member>` form; an ec2Query list wraps each entry in
/// `<item>` instead, so a suite reading one route or one association out of a route table
/// scopes the read to that entry rather than to the first element of that name anywhere in
/// the answer. Here for the reason this module exists: a suite that writes its own drifts
/// from the others.
pub(crate) fn items_of(xml: &str) -> Vec<&str> {
    xml.split("<item")
        .skip(1)
        .filter_map(|block| block.split_once('>'))
        .filter_map(|(_, body)| body.split("</item>").next())
        .collect()
}

/// One pair of elements per list member, as the map a client reads a keyed list as.
///
/// Each pair is read from the member that carries BOTH, rather than by zipping two
/// document-ordered lists: a member that ever omitted one of them would silently misalign the
/// two lists, so a suite would compare the wrong pair instead of failing. A member carrying
/// only one half contributes nothing.
pub(crate) fn pairs_of(xml: &str, key: &str, value: &str) -> BTreeMap<String, String> {
    members_of(xml)
        .into_iter()
        .filter_map(|member| {
            let key = elements(member, key).into_iter().next()?;
            let value = elements(member, value).into_iter().next()?;
            Some((key, value))
        })
        .collect()
}

/// An awsQuery answer's MAP as the map a client reads it as: each entry's key paired with
/// its own value, with the escaped entities decoded — so a value that is itself a document
/// (a topic's access policy, a queue's redrive policy) compares as the service wrote it.
///
/// The decoding counterpart of [`entries_of`], and built on it rather than by zipping a list
/// of keys against a list of values, for the reason that reader records: an entry carrying
/// only one half would misalign every pair after it instead of being left out.
pub(crate) fn decoded_entries(xml: &str) -> BTreeMap<String, String> {
    entries_of(xml)
        .into_iter()
        .map(|(key, value)| (key, unescape(value)))
        .collect()
}

/// Each `<entry>` block of an awsQuery answer's MAP, as `(key, the value block)`.
///
/// The serializer writes a map as `<entry><key>…</key><value>…</value></entry>`, with the
/// first entry (and the first member inside it) carrying the service's namespace
/// attribute, so both halves are read through [`block_of`] rather than by tag text. Here
/// for the same reason as [`pairs_of`]: each pair is read from the entry that carries
/// BOTH, so an entry missing one half contributes nothing instead of misaligning the rest.
pub(crate) fn entries_of(xml: &str) -> Vec<(String, &str)> {
    xml.split("<entry")
        .skip(1)
        .filter_map(|block| block.split_once('>'))
        .filter_map(|(_, body)| body.split("</entry>").next())
        .filter_map(|entry| {
            let key = block_of(entry, "key")?.to_string();
            let value = block_of(entry, "value")?;
            Some((key, value))
        })
        .collect()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
pub(crate) fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    pairs_of(xml, "OutputKey", "OutputValue")
}

/// A `DescribeStackResources` answer as the logical-id -> physical-id map a client reads it
/// as: each member carries one of each.
pub(crate) fn physical_ids_of(xml: &str) -> BTreeMap<String, String> {
    pairs_of(xml, "LogicalResourceId", "PhysicalResourceId")
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

    /// The attribute-tolerant reader scopes the read to the element it names, over the tag
    /// forms this emulator's awsQuery serializer writes: a bare tag, one carrying the
    /// service's namespace attribute, and a tag whose name is a PREFIX of another's.
    #[test]
    fn the_block_reader_finds_an_element_whose_tag_carries_attributes() {
        // (case, the answer, the tag read, its inner text)
        let rows: [(&str, &str, &str, Option<&str>); 5] = [
            (
                "a bare tag",
                "<R><M><Name>a</Name></M></R>",
                "M",
                Some("<Name>a</Name>"),
            ),
            (
                "a tag carrying the service's namespace",
                "<R><M xmlns=\"http://x/doc/\"><Name>a</Name></M></R>",
                "M",
                Some("<Name>a</Name>"),
            ),
            (
                "a tag whose name is a prefix of the one asked for",
                "<R><MEntry><Name>b</Name></MEntry><M><Name>a</Name></M></R>",
                "M",
                Some("<Name>a</Name>"),
            ),
            (
                "the first of two of a tag",
                "<R><M>first</M><M>second</M></R>",
                "M",
                Some("first"),
            ),
            (
                "a tag the answer does not carry",
                "<R><M>a</M></R>",
                "Other",
                None,
            ),
        ];

        for (case, body, tag, expected) in rows {
            let read = block_of(body, tag);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
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

    /// The list reader pairs the two elements of each member with EACH OTHER, over the member
    /// forms this emulator's awsQuery serializer writes: the first member of a list carries
    /// the service's namespace attribute, the rest are bare, a member missing one half of the
    /// pair contributes nothing rather than stealing the next member's value, and a value the
    /// serializer escaped comes back decoded.
    #[test]
    fn the_list_reader_pairs_within_each_member() {
        let answer = concat!(
            "<StackResources>",
            "<member xmlns=\"http://cloudformation.amazonaws.com/doc/2010-05-15/\">",
            "<LogicalResourceId>Record</LogicalResourceId>",
            "<PhysicalResourceId>a.example.</PhysicalResourceId></member>",
            "<member><LogicalResourceId>Zone</LogicalResourceId>",
            "<PhysicalResourceId>Z0123456789ABCD</PhysicalResourceId></member>",
            "<member><LogicalResourceId>Pending</LogicalResourceId></member>",
            "</StackResources>"
        );
        let read = physical_ids_of(answer);
        let expected: BTreeMap<String, String> =
            [("Record", "a.example."), ("Zone", "Z0123456789ABCD")]
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
        assert_eq!(
            read, expected,
            "case \"a namespaced member, a bare one and one with no physical id\": expected \
             {expected:?}, got {read:?}"
        );

        let outputs = concat!(
            "<Outputs><member xmlns=\"http://cloudformation.amazonaws.com/doc/2010-05-15/\">",
            "<OutputKey>Zone</OutputKey><OutputValue>Z0123456789ABCD</OutputValue></member>",
            "<member><OutputKey>Quoted</OutputValue></member>",
            "<member><OutputKey>Escaped</OutputKey>",
            "<OutputValue>a &amp; b</OutputValue></member></Outputs>"
        );
        let read = outputs_of(outputs);
        let expected: BTreeMap<String, String> =
            [("Zone", "Z0123456789ABCD"), ("Escaped", "a & b")]
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
        assert_eq!(
            read, expected,
            "case \"an output list with a malformed member\": expected {expected:?}, got \
             {read:?}"
        );
    }
}

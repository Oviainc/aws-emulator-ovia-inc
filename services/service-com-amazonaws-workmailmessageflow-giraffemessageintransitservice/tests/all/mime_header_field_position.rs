// SPDX-License-Identifier: CC0-1.0

//! Which header field a line of a message carries does not depend on WHERE in the
//! header block that line sits.
//!
//! `PutRawMessageContent` decides several criteria about the header block — the
//! content must have a header block at all, it must carry the required fields, a
//! field that may appear once may not appear twice, and an immutable field's value
//! must be unchanged — and every one of them asks the same question of a line:
//! which field, if any, does it carry? RFC 5322 §3.6.8 makes a field name one or
//! more printable US-ASCII octets before the colon, the obsolete field syntax of
//! §4.5.8 admits ASCII whitespace between the name and that colon, and §2.2.3
//! makes a line that opens with whitespace a continuation of the field above it.
//! So a line that carries `Date` carries it on the first line of the block as much
//! as on the last, and a line that carries no field carries none wherever it
//! stands.
//!
//! Each test is a TABLE whose axes are the FORM of the line — how its name is
//! padded, or in what way it is not a field name at all — and the POSITION of that
//! line in the header block. Every row goes over the wire through the harness in
//! [`crate::raw_message_content`] and reuses the seed-update-read helper in
//! [`crate::mime_header_criteria`].

use crate::mime_header_criteria::{assert_rejected, update};
use crate::mime_header_octets::shown;
use crate::raw_message_content::DATE;

/// A message whose header block is `lines` — one header line each, without their
/// CRLF — with `body` after the empty line that ends the block.
fn block(lines: &[Vec<u8>], body: &[u8]) -> Vec<u8> {
    let mut content: Vec<u8> = Vec::new();
    for line in lines {
        content.extend_from_slice(line);
        content.extend_from_slice(b"\r\n");
    }
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(body);
    content
}

/// One header line: `name`, then `padding` between the name and the colon, then
/// the value.
fn line(name: &[u8], padding: &[u8], value: &[u8]) -> Vec<u8> {
    let mut line: Vec<u8> = name.to_vec();
    line.extend_from_slice(padding);
    line.extend_from_slice(b": ");
    line.extend_from_slice(value);
    line
}

/// The fields of the message an update installs: the two every message is required
/// to carry, plus a subject the update rewrites.
fn update_fields() -> Vec<(&'static [u8], Vec<u8>)> {
    vec![
        (b"Date", DATE.as_bytes().to_vec()),
        (b"From", b"sender@example.com".to_vec()),
        (b"Subject", b"rewritten by the message flow".to_vec()),
    ]
}

/// The message in transit every row of the padding table updates: it carries both
/// required fields and no immutable header, so each row turns only on the line form
/// and its position.
fn in_transit() -> Vec<u8> {
    let lines: Vec<Vec<u8>> = vec![
        line(b"Date", b"", DATE.as_bytes()),
        line(b"From", b"", b"sender@example.com"),
        line(b"Subject", b"", b"original"),
    ];
    block(&lines, b"the original body\r\n")
}

/// The update's header block with `field`'s name padded by `padding` and its line
/// moved to `position` in the block.
fn padded_at(field: &[u8], padding: &[u8], position: usize) -> Vec<u8> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut padded: Option<Vec<u8>> = None;
    for (name, value) in update_fields() {
        let rendered = line(name, if name == field { padding } else { b"" }, &value);
        if name == field {
            padded = Some(rendered);
        } else {
            lines.push(rendered);
        }
    }
    let padded = padded.unwrap_or_else(|| panic!("the block carries a {} field", shown(field)));
    lines.insert(position.min(lines.len()), padded);
    block(&lines, b"a rewritten body\r\n")
}

/// The update's header block with `form` standing where the `Date` line would be,
/// at `position` in the block.
fn instead_of_date(form: &[u8], position: usize) -> Vec<u8> {
    let mut lines: Vec<Vec<u8>> = vec![form.to_vec()];
    for (name, value) in update_fields() {
        if name == b"Date" {
            continue;
        }
        lines.push(line(name, b"", &value));
    }
    let form = lines.remove(0);
    lines.insert(position.min(lines.len()), form);
    block(&lines, b"a rewritten body\r\n")
}

/// Every way ASCII whitespace may pad a field name before its colon. The space and
/// the horizontal tab are the whole of RFC 5322 §3.2.2 `WSP`, and §4.5.8 admits a
/// run of it between the name and the colon; the unpadded row is the control.
const PADDINGS: &[(&str, &[u8])] = &[
    ("no padding at all", b""),
    ("one horizontal tab", b"\t"),
    ("one space", b" "),
    ("a tab then a space", b"\t "),
    ("three spaces and two tabs", b"   \t\t"),
];

/// Lines that carry no header field at all: a line with no colon on it, a colon
/// with no name before it, a name of whitespace only (which §2.2.3 makes a
/// continuation line), and a name carrying an octet RFC 5322 §3.6.8 does not admit
/// in one — a byte outside printable US-ASCII, or a space inside the name.
const NOT_FIELDS: &[(&str, &[u8])] = &[
    ("no colon anywhere on the line", b"Date is a field this line forgets"),
    ("a colon with no name before it", b": a value with no name"),
    ("only whitespace before the colon", b"\t: a value with no name"),
    ("a name carrying an octet outside printable US-ASCII", b"Dat\xffe: Tue"),
    ("a name carrying a space inside it", b"Date Received: Tue"),
];

/// A field name padded with ASCII whitespace before its colon is that field at
/// every position in the header block, the first line included: the update is
/// accepted and installed. The axes are the padding, which field carries it, and
/// where in the block that field's line sits — each padding and each field is
/// driven at all three positions, since the position cycles with both.
#[tokio::test]
async fn a_whitespace_padded_field_name_names_its_field_at_every_position() {
    let fields: &[&[u8]] = &[b"Date", b"From", b"Subject"];
    let positions = 3usize;

    for (padding_index, (padding_case, padding)) in PADDINGS.iter().enumerate() {
        for (field_index, field) in fields.iter().enumerate() {
            let position = (padding_index + field_index) % positions;
            let case = format!(
                "{padding_case} on the {} field, {} in the block",
                String::from_utf8_lossy(field),
                match position {
                    0 => "first",
                    1 => "second",
                    _ => "last",
                }
            );
            let content = padded_at(field, padding, position);
            let (reply, after) = update(&in_transit(), &content).await;
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected HTTP 200, got {} ({})",
                reply.status,
                reply.summary()
            );
            assert_eq!(
                after, content,
                "case {case:?}: expected the accepted content in transit, got {}",
                shown(&after[..after.len().min(120)])
            );
        }
    }
}

/// The padding sweep again with the padded line ALWAYS first, so the position is
/// not what carries the row: every padding, on every field, opening the block.
#[tokio::test]
async fn a_padded_field_name_on_the_first_line_is_that_field() {
    let fields: &[&[u8]] = &[b"Date", b"From", b"Subject"];

    for (padding_case, padding) in PADDINGS {
        for field in fields {
            let case = format!(
                "{padding_case} on the {} field, opening the block",
                String::from_utf8_lossy(field)
            );
            let content = padded_at(field, padding, 0);
            let (reply, after) = update(&in_transit(), &content).await;
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected HTTP 200, got {} ({})",
                reply.status,
                reply.summary()
            );
            assert_eq!(
                after, content,
                "case {case:?}: expected the accepted content in transit, got {}",
                shown(&after[..after.len().min(120)])
            );
        }
    }
}

/// A line that carries no header field supplies none wherever it stands: with it
/// where the `Date` line would be, the message carries no `Date` field and the
/// update is refused, first in the block or after another field. The axes are the
/// line form and its position.
#[tokio::test]
async fn a_line_that_is_no_header_field_supplies_none_at_any_position() {
    for (form_case, form) in NOT_FIELDS {
        for position in 0..3usize {
            let case = format!(
                "{form_case}, {} in the block",
                match position {
                    0 => "first",
                    1 => "second",
                    _ => "last",
                }
            );
            let original = in_transit();
            let content = instead_of_date(form, position);
            let (reply, after) = update(&original, &content).await;
            assert_rejected(&case, &reply);
            if position > 0 {
                // Below the first line the content still HAS a header block, so the
                // refusal is the one for the field this line does not carry.
                let reported = reply.error_message().to_ascii_lowercase();
                assert!(
                    reported.contains("date"),
                    "case {case:?}: expected the rejection to name the missing date field, \
                     got {:?}",
                    reply.error_message()
                );
            }
            assert_eq!(
                after, original,
                "case {case:?}: the rejected update changed the content in transit to {}",
                shown(&after[..after.len().min(120)])
            );
        }
    }
}

/// The same forms once more, this time in a block that carries both required
/// fields: a line that is no header field is simply not a field, so it refuses the
/// content only where it would be the start of the header block.
#[tokio::test]
async fn a_line_that_is_no_header_field_only_refuses_content_it_opens() {
    for (form_case, form) in NOT_FIELDS {
        let original = in_transit();

        let mut lines: Vec<Vec<u8>> = update_fields()
            .into_iter()
            .map(|(name, value)| line(name, b"", &value))
            .collect();
        lines.push(form.to_vec());
        let below = block(&lines, b"a rewritten body\r\n");
        let case = format!("{form_case}, below a complete header block");
        let (reply, after) = update(&original, &below).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            after, below,
            "case {case:?}: expected the accepted content in transit, got {}",
            shown(&after[..after.len().min(120)])
        );

        let mut lines: Vec<Vec<u8>> = vec![form.to_vec()];
        lines.extend(
            update_fields()
                .into_iter()
                .map(|(name, value)| line(name, b"", &value)),
        );
        let opening = block(&lines, b"a rewritten body\r\n");
        let case = format!("{form_case}, opening the content");
        let (reply, after) = update(&original, &opening).await;
        assert_rejected(&case, &reply);
        assert_eq!(
            after, original,
            "case {case:?}: the rejected update changed the content in transit to {}",
            shown(&after[..after.len().min(120)])
        );
    }
}

/// An immutable header is compared as the field its padded name says it is: an
/// update whose `Message-ID` name is padded carries the same value as the message
/// in transit and is accepted, and one whose padded `Message-ID` carries a
/// different value is refused — at every position, the first line included.
#[tokio::test]
async fn a_padded_immutable_field_name_is_compared_as_that_field() {
    const STORED_ID: &[u8] = b"<00000001@example.com>";

    /// The message in transit for this table: it carries the identifier the mail
    /// system set before the message reached the flow.
    fn original() -> Vec<u8> {
        let lines: Vec<Vec<u8>> = vec![
            line(b"Message-ID", b"", STORED_ID),
            line(b"Date", b"", DATE.as_bytes()),
            line(b"From", b"", b"sender@example.com"),
            line(b"Subject", b"", b"original"),
        ];
        block(&lines, b"the original body\r\n")
    }

    /// The update's block with a `Message-ID` of `value`, its name padded by
    /// `padding`, at `position`.
    fn with_message_id(padding: &[u8], value: &[u8], position: usize) -> Vec<u8> {
        let mut lines: Vec<Vec<u8>> = update_fields()
            .into_iter()
            .map(|(name, field_value)| line(name, b"", &field_value))
            .collect();
        lines.insert(
            position.min(lines.len()),
            line(b"Message-ID", padding, value),
        );
        block(&lines, b"a rewritten body\r\n")
    }

    for (padding_index, (padding_case, padding)) in PADDINGS.iter().enumerate() {
        let position = padding_index % 4;

        let case = format!("{padding_case}, the stored identifier, at position {position}");
        let content = with_message_id(padding, STORED_ID, position);
        let (reply, after) = update(&original(), &content).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            after, content,
            "case {case:?}: expected the accepted content in transit, got {}",
            shown(&after[..after.len().min(120)])
        );

        let case = format!("{padding_case}, a different identifier, at position {position}");
        let content = with_message_id(padding, b"<99999999@example.com>", position);
        let (reply, after) = update(&original(), &content).await;
        assert_rejected(&case, &reply);
        let reported = reply.error_message().to_ascii_lowercase();
        assert!(
            reported.contains("message-id"),
            "case {case:?}: expected the rejection to name the message-id header, got {:?}",
            reply.error_message()
        );
        assert_eq!(
            after,
            original(),
            "case {case:?}: the rejected update changed the content in transit to {}",
            shown(&after[..after.len().min(120)])
        );
    }
}

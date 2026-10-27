// SPDX-License-Identifier: CC0-1.0

//! What `PutRawMessageContent` will read of a message's HEADER BLOCK.
//!
//! The block is the content up to the first empty line, so a caller decides how
//! long it is and how many lines stand in it, and the handler takes it apart into
//! fields to decide the criteria the `RawMessageContent` documentation states. Two
//! ceilings bound that work — the octets of the block and the number of header
//! lines in it — and content beyond either is refused as unusable with
//! `MessageRejected`, the way content beyond the octet ceiling on the whole
//! message already is. Neither is a modelled limit: both sit far above any header
//! block a mail system accepts, so the rows inside them are what a real message
//! looks like and the rows past them are what no message does.
//!
//! Each test is a TABLE whose axis is the size of the header block against the
//! ceiling: inside it, exactly at it, one past it and far past it. Every row goes
//! over the wire through the harness in [`crate::raw_message_content`] and reuses
//! the seed-update-read helper in [`crate::mime_header_criteria`].

use crate::mime_header_criteria::{assert_rejected, update};
use crate::raw_message_content::{mime, DATE};

/// The longest a line of a message may be, including its CRLF: the bound the
/// model's `RawMessageContent` documentation publishes ("Per RFC 5321, the maximum
/// length of each line of text, including the <CRLF>, must not exceed 1,000
/// characters"), which every row here stays inside so no row is refused for its
/// lines rather than for its block.
const MAX_LINE_OCTETS: usize = 1000;

/// The ceilings the handler reads a header block up to. Mirrored here rather than
/// published by the model: they are the emulator's own guard on the work one
/// request can ask for, so a test is what pins them.
const MAX_HEADER_BLOCK_OCTETS: usize = 1024 * 1024;

/// How many header lines the handler will read out of one block. See
/// [`MAX_HEADER_BLOCK_OCTETS`].
const MAX_HEADER_FIELDS: usize = 10_000;

/// The shortest padding line this builder emits: `X-Pad: ` plus one octet of
/// padding plus the CRLF.
const MIN_PAD_OCTETS: usize = "X-Pad: ".len() + 1 + "\r\n".len();

/// A padding header line of exactly `octets` octets, counting its CRLF.
fn pad_line(octets: usize) -> Vec<u8> {
    assert!(
        (MIN_PAD_OCTETS..=MAX_LINE_OCTETS).contains(&octets),
        "a padding line of {octets} octets is not one this builder can emit"
    );
    let mut line = b"X-Pad: ".to_vec();
    line.resize(octets - "\r\n".len(), b'a');
    line.extend_from_slice(b"\r\n");
    line
}

/// A message whose header block is exactly `octets` octets long — counting every
/// header line with its CRLF, which is what the handler's ceiling counts — and
/// which carries both required header fields, with every line inside
/// [`MAX_LINE_OCTETS`].
fn block_of_octets(octets: usize) -> Vec<u8> {
    let required = format!("Date: {DATE}\r\nFrom: a@example.com\r\n").into_bytes();
    assert!(
        octets >= required.len() + MIN_PAD_OCTETS,
        "a header block of {octets} octets cannot carry the required fields"
    );
    let mut content = required.clone();
    let mut remaining = octets - required.len();
    while remaining > 0 {
        // Take a full line while the remainder after it can still be a line of its
        // own; otherwise leave exactly the shortest line behind.
        let take = if remaining <= MAX_LINE_OCTETS {
            remaining
        } else if remaining - MAX_LINE_OCTETS < MIN_PAD_OCTETS {
            remaining - MIN_PAD_OCTETS
        } else {
            MAX_LINE_OCTETS
        };
        content.extend_from_slice(&pad_line(take));
        remaining -= take;
    }
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(b"body\r\n");
    content
}

/// A message whose header block is exactly `lines` header lines long, both
/// required fields among them.
fn block_of_lines(lines: usize) -> Vec<u8> {
    assert!(lines >= 2, "a header block of {lines} lines carries no message");
    let mut content = format!("Date: {DATE}\r\nFrom: a@example.com\r\n").into_bytes();
    for _ in 2..lines {
        content.extend_from_slice(&pad_line(MIN_PAD_OCTETS));
    }
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(b"body\r\n");
    content
}

/// How many header lines a message's block carries, counted the way the handler
/// does: every line up to the first empty one.
fn header_lines(content: &[u8]) -> usize {
    content
        .split(|byte| *byte == b'\n')
        .take_while(|line| !line.strip_suffix(b"\r").unwrap_or(line).is_empty())
        .count()
}

/// How many octets a message's header block carries, counted the way the handler
/// does: every header line with the line feed that ended it.
fn header_octets(content: &[u8]) -> usize {
    content
        .split(|byte| *byte == b'\n')
        .take_while(|line| !line.strip_suffix(b"\r").unwrap_or(line).is_empty())
        .map(|line| line.len() + 1)
        .sum()
}

/// A header block inside the ceilings is read and the update installed; one past
/// either ceiling is refused as unusable content and installs nothing. The axis is
/// the size of the block against the ceiling, on both of the ceiling's halves.
#[tokio::test]
async fn a_header_block_is_read_up_to_the_ceiling_and_refused_past_it() {
    let accepted: Vec<(&str, Vec<u8>)> = vec![
        (
            "an ordinary header block",
            block_of_octets(4 * MIN_PAD_OCTETS + 128),
        ),
        (
            "a block one octet inside the octet ceiling",
            block_of_octets(MAX_HEADER_BLOCK_OCTETS - 1),
        ),
        (
            "a block exactly at the octet ceiling",
            block_of_octets(MAX_HEADER_BLOCK_OCTETS),
        ),
        (
            "a block one line inside the line ceiling",
            block_of_lines(MAX_HEADER_FIELDS - 1),
        ),
        (
            "a block exactly at the line ceiling",
            block_of_lines(MAX_HEADER_FIELDS),
        ),
    ];

    for (case, content) in accepted {
        let (reply, after) = update(&mime("original", "body"), &content).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200 for a block of {} octets in {} lines, got {} ({})",
            header_octets(&content),
            header_lines(&content),
            reply.status,
            reply.summary()
        );
        assert_eq!(
            after.len(),
            content.len(),
            "case {case:?}: expected the accepted content in transit, got {} octets",
            after.len()
        );
        assert!(
            after == content,
            "case {case:?}: the content in transit is not the content the update carried"
        );
    }

    let refused: Vec<(&str, Vec<u8>)> = vec![
        (
            "a block one octet past the octet ceiling",
            block_of_octets(MAX_HEADER_BLOCK_OCTETS + 1),
        ),
        (
            "a block far past the octet ceiling",
            block_of_octets(MAX_HEADER_BLOCK_OCTETS * 4),
        ),
        (
            "a block one line past the line ceiling",
            block_of_lines(MAX_HEADER_FIELDS + 1),
        ),
        (
            "a block far past the line ceiling",
            block_of_lines(MAX_HEADER_FIELDS * 4),
        ),
    ];

    for (case, content) in refused {
        let original = mime("original", "body");
        let (reply, after) = update(&original, &content).await;
        assert_rejected(case, &reply);
        let reported = reply.error_message().to_ascii_lowercase();
        assert!(
            reported.contains("header block"),
            "case {case:?}: expected the rejection to say the header block is past what is \
             read, got {:?}",
            reply.error_message()
        );
        assert_eq!(
            after, original,
            "case {case:?}: the rejected update changed the content in transit, now {} octets",
            after.len()
        );
    }
}

/// A block past the ceiling is refused for its SIZE and not for its lines: the
/// same rows carry both required header fields and no repeated unique header, so
/// none of the other criteria explains the refusal, and a block of the same shape
/// inside the ceiling is accepted.
#[tokio::test]
async fn a_block_past_the_ceiling_is_refused_though_its_fields_are_in_order() {
    let inside = block_of_lines(MAX_HEADER_FIELDS);
    let past = block_of_lines(MAX_HEADER_FIELDS + 1);
    assert_eq!(
        header_lines(&inside),
        MAX_HEADER_FIELDS,
        "the builder emits the header lines it is asked for"
    );
    assert_eq!(
        header_lines(&past),
        MAX_HEADER_FIELDS + 1,
        "the builder emits the header lines it is asked for"
    );

    let original = mime("original", "body");
    let (reply, after) = update(&original, &inside).await;
    assert_eq!(
        reply.status,
        200,
        "a block of {} lines is inside the ceiling, got {} ({})",
        header_lines(&inside),
        reply.status,
        reply.summary()
    );
    assert!(
        after == inside,
        "a block inside the ceiling is installed; the content in transit is {} octets",
        after.len()
    );

    let (reply, after) = update(&original, &past).await;
    assert_rejected("one line past the ceiling", &reply);
    assert_eq!(
        after, original,
        "the refused update changed the content in transit, now {} octets",
        after.len()
    );
}

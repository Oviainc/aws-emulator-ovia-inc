// SPDX-License-Identifier: CC0-1.0

//! The ONE walk `PutRawMessageContent` reads a message's header block by, driven
//! through the wire in both of the roles that walk serves.
//!
//! The handler bounds the header block it reads with two ceilings — the octets of
//! the block and the number of header LINES in it — and the same walk that decides
//! them hands back the lines the block carries, which the handler takes apart into
//! the fields the `RawMessageContent` documentation's criteria are decided over
//! ("The message must contain all the required header fields", "certain unique
//! headers can only appear once", "The value of immutable headers must remain
//! unchanged": `smithy_query(query="doc", shape="RawMessageContent")`).
//!
//! One walk means the two roles agree on what the block is, and that is what these
//! tables pin:
//!
//! * the ceiling counts every line standing before the empty line that ends the
//!   block — a folded continuation and a line that carries no field among them —
//!   and not the number of fields those lines parse into, so blocks with the same
//!   line count and wildly different field counts are accepted and refused
//!   together ([`the_block_ceiling_counts_every_line_of_the_block`]);
//! * the empty line that ends the block belongs to neither ceiling, so a block
//!   that reaches a ceiling exactly still has its LAST header line read into a
//!   field ([`the_last_line_of_a_block_at_the_ceiling_is_read_into_a_field`]);
//! * the block of the message in transit is read by the same walk as the block of
//!   the update, so a value folded on one side and not the other is the one value
//!   it unfolds to ([`a_folded_header_is_the_value_it_unfolds_to`]).
//!
//! Every row goes over the wire through the harness in
//! [`crate::raw_message_content`] and the seed-update-read helper in
//! [`crate::mime_header_criteria`].

use crate::mime_header_criteria::{assert_rejected, update};
use crate::raw_message_content::{mime, DATE};

/// The longest a line of a message may be, including its CRLF — the bound the
/// model's `RawMessageContent` documentation publishes ("Per RFC 5321, the maximum
/// length of each line of text, including the <CRLF>, must not exceed 1,000
/// characters"), which every row here stays inside so no row is refused for its
/// lines rather than for its block.
const MAX_LINE_OCTETS: usize = 1000;

/// The ceilings the handler reads a header block up to. Mirrored here rather than
/// published by the model, and rather than imported: they are the emulator's own
/// guard on the work one request can ask for, so a test is what pins them.
const MAX_HEADER_BLOCK_OCTETS: usize = 1024 * 1024;

/// How many header lines the handler will read out of one block. See
/// [`MAX_HEADER_BLOCK_OCTETS`].
const MAX_HEADER_FIELDS: usize = 10_000;

/// The shortest padding line [`pad_line`] emits: `X-Pad: ` plus one octet of
/// padding plus the CRLF.
const MIN_PAD_OCTETS: usize = "X-Pad: ".len() + 1 + "\r\n".len();

/// The `Date` header line every block here opens with.
fn date_line() -> Vec<u8> {
    format!("Date: {DATE}\r\n").into_bytes()
}

/// The `From` header line every block here carries.
fn from_line() -> Vec<u8> {
    b"From: a@example.com\r\n".to_vec()
}

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

/// What the lines of a block that are neither of its required fields are made of:
/// the axis the ceiling must be INSENSITIVE to, because each kind contributes a
/// different number of fields per line.
#[derive(Clone, Copy, Debug)]
enum Filler {
    /// A header line carrying a field of its own: one line, one field.
    Field,
    /// A folded continuation (RFC 5322 §2.2.3): one line, no field of its own —
    /// it continues the value of the field above it.
    Fold,
    /// A line with no colon on it, so it carries no field at all: one line, no
    /// field.
    NoField,
}

impl Filler {
    /// One line of this kind, with its CRLF.
    fn line(self) -> Vec<u8> {
        match self {
            Filler::Field => pad_line(MIN_PAD_OCTETS),
            Filler::Fold => b" folded onto the field above\r\n".to_vec(),
            Filler::NoField => b"X-Pad no colon on this line\r\n".to_vec(),
        }
    }
}

/// A message whose header block is exactly `lines` header lines long: the two
/// required fields first, then `lines - 2` lines of `filler`, then the empty line
/// that ends the block.
fn block_of_lines(lines: usize, filler: Filler) -> Vec<u8> {
    assert!(
        lines >= 2,
        "a header block of {lines} lines cannot carry the required fields"
    );
    let mut content = date_line();
    content.extend_from_slice(&from_line());
    for _ in 2..lines {
        content.extend_from_slice(&filler.line());
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
/// does: every header line with the line feed that ended it, and never the empty
/// line that ends the block.
fn header_octets(content: &[u8]) -> usize {
    content
        .split(|byte| *byte == b'\n')
        .take_while(|line| !line.strip_suffix(b"\r").unwrap_or(line).is_empty())
        .map(|line| line.len() + 1)
        .sum()
}

/// A message whose header block is exactly `octets` octets long and whose LAST
/// header line is the required `From` field, with every line inside
/// [`MAX_LINE_OCTETS`].
fn block_of_octets_ending_in_a_required_field(octets: usize) -> Vec<u8> {
    let date = date_line();
    let from = from_line();
    assert!(
        octets >= date.len() + from.len() + MIN_PAD_OCTETS,
        "a header block of {octets} octets cannot carry the required fields"
    );
    let mut content = date.clone();
    let mut remaining = octets - date.len() - from.len();
    while remaining > 0 {
        // Take a full line while the remainder after it can still be a line of
        // its own; otherwise leave exactly the shortest line behind.
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
    content.extend_from_slice(&from);
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(b"body\r\n");
    content
}

/// A message whose header block is exactly `lines` header lines long and whose
/// LAST header line is the required `From` field.
fn block_of_lines_ending_in_a_required_field(lines: usize) -> Vec<u8> {
    assert!(
        lines >= 2,
        "a header block of {lines} lines cannot carry the required fields"
    );
    let mut content = date_line();
    for _ in 2..lines {
        content.extend_from_slice(&pad_line(MIN_PAD_OCTETS));
    }
    content.extend_from_slice(&from_line());
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(b"body\r\n");
    content
}

/// Assert `reply` is the refusal a block past a ceiling earns: it says the HEADER
/// BLOCK is past what is read, and not that a field is missing — the block was
/// refused for its size, not for what the truncated part of it would have carried.
fn assert_refused_for_its_block(case: &str, reply: &crate::raw_message_content::Reply) {
    assert_rejected(case, reply);
    let reported = reply.error_message().to_ascii_lowercase();
    assert!(
        reported.contains("header block"),
        "case {case:?}: expected the refusal to say the header block is past what is read, got \
         {:?}",
        reply.error_message()
    );
    assert!(
        !reported.contains("missing"),
        "case {case:?}: expected the refusal to be about the block's size and not about a \
         missing field, got {:?}",
        reply.error_message()
    );
}

/// The line ceiling counts the block's LINES, whatever those lines parse into: a
/// block of the same line count is accepted or refused the same way whether its
/// lines each carry a field, fold onto the field above, or carry no field at all.
/// The axis is the kind of line the block is filled with, crossed with the block's
/// line count against the ceiling.
#[tokio::test]
async fn the_block_ceiling_counts_every_line_of_the_block() {
    let fillers = [Filler::Field, Filler::Fold, Filler::NoField];

    for filler in fillers {
        let content = block_of_lines(MAX_HEADER_FIELDS, filler);
        let case = format!("a block of {MAX_HEADER_FIELDS} lines filled with {filler:?} lines");
        assert_eq!(
            header_lines(&content),
            MAX_HEADER_FIELDS,
            "case {case:?}: the builder emits the header lines it is asked for, got {}",
            header_lines(&content)
        );
        let (reply, after) = update(&mime("original", "body"), &content).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200 for a block of {} lines in {} octets, got {} ({})",
            header_lines(&content),
            header_octets(&content),
            reply.status,
            reply.summary()
        );
        assert!(
            after == content,
            "case {case:?}: the content in transit is not the content the update carried; it is \
             {} octets",
            after.len()
        );
    }

    for filler in fillers {
        let content = block_of_lines(MAX_HEADER_FIELDS + 1, filler);
        let case = format!(
            "a block of {} lines filled with {filler:?} lines",
            MAX_HEADER_FIELDS + 1
        );
        assert_eq!(
            header_lines(&content),
            MAX_HEADER_FIELDS + 1,
            "case {case:?}: the builder emits the header lines it is asked for, got {}",
            header_lines(&content)
        );
        let original = mime("original", "body");
        let (reply, after) = update(&original, &content).await;
        assert_refused_for_its_block(&case, &reply);
        assert_eq!(
            after, original,
            "case {case:?}: the refused update changed the content in transit, now {} octets",
            after.len()
        );
    }
}

/// A block that reaches a ceiling exactly is read whole, down to its last header
/// line: every row's last header line is a REQUIRED field, so a walk that stopped
/// one line early, or charged the empty line that ends the block to a ceiling,
/// would refuse the row for a missing field instead of installing it. The axis is
/// the block's size against each ceiling: inside it, exactly at it, and one past
/// it.
#[tokio::test]
async fn the_last_line_of_a_block_at_the_ceiling_is_read_into_a_field() {
    let accepted: Vec<(String, Vec<u8>)> = vec![
        (
            "a block one octet inside the octet ceiling".to_string(),
            block_of_octets_ending_in_a_required_field(MAX_HEADER_BLOCK_OCTETS - 1),
        ),
        (
            "a block exactly at the octet ceiling".to_string(),
            block_of_octets_ending_in_a_required_field(MAX_HEADER_BLOCK_OCTETS),
        ),
        (
            "a block one line inside the line ceiling".to_string(),
            block_of_lines_ending_in_a_required_field(MAX_HEADER_FIELDS - 1),
        ),
        (
            "a block exactly at the line ceiling".to_string(),
            block_of_lines_ending_in_a_required_field(MAX_HEADER_FIELDS),
        ),
    ];

    for (case, content) in accepted {
        let (reply, after) = update(&mime("original", "body"), &content).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200 for a block of {} octets in {} lines whose last \
             header line is a required field, got {} ({})",
            header_octets(&content),
            header_lines(&content),
            reply.status,
            reply.summary()
        );
        assert!(
            after == content,
            "case {case:?}: the content in transit is not the content the update carried; it is \
             {} octets",
            after.len()
        );
    }

    let refused: Vec<(String, Vec<u8>)> = vec![
        (
            "a block one octet past the octet ceiling".to_string(),
            block_of_octets_ending_in_a_required_field(MAX_HEADER_BLOCK_OCTETS + 1),
        ),
        (
            "a block one line past the line ceiling".to_string(),
            block_of_lines_ending_in_a_required_field(MAX_HEADER_FIELDS + 1),
        ),
    ];

    for (case, content) in refused {
        let original = mime("original", "body");
        let (reply, after) = update(&original, &content).await;
        assert_refused_for_its_block(&case, &reply);
        assert_eq!(
            after, original,
            "case {case:?}: the refused update changed the content in transit, now {} octets",
            after.len()
        );
    }
}

/// The block of the message in transit and the block of the update are read by the
/// same walk, so a header's value is what it unfolds to on either side: an update
/// that carries an immutable header folded where the message in transit has it on
/// one line — or the other way round — changes nothing and is accepted, while one
/// that rewrites what the continuation says is refused. The axis is which side
/// folds the value, and whether the unfolded values agree.
#[tokio::test]
async fn a_folded_header_is_the_value_it_unfolds_to() {
    /// The same message with its `Received` trace written on one line.
    fn unfolded(trace: &str) -> Vec<u8> {
        format!(
            "Received: {trace}\r\nMessage-ID: <00000001@example.com>\r\nDate: {DATE}\r\n\
             From: sender@example.com\r\n\r\nbody\r\n"
        )
        .into_bytes()
    }

    /// The same message with its `Received` trace folded after `head`, the
    /// continuation carrying `tail` — which RFC 5322 §2.2.3 unfolds back to
    /// `head` + `tail`.
    fn folded(head: &str, tail: &str) -> Vec<u8> {
        format!(
            "Received: {head}\r\n{tail}\r\nMessage-ID: <00000001@example.com>\r\nDate: {DATE}\r\n\
             From: sender@example.com\r\n\r\nbody\r\n"
        )
        .into_bytes()
    }

    let accepted: Vec<(&str, Vec<u8>, Vec<u8>)> = vec![
        (
            "the update unfolds a trace the message in transit carries folded",
            folded("by relay-two", " with SMTP"),
            unfolded("by relay-two with SMTP"),
        ),
        (
            "the update folds a trace the message in transit carries on one line",
            unfolded("by relay-two with SMTP"),
            folded("by relay-two", " with SMTP"),
        ),
        (
            "both sides fold the trace at the same point",
            folded("by relay-two", "\twith SMTP"),
            folded("by relay-two", "\twith SMTP"),
        ),
    ];

    for (case, original, content) in accepted {
        let (reply, after) = update(&original, &content).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200 for an update that folds the same value, got {} \
             ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            after, content,
            "case {case:?}: expected the accepted content in transit, got {:?}",
            String::from_utf8_lossy(&after[..after.len().min(120)])
        );
    }

    let refused: Vec<(&str, Vec<u8>, Vec<u8>)> = vec![
        (
            "the update rewrites what the continuation of the trace says",
            folded("by relay-two", " with SMTP"),
            folded("by relay-two", " with ESMTP"),
        ),
        (
            "the update drops the continuation of the trace",
            folded("by relay-two", " with SMTP"),
            unfolded("by relay-two"),
        ),
    ];

    for (case, original, content) in refused {
        let (reply, after) = update(&original, &content).await;
        assert_rejected(case, &reply);
        assert_eq!(
            after, original,
            "case {case:?}: the refused update changed the content in transit to {:?}",
            String::from_utf8_lossy(&after[..after.len().min(120)])
        );
    }
}

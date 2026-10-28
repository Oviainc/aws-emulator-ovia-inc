// SPDX-License-Identifier: CC0-1.0

//! `SendRawEmail`, end to end over the awsQuery wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so each call
//! is a real request against the service: form-urlencoded body in, XML out, one
//! shared engine behind all of them. What is asserted is the RULE the operation
//! follows over a SET of raw messages, not one recorded request:
//!
//! * the sender is the `Source` parameter when the request names one and the
//!   addr-spec of the message's own `From` line when it does not, and a message
//!   with neither is refused;
//! * the recipients are the `Destinations` parameter when the request names any
//!   and otherwise every address on the message's `To:`, `CC:` and `BCC:` lines,
//!   whatever form those address lists take — a display name, a group, a folded
//!   line, several lines of the same name;
//! * an accepted message is LOGGED, so the account's own `SentLast24Hours`
//!   counts every recipient of it, and a refused one is not;
//! * the gates a send passes, and the modelled error each is refused with: the
//!   message's own structure and recipient set, the account's pause flag, the
//!   configuration set the send names, the sending identity, and the 24-hour
//!   quota.
//!
//! The ceilings the rows probe are the ones the service publishes
//! ([`MAX_RECIPIENTS`], [`MAX_MESSAGE_BYTES`], [`MAX_MULTIPART_CONTAINERS`]),
//! read from the crate rather than written again here, and each is driven at the
//! bound and one past it.

use crate::harness::{call, element, emulator, field, xml_text, Field};
use service_com_amazonaws_ses_simpleemailservice::state::{
    MAX_MESSAGE_BYTES, MAX_MULTIPART_CONTAINERS, MAX_RECIPIENTS,
};
use smithy_server_runtime::MultiServiceRouter;

/// The line ending RFC 5322 writes between header lines.
const CRLF: &str = "\r\n";

/// A raw message from its header lines and its body, separated by the blank line
/// SES requires between them.
fn message(headers: &[(&str, &str)], body: &str) -> String {
    let mut raw = String::new();
    for (name, value) in headers {
        raw.push_str(name);
        raw.push_str(": ");
        raw.push_str(value);
        raw.push_str(CRLF);
    }
    raw.push_str(CRLF);
    raw.push_str(body);
    raw
}

/// The common case: a message from `from` to `to`, with a subject and a body.
fn simple_message(from: &str, to: &str) -> String {
    message(
        &[("From", from), ("To", to), ("Subject", "Quarterly report")],
        "See attached.\r\n",
    )
}

/// `SendRawEmail` carrying `raw` as its `RawMessage`, plus any extra parameters
/// the case names.
///
/// `RawMessage$Data` is a blob, so the awsQuery wire carries it base64-encoded —
/// through the runtime's own encoder, the one the service's codec decodes with.
async fn send_raw(
    router: &MultiServiceRouter,
    raw: &str,
    extra: &[Field],
) -> (u16, String) {
    let mut params = vec![field(
        "RawMessage.Data",
        ::aws_smithy_types::base64::encode(raw),
    )];
    params.extend_from_slice(extra);
    call(router, "SendRawEmail", &params).await
}

/// `VerifyEmailIdentity`, which is what puts a sending identity in the store.
async fn verify_email(router: &MultiServiceRouter, address: &str) {
    let (status, body) = call(
        router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", address)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case verify {address:?}: expected 200, got {status} with {body}"
    );
}

/// `GetSendQuota`'s `SentLast24Hours`, which is DERIVED from the send log: it is
/// how a test reads back what an accepted send wrote.
async fn sent_last_24_hours(router: &MultiServiceRouter) -> f64 {
    let (status, body) = call(router, "GetSendQuota", &[]).await;
    assert_eq!(
        status, 200,
        "case GetSendQuota: expected 200, got {status} with {body}"
    );
    let text = element(&body, "SentLast24Hours")
        .unwrap_or_else(|| panic!("case GetSendQuota: expected SentLast24Hours, got {body}"));
    text.parse::<f64>()
        .unwrap_or_else(|_| panic!("case GetSendQuota: expected a number, got {text:?}"))
}

/// The `<Code>` of a refusal, and its `<Message>` with the XML escaping undone.
fn refusal(body: &str) -> (String, String) {
    (
        element(body, "Code").unwrap_or_default(),
        element(body, "Message")
            .map(|raw| xml_text(&raw))
            .unwrap_or_default(),
    )
}

/// `Destinations.member.N` for each address, which is how an address list
/// reaches an awsQuery request.
fn destinations(addresses: &[&str]) -> Vec<Field> {
    addresses
        .iter()
        .enumerate()
        .map(|(index, address)| field(format!("Destinations.member.{}", index + 1), *address))
        .collect()
}

/// One accepted-message row: the case, the raw message, and the recipients the
/// message addresses.
struct Accepted {
    case: &'static str,
    raw: String,
    recipients: usize,
}

#[tokio::test]
async fn a_raw_message_is_accepted_in_every_form_its_headers_can_take() {
    let router = emulator();
    verify_email(&router, "sender@example.com").await;

    // The rows differ along what the rule generalises over: how many recipient
    // lines the message carries, how many addresses sit on one line, the form
    // each address takes (bare, display-name, group), how the lines are folded,
    // which line endings they use, and whether the message is one part or a
    // multipart tree.
    let rows = vec![
        Accepted {
            case: "a plain single-part message",
            raw: simple_message("sender@example.com", "one@example.net"),
            recipients: 1,
        },
        Accepted {
            case: "two addresses on one To: line",
            raw: simple_message("sender@example.com", "one@example.net, two@example.net"),
            recipients: 2,
        },
        Accepted {
            case: "a To: line folded over two lines",
            raw: message(
                &[
                    ("From", "sender@example.com"),
                    ("To", "one@example.net,\r\n two@example.net"),
                ],
                "Folded.\r\n",
            ),
            recipients: 2,
        },
        Accepted {
            case: "display names around the addresses",
            raw: message(
                &[
                    ("From", "\"Ops Team\" <sender@example.com>"),
                    ("To", "Someone <one@example.net>"),
                    ("Cc", "\"Last, First\" <two@example.net>"),
                ],
                "Named.\r\n",
            ),
            recipients: 2,
        },
        Accepted {
            case: "a group address list",
            raw: message(
                &[
                    ("From", "sender@example.com"),
                    ("To", "Team: one@example.net, two@example.net;"),
                ],
                "Grouped.\r\n",
            ),
            recipients: 2,
        },
        Accepted {
            case: "To:, Cc: and Bcc: lines together",
            raw: message(
                &[
                    ("From", "sender@example.com"),
                    ("To", "one@example.net"),
                    ("Cc", "two@example.net"),
                    ("Bcc", "three@example.net, four@example.net"),
                ],
                "Everyone.\r\n",
            ),
            recipients: 4,
        },
        Accepted {
            case: "two To: lines of the same name",
            raw: message(
                &[
                    ("From", "sender@example.com"),
                    ("To", "one@example.net"),
                    ("To", "two@example.net"),
                ],
                "Twice.\r\n",
            ),
            recipients: 2,
        },
        Accepted {
            case: "bare line-feed line endings",
            raw: "From: sender@example.com\nTo: one@example.net\nSubject: Unix\n\nPlain.\n"
                .to_string(),
            recipients: 1,
        },
        Accepted {
            case: "a multipart MIME message with an attachment",
            raw: message(
                &[
                    ("From", "sender@example.com"),
                    ("To", "one@example.net"),
                    ("Subject", "With an attachment"),
                    ("MIME-Version", "1.0"),
                    ("Content-Type", "multipart/mixed; boundary=\"edge\""),
                ],
                "--edge\r\nContent-Type: text/plain\r\n\r\nThe text part.\r\n--edge\r\n\
                 Content-Type: application/octet-stream\r\n\
                 Content-Transfer-Encoding: base64\r\n\r\nAAEC\r\n--edge--\r\n",
            ),
            recipients: 1,
        },
        Accepted {
            case: "no Subject line at all",
            raw: message(
                &[
                    ("From", "sender@example.com"),
                    ("To", "one@example.net"),
                ],
                "Unsubjected.\r\n",
            ),
            recipients: 1,
        },
        Accepted {
            case: "an empty body behind the separator",
            raw: message(
                &[
                    ("From", "sender@example.com"),
                    ("To", "one@example.net"),
                ],
                "",
            ),
            recipients: 1,
        },
    ];

    let mut counted = 0.0;
    let mut ids: Vec<String> = Vec::new();
    for row in &rows {
        let case = row.case;
        let (status, body) = send_raw(&router, &row.raw, &[]).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let id = element(&body, "MessageId")
            .unwrap_or_else(|| panic!("case {case}: expected a MessageId, got {body}"));
        assert!(
            id.starts_with("0100") && id.ends_with("-000000"),
            "case {case}: expected an SES-shaped MessageId, got {id:?}"
        );
        assert!(
            !ids.contains(&id),
            "case {case}: expected a fresh MessageId, got the earlier {id:?} again"
        );
        ids.push(id);

        // The accepted message reached the send log, where every recipient of it
        // counts against the account's 24-hour quota.
        counted += row.recipients as f64;
        let sent = sent_last_24_hours(&router).await;
        assert_eq!(
            sent, counted,
            "case {case}: expected SentLast24Hours to count every recipient ({counted}), got \
             {sent}"
        );
    }
}

/// One sender-resolution row: what the request names, what the message's own
/// `From` line names, and the identity the gate must read.
struct Sender {
    case: &'static str,
    source: Option<&'static str>,
    from: Option<&'static str>,
    /// The identity the verification gate must have read, and `None` when the
    /// send must be accepted.
    refused_identity: Option<&'static str>,
}

#[tokio::test]
async fn the_sender_is_the_source_parameter_or_the_messages_own_from_line() {
    // One identity is verified; every row differs in whether the request, the
    // message, or both name it, so what the gate read is observable from the
    // refusal — which names the identity it checked.
    let rows = vec![
        Sender {
            case: "the Source names the verified identity and the message carries no From line",
            source: Some("sender@example.com"),
            from: None,
            refused_identity: None,
        },
        Sender {
            case: "the From line names the verified identity and the request names no Source",
            source: None,
            from: Some("sender@example.com"),
            refused_identity: None,
        },
        Sender {
            case: "the From line carries a display name around the verified identity",
            source: None,
            from: Some("\"Ops Team\" <sender@example.com>"),
            refused_identity: None,
        },
        Sender {
            case: "the Source carries a display name around the verified identity",
            source: Some("Ops <sender@example.com>"),
            from: None,
            refused_identity: None,
        },
        Sender {
            case: "both are named, and the Source is the one the gate reads",
            source: Some("stranger@example.org"),
            from: Some("sender@example.com"),
            refused_identity: Some("stranger@example.org"),
        },
        Sender {
            case: "both are named the other way round, and the Source is still the one read",
            source: Some("sender@example.com"),
            from: Some("stranger@example.org"),
            refused_identity: None,
        },
        Sender {
            case: "only the From line is named, and it is not verified",
            source: None,
            from: Some("stranger@example.org"),
            refused_identity: Some("stranger@example.org"),
        },
        Sender {
            case: "the first mailbox of a From line naming several",
            source: None,
            from: Some("stranger@example.org, sender@example.com"),
            refused_identity: Some("stranger@example.org"),
        },
    ];

    for row in &rows {
        let case = row.case;
        let router = emulator();
        verify_email(&router, "sender@example.com").await;

        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(from) = row.from {
            headers.push(("From", from));
        }
        headers.push(("To", "one@example.net"));
        let raw = message(&headers, "Who sent this?\r\n");
        let extra: Vec<Field> = row
            .source
            .map(|source| vec![field("Source", source)])
            .unwrap_or_default();

        let (status, body) = send_raw(&router, &raw, &extra).await;
        match row.refused_identity {
            None => assert_eq!(
                status, 200,
                "case {case}: expected the send to be accepted, got {status} with {body}"
            ),
            Some(identity) => {
                assert_eq!(
                    status, 400,
                    "case {case}: expected HTTP 400, got {status} with {body}"
                );
                let (code, stated) = refusal(&body);
                assert_eq!(
                    code, "MessageRejected",
                    "case {case}: expected the modelled MessageRejected, got {code:?} in {body}"
                );
                assert!(
                    stated.contains(identity),
                    "case {case}: expected the refusal to name {identity:?}, got {stated:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_message_with_neither_a_source_nor_a_from_line_is_refused() {
    let router = emulator();
    verify_email(&router, "sender@example.com").await;

    // "If you do not provide a value for this parameter, you must specify a
    // 'From' address in the raw text of the message": neither, and there is no
    // sender to check at all. The rows differ in what stands in for the From
    // line — nothing, an empty value, a value that names no mailbox.
    let rows: &[(&str, &[(&str, &str)])] = &[
        ("no From line at all", &[("To", "one@example.net")]),
        (
            "an empty From line",
            &[("From", ""), ("To", "one@example.net")],
        ),
        (
            "a From line carrying only a display name",
            &[("From", "\"Ops Team\""), ("To", "one@example.net")],
        ),
    ];
    for (case, headers) in rows {
        let raw = message(headers, "Nobody sent this.\r\n");
        let (status, body) = send_raw(&router, &raw, &[]).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, _) = refusal(&body);
        assert_eq!(
            code, "MessageRejected",
            "case {case}: expected the modelled MessageRejected, got {code:?} in {body}"
        );
        assert_eq!(
            sent_last_24_hours(&router).await,
            0.0,
            "case {case}: a refused send logs nothing"
        );
    }
}

#[tokio::test]
async fn the_destinations_parameter_is_the_recipient_set_when_the_request_names_any() {
    // "A list of destinations for the message, consisting of To:, CC:, and BCC:
    // addresses": the parameter is the destination list, so it is what the quota
    // counts — the message's own recipient lines are not counted beside it. The
    // rows differ in how many addresses each side names, so neither count can be
    // mistaken for the other.
    let rows: &[(&str, &[&str], &str, f64)] = &[
        ("one destination against three header addresses", &["only@example.net"], "a@example.net, b@example.net, c@example.net", 1.0),
        ("three destinations against one header address", &["a@example.net", "b@example.net", "c@example.net"], "only@example.net", 3.0),
        ("two destinations and no recipient line at all", &["a@example.net", "b@example.net"], "", 2.0),
    ];
    for (case, listed, header, expected) in rows {
        let router = emulator();
        verify_email(&router, "sender@example.com").await;
        let mut headers: Vec<(&str, &str)> = vec![("From", "sender@example.com")];
        if !header.is_empty() {
            headers.push(("To", header));
        }
        let raw = message(&headers, "Addressed.\r\n");
        let (status, body) = send_raw(&router, &raw, &destinations(listed)).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let sent = sent_last_24_hours(&router).await;
        assert_eq!(
            sent, *expected,
            "case {case}: expected the Destinations parameter's {expected} recipients to be the \
             ones counted, got {sent}"
        );
    }
}

/// `n` distinct recipient addresses, as one `To:` line's value.
fn recipient_line(n: usize) -> String {
    (0..n)
        .map(|index| format!("r{index}@example.net"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A message whose MIME tree nests `levels` `multipart` containers, one inside
/// the next, with a single `text/plain` leaf at the bottom.
///
/// `levels` is exactly the number of `multipart/` declarations the message
/// carries, which is what the handler counts.
fn nested_multipart_message(levels: usize) -> String {
    let mut inner = "Content-Type: text/plain\r\n\r\nThe deepest part.\r\n".to_string();
    for level in (0..levels).rev() {
        let boundary = format!("b{level}");
        inner = format!(
            "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n\
             --{boundary}\r\n{inner}\r\n--{boundary}--\r\n"
        );
    }
    format!("From: sender@example.com\r\nTo: one@example.net\r\n{inner}")
}

#[tokio::test]
async fn the_recipient_ceiling_is_the_published_one_wherever_the_recipients_come_from() {
    // "Each message can include up to 50 recipient addresses across the To:, CC:,
    // or BCC: lines." The bound is driven at the published number and one past
    // it, from each place a recipient can come from.
    let listed: Vec<String> = (0..=MAX_RECIPIENTS)
        .map(|index| format!("r{index}@example.net"))
        .collect();
    let listed: Vec<&str> = listed.iter().map(String::as_str).collect();
    let rows: Vec<(String, String, Vec<Field>, u16)> = vec![
        (
            format!("{MAX_RECIPIENTS} addresses on the To: line"),
            recipient_line(MAX_RECIPIENTS),
            Vec::new(),
            200,
        ),
        (
            format!("{} addresses on the To: line", MAX_RECIPIENTS + 1),
            recipient_line(MAX_RECIPIENTS + 1),
            Vec::new(),
            400,
        ),
        (
            format!("{MAX_RECIPIENTS} addresses in Destinations"),
            String::new(),
            destinations(&listed[..MAX_RECIPIENTS]),
            200,
        ),
        (
            format!("{} addresses in Destinations", MAX_RECIPIENTS + 1),
            String::new(),
            destinations(&listed),
            400,
        ),
    ];
    for (case, header, extra, expected) in &rows {
        let router = emulator();
        verify_email(&router, "sender@example.com").await;
        let mut headers: Vec<(&str, &str)> = vec![("From", "sender@example.com")];
        if !header.is_empty() {
            headers.push(("To", header));
        }
        let raw = message(&headers, "Crowded.\r\n");
        let (status, body) = send_raw(&router, &raw, extra).await;
        assert_eq!(
            status, *expected,
            "case {case}: expected HTTP {expected}, got {status} with {body}"
        );
        if *expected == 400 {
            let (code, _) = refusal(&body);
            assert_eq!(
                code, "MessageRejected",
                "case {case}: expected the modelled MessageRejected, got {code:?} in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_message_past_the_published_size_is_refused_and_one_at_it_is_not() {
    // "The maximum message size, including attachments, is 10 MB." Driven at the
    // published ceiling and one byte past it, so the bound itself is asserted
    // rather than some number near it.
    for (case, bytes, expected) in [
        ("a message of exactly the published maximum", MAX_MESSAGE_BYTES, 200u16),
        ("one byte past the published maximum", MAX_MESSAGE_BYTES + 1, 400),
    ] {
        let router = emulator();
        verify_email(&router, "sender@example.com").await;
        let head = message(
            &[
                ("From", "sender@example.com"),
                ("To", "one@example.net"),
                ("Subject", "Large"),
            ],
            "",
        );
        let mut raw = String::with_capacity(bytes);
        raw.push_str(&head);
        raw.extend(std::iter::repeat_n('p', bytes - head.len()));
        assert_eq!(
            raw.len(),
            bytes,
            "case {case}: the probe message must be exactly {bytes} bytes"
        );

        let (status, body) = send_raw(&router, &raw, &[]).await;
        assert_eq!(
            status, expected,
            "case {case}: expected HTTP {expected}, got {status} with {body}"
        );
        if expected == 400 {
            let (code, _) = refusal(&body);
            assert_eq!(
                code, "MessageRejected",
                "case {case}: expected the modelled MessageRejected, got {code:?} in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_message_nested_past_the_multipart_ceiling_is_refused_and_one_at_it_is_not() {
    // Each level of a MIME tree is one `multipart/*` container and costs only a
    // header line, so the message-size ceiling does not bound how deeply a
    // caller can nest: the container ceiling does, and it is driven at the bound
    // and one past it. The rows also feed the handler the malformed and
    // truncated shapes a tree walk has to survive.
    for (case, levels, expected) in [
        ("no nesting at all", 0usize, 200u16),
        ("a single multipart container", 1, 200),
        (
            "nesting at the published container ceiling",
            MAX_MULTIPART_CONTAINERS,
            200,
        ),
        (
            "one container past the published ceiling",
            MAX_MULTIPART_CONTAINERS + 1,
            400,
        ),
        (
            "far past the published ceiling",
            MAX_MULTIPART_CONTAINERS * 100,
            400,
        ),
    ] {
        let router = emulator();
        verify_email(&router, "sender@example.com").await;
        let raw = nested_multipart_message(levels);
        let (status, body) = send_raw(&router, &raw, &[]).await;
        assert_eq!(
            status, expected,
            "case {case} ({levels} containers): expected HTTP {expected}, got {status} with \
             {body}"
        );
        if expected == 400 {
            let (code, _) = refusal(&body);
            assert_eq!(
                code, "MessageRejected",
                "case {case}: expected the modelled MessageRejected, got {code:?} in {body}"
            );
        }
    }

    // A message whose BODY merely mentions the media type is not a nested one:
    // only a `Content-Type` field declares a media type, so prose past the
    // ceiling is accepted.
    let router = emulator();
    verify_email(&router, "sender@example.com").await;
    let prose = (0..MAX_MULTIPART_CONTAINERS * 3)
        .map(|n| format!("line {n}: a multipart/mixed container holds parts.\r\n"))
        .collect::<String>();
    let raw = message(
        &[
            ("From", "sender@example.com"),
            ("To", "one@example.net"),
            ("Subject", "What multipart/mixed means"),
        ],
        &prose,
    );
    let (status, body) = send_raw(&router, &raw, &[]).await;
    assert_eq!(
        status, 200,
        "case a body mentioning the media type {} times: expected 200, got {status} with {body}",
        MAX_MULTIPART_CONTAINERS * 3
    );

    // A truncated tree — the closing boundaries cut off — is answered rather
    // than hung on, whichever way it is cut.
    let router = emulator();
    verify_email(&router, "sender@example.com").await;
    let full = nested_multipart_message(4);
    for (case, raw) in [
        ("a tree truncated mid-body", &full[..full.len() / 2]),
        (
            "a tree truncated just past its header block",
            &full[..full
                .find("\r\n\r\n")
                .map(|at| (at + 5).min(full.len()))
                .unwrap_or(full.len())],
        ),
    ] {
        let (status, body) = send_raw(&router, raw, &[]).await;
        assert!(
            status == 200 || status == 400,
            "case {case}: expected the call to be answered, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_malformed_or_unaddressed_message_is_refused_and_logs_nothing() {
    let router = emulator();
    verify_email(&router, "sender@example.com").await;

    // Each row is a message SES could not send, for a reason the operation's own
    // documentation states: no header/body separator, a header block that is not
    // one, no recipient at all, or a recipient that is not an address.
    let rows: Vec<(&str, String)> = vec![
        (
            "no blank line between the headers and the body",
            "From: sender@example.com\r\nTo: one@example.net\r\nSubject: Unseparated".to_string(),
        ),
        (
            "a header block that is not a header block",
            "this is not a header line\r\n\r\nBody.\r\n".to_string(),
        ),
        (
            "no recipient line and no Destinations",
            message(&[("From", "sender@example.com")], "To nobody.\r\n"),
        ),
        (
            "a recipient line naming nobody",
            message(
                &[("From", "sender@example.com"), ("To", "")],
                "To nobody.\r\n",
            ),
        ),
        (
            "a recipient that is not an address",
            message(
                &[("From", "sender@example.com"), ("To", "not-an-address")],
                "Misaddressed.\r\n",
            ),
        ),
        (
            "a recipient with no top-level domain",
            message(
                &[("From", "sender@example.com"), ("To", "user@localhost")],
                "Misaddressed.\r\n",
            ),
        ),
        (
            "one invalid recipient among valid ones",
            message(
                &[
                    ("From", "sender@example.com"),
                    ("To", "a@example.net, b@@example.net"),
                ],
                "Misaddressed.\r\n",
            ),
        ),
    ];
    for (case, raw) in &rows {
        let (status, body) = send_raw(&router, raw, &[]).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, _) = refusal(&body);
        assert_eq!(
            code, "MessageRejected",
            "case {case}: expected the modelled MessageRejected, got {code:?} in {body}"
        );
    }

    // A refusal never reaches the send log.
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent, 0.0,
        "case after {} refusals: expected an untouched send log, got {sent}",
        rows.len()
    );
}

#[tokio::test]
async fn each_send_gate_is_refused_with_its_own_modelled_error() {
    // The gates every send of this service passes, each with the error the model
    // binds to it: the account's pause flag, the configuration set the send
    // names, that set's own pause flag, and the sending identity.
    let raw = simple_message("sender@example.com", "one@example.net");

    // An unverified sender.
    let router = emulator();
    let (status, body) = send_raw(&router, &raw, &[]).await;
    let (code, stated) = refusal(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "MessageRejected"),
        "case an unverified sender: expected 400 MessageRejected, got {status} with {body}"
    );
    assert!(
        stated.contains("sender@example.com"),
        "case an unverified sender: expected the refusal to name the identity, got {stated:?}"
    );

    // A configuration set the account does not hold.
    verify_email(&router, "sender@example.com").await;
    let (status, body) = send_raw(
        &router,
        &raw,
        &[field("ConfigurationSetName", "no-such-set")],
    )
    .await;
    let (code, stated) = refusal(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "ConfigurationSetDoesNotExist"),
        "case an unknown configuration set: expected 400 ConfigurationSetDoesNotExist, got \
         {status} with {body}"
    );
    assert_eq!(
        stated, "Configuration set <no-such-set> does not exist.",
        "case an unknown configuration set: expected the set to be named, got {stated:?}"
    );

    // A configuration set that exists is NOT refused, which is what makes the
    // gate above about the set and not about every send.
    let (status, body) = call(
        &router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", "real-set")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateConfigurationSet: expected 200, got {status} with {body}"
    );
    let (status, body) = send_raw(&router, &raw, &[field("ConfigurationSetName", "real-set")]).await;
    assert_eq!(
        status, 200,
        "case a configuration set the account holds: expected 200, got {status} with {body}"
    );

    // That set with its sending paused.
    let (status, body) = call(
        &router,
        "UpdateConfigurationSetSendingEnabled",
        &[
            field("ConfigurationSetName", "real-set"),
            field("Enabled", "false"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateConfigurationSetSendingEnabled: expected 200, got {status} with {body}"
    );
    let (status, body) = send_raw(&router, &raw, &[field("ConfigurationSetName", "real-set")]).await;
    let (code, _) = refusal(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "ConfigurationSetSendingPausedException"),
        "case a paused configuration set: expected 400 ConfigurationSetSendingPausedException, \
         got {status} with {body}"
    );

    // The account itself paused, which is read before anything the request names.
    let (status, body) = call(
        &router,
        "UpdateAccountSendingEnabled",
        &[field("Enabled", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateAccountSendingEnabled: expected 200, got {status} with {body}"
    );
    let (status, body) = send_raw(&router, &raw, &[]).await;
    let (code, _) = refusal(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "AccountSendingPausedException"),
        "case a paused account: expected 400 AccountSendingPausedException, got {status} with \
         {body}"
    );

    // Exactly one message was accepted through all of this, so the log holds
    // that one recipient and nothing the refusals carried.
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent, 1.0,
        "case after one accepted send and four refusals: expected 1 counted recipient, got {sent}"
    );
}

#[tokio::test]
async fn a_verified_domain_covers_the_mailboxes_that_send_from_it() {
    // The same reading of a verified sender as every other send: "You can only
    // send email from verified email addresses or domains", and a verified domain
    // covers the mailboxes in it and under its subdomains.
    let router = emulator();
    let (status, body) = call(
        &router,
        "VerifyDomainIdentity",
        &[field("Domain", "example.org")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case VerifyDomainIdentity: expected 200, got {status} with {body}"
    );

    let cases: &[(&str, u16)] = &[
        ("anyone@example.org", 200),
        ("anyone@mail.example.org", 200),
        ("anyone@deep.mail.example.org", 200),
        ("anyone@notexample.org", 400),
        ("anyone@example.org.evil.com", 400),
    ];
    for (from, expected) in cases {
        let raw = simple_message(from, "one@example.net");
        let (status, body) = send_raw(&router, &raw, &[]).await;
        assert_eq!(
            status, *expected,
            "case a message from {from:?} under a verified example.org: expected {expected}, got \
             {status} with {body}"
        );
    }
}

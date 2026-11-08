// SPDX-License-Identifier: CC0-1.0

//! What makes a value a CHANNEL ARN, asserted over a table whose rows differ along every
//! field of the ARN grammar.
//!
//! The rule: a channel reference written as an ARN names a channel only when it is an ARN
//! of the partition this emulator serves and of the service that mints channels, carrying
//! a Region label, a 12-digit account and a `channel/<channel-id>` resource whose id is
//! written the way CloudTrail writes a channel id — the canonical hyphenated UUID its
//! control plane mints (`CreateChannel`). Every other `arn:` value is not a channel ARN
//! and is refused `InvalidChannelARN` with the sentence the model documents for that
//! shape, WHATEVER bytes or encoding it carries: the form of the reference decides which
//! refusal the call takes, so the same logical reference cannot change error class by
//! being spelled in another encoding.
//!
//! A value the grammar DOES admit is resolved against the store, which is the next
//! question and a different answer (`ChannelNotFound`, or a success for a registered
//! channel) — so each row states which of the two layers answers it.
//!
//! The client is this crate's one test harness, reached from the suite that owns it.

use crate::put_audit_events::{ev, Harness, Reply, REGION};
use service_com_amazonaws_cloudtraildata_cloudtraildataservice::state::Channel;

/// The sentence `InvalidChannelARN` carries, as the model documents the shape
/// (`smithy_query(query="shape", shape="InvalidChannelARN")`).
const INVALID_CHANNEL_ARN: &str = "The specified channel ARN is not a valid channel ARN.";

/// A JSON audit event that passes per-event validation, so nothing but the channel
/// reference decides these calls.
const GOOD_EVENT: &str = r#"{"eventSource":"custom.app","eventName":"Login"}"#;

/// The channel id this suite registers, in the canonical form CloudTrail mints a channel
/// id in.
const REGISTERED_ID: &str = "6a5d2c8b-71e4-4f03-9b2a-5c7d8e9f0a1b";

/// A channel id of the same form that no channel is registered under.
const UNHELD_ID: &str = "7b6e3d9c-82f5-4a14-8c3b-6d8e9f0a1b2c";

/// The prose member of a refusal: the modelled error shapes of this service spell it
/// `message`, and the authorization layer's `AccessDeniedException` spells it `Message`.
fn message_of(reply: &Reply) -> String {
    for member in ["message", "Message"] {
        if let Some(text) = reply.body.get(member).and_then(serde_json::Value::as_str) {
            return text.to_string();
        }
    }
    String::new()
}

/// `value` with every byte a URI query component may not carry percent-encoded, so a
/// reference carrying multi-byte characters reaches the service as the caller sent it
/// rather than failing to be a URI at all.
fn encoded(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The ARN the rows build on: every field is well formed unless the row changes one.
fn arn_of(partition: &str, vendor: &str, region: &str, account: &str, resource: &str) -> String {
    format!("arn:{partition}:{vendor}:{region}:{account}:{resource}")
}

/// A channel ARN in the partition, Region and account this emulator serves, over the
/// resource field the row states.
fn served(resource: &str) -> String {
    arn_of(
        "aws",
        "cloudtrail",
        REGION,
        engine::emulator_account_id(),
        resource,
    )
}

/// One row: the reference, and which layer answers it.
struct Row {
    case: &'static str,
    channel: String,
    /// The error code expected, or `""` for a call the service serves.
    code: &'static str,
}

/// A value the ARN grammar does not admit is the service's own `InvalidChannelARN`; a
/// value it admits reaches channel resolution. The rows differ along the partition, the
/// vendor, the Region field, the account field, the resource type and the id form — every
/// field of the grammar — plus the registered channel as the control.
#[tokio::test]
async fn the_channel_arn_grammar_decides_which_refusal_a_reference_takes() {
    let harness = Harness::new();
    let registered = served(&format!("channel/{REGISTERED_ID}"));
    harness.register(Channel::from_arn(&registered, "Custom").expect("a channel ARN"));

    let rows: Vec<Row> = vec![
        Row {
            case: "the registered channel ARN",
            channel: registered.clone(),
            code: "",
        },
        Row {
            case: "a well-formed channel ARN this emulator holds no channel for",
            channel: served(&format!("channel/{UNHELD_ID}")),
            code: "ChannelNotFound",
        },
        // The partitions AWS publishes beside the one this emulator serves (the partition
        // `::engine::emulator_arn` mints every ARN of this emulator in): a reference built
        // in any of them names a resource no call here can reach. `aws-cn` is the row the
        // issue cited; the others are the same rule on the partitions it did not.
        Row {
            case: "the China partition",
            channel: arn_of(
                "aws-cn",
                "cloudtrail",
                "cn-north-1",
                engine::emulator_account_id(),
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "InvalidChannelARN",
        },
        Row {
            case: "the GovCloud partition",
            channel: arn_of(
                "aws-us-gov",
                "cloudtrail",
                "us-gov-west-1",
                engine::emulator_account_id(),
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "InvalidChannelARN",
        },
        Row {
            case: "an ISO partition",
            channel: arn_of(
                "aws-iso",
                "cloudtrail",
                REGION,
                engine::emulator_account_id(),
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "InvalidChannelARN",
        },
        Row {
            case: "the served partition spelled in upper case",
            channel: arn_of(
                "AWS",
                "cloudtrail",
                REGION,
                engine::emulator_account_id(),
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "InvalidChannelARN",
        },
        Row {
            case: "an empty partition field",
            channel: arn_of(
                "",
                "cloudtrail",
                REGION,
                engine::emulator_account_id(),
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "InvalidChannelARN",
        },
        Row {
            case: "another service's ARN carrying a channel resource",
            channel: arn_of(
                "aws",
                "s3",
                REGION,
                engine::emulator_account_id(),
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "InvalidChannelARN",
        },
        Row {
            case: "another CloudTrail resource type",
            channel: served("trail/a-trail"),
            code: "InvalidChannelARN",
        },
        Row {
            case: "a resource field with no type separator",
            channel: served("channel"),
            code: "InvalidChannelARN",
        },
        Row {
            case: "an empty Region field",
            channel: arn_of(
                "aws",
                "cloudtrail",
                "",
                engine::emulator_account_id(),
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "InvalidChannelARN",
        },
        Row {
            case: "a Region field that is not a Region label",
            channel: arn_of(
                "aws",
                "cloudtrail",
                "US_EAST_1",
                engine::emulator_account_id(),
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "InvalidChannelARN",
        },
        Row {
            case: "another Region this emulator holds no channel in",
            channel: arn_of(
                "aws",
                "cloudtrail",
                "ap-southeast-2",
                engine::emulator_account_id(),
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "ChannelNotFound",
        },
        Row {
            case: "an account field of eleven digits",
            channel: arn_of(
                "aws",
                "cloudtrail",
                REGION,
                "00000000000",
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "InvalidChannelARN",
        },
        Row {
            case: "an account field that is not digits",
            channel: arn_of(
                "aws",
                "cloudtrail",
                REGION,
                "00000000000x",
                &format!("channel/{REGISTERED_ID}"),
            ),
            code: "InvalidChannelARN",
        },
    ];

    for row in &rows {
        let reply = harness
            .put(&encoded(&row.channel), None, &[ev("evt-1", GOOD_EVENT)])
            .await;
        let case = row.case;
        let expected_status = if row.code.is_empty() { 200 } else { 400 };
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            row.code,
            "case {case:?}: expected error code {:?}, got body {}",
            row.code,
            reply.body
        );
        if row.code == "InvalidChannelARN" {
            let stated = message_of(&reply);
            assert_eq!(
                stated, INVALID_CHANNEL_ARN,
                "case {case:?}: expected the documented sentence {INVALID_CHANNEL_ARN:?}, \
                 got {stated:?}"
            );
        }
    }
}

/// The id inside a channel ARN must be written the way CloudTrail writes a channel id —
/// the canonical hyphenated UUID its control plane mints — so an ARN carrying anything
/// else is not a channel ARN. The rows differ along the FORM of the id: the canonical
/// rendering, the other spellings of the same UUID the `uuid` crate's grammar admits
/// (simple, braced, URN, upper case), ids of the wrong length, and strings that are no
/// UUID at all.
#[tokio::test]
async fn the_resource_id_of_a_channel_arn_must_be_a_channel_id() {
    let harness = Harness::new();
    let registered = served(&format!("channel/{REGISTERED_ID}"));
    harness.register(Channel::from_arn(&registered, "Custom").expect("a channel ARN"));

    // (case, the id inside `channel/`, the code the call is answered with — "" is a
    // served call). The canonical row is the registered channel, so a row that expects
    // `ChannelNotFound` is one the grammar admitted and the store did not hold.
    let rows: Vec<(&str, String, &str)> = vec![
        ("the canonical form of a registered id", REGISTERED_ID.to_string(), ""),
        (
            "the canonical form of an id no channel carries",
            UNHELD_ID.to_string(),
            "ChannelNotFound",
        ),
        (
            "the same UUID with its hyphens removed",
            REGISTERED_ID.replace('-', ""),
            "InvalidChannelARN",
        ),
        (
            "the same UUID in upper case",
            REGISTERED_ID.to_uppercase(),
            "InvalidChannelARN",
        ),
        (
            "the same UUID in braces",
            format!("{{{REGISTERED_ID}}}"),
            "InvalidChannelARN",
        ),
        (
            "the same UUID as a URN",
            format!("urn:uuid:{REGISTERED_ID}"),
            "InvalidChannelARN",
        ),
        (
            "an id one hex digit short",
            REGISTERED_ID[..REGISTERED_ID.len() - 1].to_string(),
            "InvalidChannelARN",
        ),
        (
            "an id one hex digit long",
            format!("{REGISTERED_ID}f"),
            "InvalidChannelARN",
        ),
        (
            "a word that is no UUID",
            "cafe-rocket-not-real".to_string(),
            "InvalidChannelARN",
        ),
        ("an id with no hex meaning", "not-a-channel".to_string(), "InvalidChannelARN"),
        ("an empty id", String::new(), "InvalidChannelARN"),
        (
            "an id carrying a path separator",
            format!("{REGISTERED_ID}/extra"),
            "InvalidChannelARN",
        ),
    ];

    for (case, id, code) in &rows {
        let channel = served(&format!("channel/{id}"));
        let reply = harness
            .put(&encoded(&channel), None, &[ev("evt-1", GOOD_EVENT)])
            .await;
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case:?}: expected error code {code:?} for id {id:?}, got body {}",
            reply.body
        );
    }
}

/// The ENCODING of a reference does not decide which refusal it takes: a non-ASCII id and
/// its plain-ASCII analogue are the same logical reference as far as the grammar is
/// concerned — neither is a channel id — so both are answered under the same code and the
/// same sentence. Asserted over pairs that differ in how the multi-byte characters sit in
/// the id.
#[tokio::test]
async fn an_ids_encoding_does_not_change_which_refusal_it_takes() {
    let harness = Harness::new();

    // (case, the id as multi-byte characters, its plain-ASCII analogue)
    let pairs: Vec<(&str, &str, &str)> = vec![
        (
            "accents and an emoji in the middle",
            "café-🚀-not-real",
            "cafe-rocket-not-real",
        ),
        ("a leading multi-byte character", "ñ-channel", "n-channel"),
        ("a trailing multi-byte character", "channel-é", "channel-e"),
        (
            "multi-byte characters where a canonical id carries hex digits",
            "6a5d2c8b-71e4-4f03-9b2a-5c7d8e9f0a1é",
            "6a5d2c8b-71e4-4f03-9b2a-5c7d8e9f0a1x",
        ),
    ];

    for (case, multi_byte, ascii) in &pairs {
        let mut answers = Vec::new();
        for id in [*multi_byte, *ascii] {
            let channel = served(&format!("channel/{id}"));
            let reply = harness
                .put(&encoded(&channel), None, &[ev("evt-1", GOOD_EVENT)])
                .await;
            answers.push((reply.status, reply.error_code(), message_of(&reply)));
        }
        assert_eq!(
            answers[0], answers[1],
            "case {case:?}: the multi-byte id {multi_byte:?} and its ASCII analogue \
             {ascii:?} must be answered alike, got {:?} and {:?}",
            answers[0], answers[1]
        );
    }

    // The agreement above must not be agreement on the WRONG class: an id in neither
    // encoding is a channel id, so both forms are the malformed-ARN refusal, while the
    // canonical id the same bytes would spell is the store's answer instead.
    for (id, code) in [
        ("café-🚀-not-real", "InvalidChannelARN"),
        ("cafe-rocket-not-real", "InvalidChannelARN"),
        (UNHELD_ID, "ChannelNotFound"),
    ] {
        let channel = served(&format!("channel/{id}"));
        let reply = harness
            .put(&encoded(&channel), None, &[ev("evt-1", GOOD_EVENT)])
            .await;
        assert_eq!(
            reply.error_code(),
            code,
            "case \"the id {id:?}\": expected {code:?}, got body {}",
            reply.body
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SHAPE of every identifier this service generates.
//!
//! Every case drives real HTTP through the production router — restJson1 request on the
//! wire in, restJson1 response out — so nothing here can pass by reading a handler's Rust
//! types directly.
//!
//! The rule under test: an identifier Amazon Chime SDK Messaging generates is minted in
//! the id form that identifier's class carries — the generated last segment of one of this
//! service's app-instance-scoped ARNs (`…/channel/<id>`, `…/channel-flow/<id>`) is a
//! 64-character lowercase hex digest, and a `MessageId` is a canonical hyphenated
//! lowercase UUID — while an id the CALLER supplies is carried through unchanged. Each
//! generated id is also unique per resource and is the id the resource reads back by.
//!
//! The tables span what that rule generalises over rather than re-running one recorded
//! request: two `AppInstanceArn` forms, every `ChannelMode` and `ChannelPrivacy` wire
//! value, optional members present and absent, a caller-supplied `ChannelId` beside a
//! generated one, every `ChannelMessageType` and `ChannelMessagePersistenceType` wire
//! value, and a bulk run of sends whose ids must all differ.

use std::collections::HashSet;

use serde_json::{json, Value};

// The restJson1 label encoder and the HTTP harness this binary's main suite already
// states: one copy for the whole test target, so two suites cannot encode a label or
// build a request differently.
use super::channel_messaging::{encode, Emulator};

/// An `AppInstanceArn` whose own id is a uuid, the form Chime SDK Identity mints.
const APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/3c4d1b53-8e8a-4e0f-9f22-7a7b1e1a9c55";
/// A second, differently shaped `AppInstanceArn`, so the id rule is not tested under one
/// ARN form only.
const SHORT_APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai2";

/// How many characters a generated channel id or channel flow id carries.
///
/// The id shape Amazon Chime SDK Messaging's recorded app-instance-scoped ARNs have: a
/// full lowercase hex digest, which is also exactly `ChannelId`'s modelled maximum
/// (`smithy_query(query="shape", shape="ChannelId")`: `len 1..64`).
///
/// Stated here independently of the service's own digest width on purpose: the number is
/// what the service owes its callers, so a test that read it from the implementation could
/// not tell a changed id shape from an unchanged one.
const GENERATED_ID_LENGTH: usize = 64;

/// The `ChannelMode` wire values, from
/// `smithy_query(query="shape", shape="ChannelMode")`.
const CHANNEL_MODES: [&str; 2] = ["UNRESTRICTED", "RESTRICTED"];
/// The `ChannelPrivacy` wire values, from
/// `smithy_query(query="shape", shape="ChannelPrivacy")`.
const CHANNEL_PRIVACIES: [&str; 2] = ["PUBLIC", "PRIVATE"];
/// The `ChannelMessageType` wire values, from
/// `smithy_query(query="shape", shape="ChannelMessageType")`.
const MESSAGE_TYPES: [&str; 2] = ["STANDARD", "CONTROL"];
/// The `ChannelMessagePersistenceType` wire values, from
/// `smithy_query(query="shape", shape="ChannelMessagePersistenceType")`.
const PERSISTENCE_TYPES: [&str; 2] = ["PERSISTENT", "NON_PERSISTENT"];

/// Why `value` is not a generated ARN id segment, or `None` when it is one.
fn resource_id_violation(value: &str) -> Option<String> {
    if value.len() != GENERATED_ID_LENGTH {
        return Some(format!(
            "expected {GENERATED_ID_LENGTH} characters, got {}",
            value.len()
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    {
        return Some("expected lowercase hex characters only".to_string());
    }
    None
}

/// Why `value` is not a canonical hyphenated lowercase UUID, or `None` when it is one.
///
/// The form is `8-4-4-4-12` lowercase hex characters; the version nibble (the first
/// character of the third group) is `4` and the variant nibble (the first character of
/// the fourth group) is one of `8`, `9`, `a`, `b`, which are the two nibbles a UUID built
/// from random bytes carries.
fn uuid_violation(value: &str) -> Option<String> {
    let groups: Vec<&str> = value.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
    if lengths != vec![8, 4, 4, 4, 12] {
        return Some(format!("expected groups of 8-4-4-4-12, got {lengths:?}"));
    }
    if !groups
        .iter()
        .all(|group| group.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
    {
        return Some("expected lowercase hex characters in every group".to_string());
    }
    if !groups[2].starts_with('4') {
        return Some(format!(
            "expected the version nibble 4, got {:?}",
            &groups[2][..1]
        ));
    }
    if !matches!(&groups[3][..1], "8" | "9" | "a" | "b") {
        return Some(format!(
            "expected a variant nibble of 8, 9, a or b, got {:?}",
            &groups[3][..1]
        ));
    }
    None
}

/// The segment of `arn` that follows `.../<resource_type>/`.
fn id_segment(arn: &str, resource_type: &str) -> String {
    let marker = format!("/{resource_type}/");
    arn.rsplit_once(&marker)
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| panic!("{arn:?} carries no {marker:?} segment"))
}

/// One row of the `CreateChannel` table.
#[derive(Debug)]
struct ChannelCase {
    label: String,
    app_instance: &'static str,
    mode: Option<&'static str>,
    privacy: Option<&'static str>,
    metadata: Option<&'static str>,
    channel_id: Option<&'static str>,
}

/// RULE: the channel id `CreateChannel` generates when the request names none is a
/// 64-character lowercase hex digest, unique per channel, and it is the id the channel's
/// ARN reads back by; a `ChannelId` the caller supplies is carried through unchanged.
#[tokio::test]
async fn a_generated_channel_id_is_a_full_hex_digest() {
    let emulator = Emulator::new();

    let mut rows = vec![
        ChannelCase {
            label: "only the required members".to_string(),
            app_instance: APP_INSTANCE,
            mode: None,
            privacy: None,
            metadata: None,
            channel_id: None,
        },
        ChannelCase {
            label: "a second app instance, so the ARN form varies".to_string(),
            app_instance: SHORT_APP_INSTANCE,
            mode: None,
            privacy: None,
            metadata: Some("{\"topic\":\"support\"}"),
            channel_id: None,
        },
        ChannelCase {
            label: "a ChannelId the caller chose, which is NOT generated".to_string(),
            app_instance: APP_INSTANCE,
            mode: None,
            privacy: None,
            metadata: None,
            channel_id: Some("caller.chosen-id_1"),
        },
        ChannelCase {
            label: "a one-character ChannelId the caller chose".to_string(),
            app_instance: SHORT_APP_INSTANCE,
            mode: None,
            privacy: None,
            metadata: None,
            channel_id: Some("c"),
        },
    ];
    // Every modelled Mode x Privacy combination, each with a generated id.
    for mode in CHANNEL_MODES {
        for privacy in CHANNEL_PRIVACIES {
            rows.push(ChannelCase {
                label: format!("Mode {mode} / Privacy {privacy}"),
                app_instance: APP_INSTANCE,
                mode: Some(mode),
                privacy: Some(privacy),
                metadata: None,
                channel_id: None,
            });
        }
    }

    let mut generated: HashSet<String> = HashSet::new();
    for (index, case) in rows.iter().enumerate() {
        let mut request = json!({
            "AppInstanceArn": case.app_instance,
            "Name": format!("channel-{index}"),
            "ClientRequestToken": format!("token-{index}"),
        });
        for (member, value) in [
            ("Mode", case.mode),
            ("Privacy", case.privacy),
            ("Metadata", case.metadata),
            ("ChannelId", case.channel_id),
        ] {
            if let Some(value) = value {
                request[member] = json!(value);
            }
        }

        let created = emulator.create_channel(&request).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: CreateChannel must answer 201, got {} {:?}",
            case.label, created.status, created.body
        );
        let arn = created.body["ChannelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no ChannelArn in {:?}", case.label, created.body))
            .to_string();
        assert!(
            arn.starts_with(&format!("{}/channel/", case.app_instance)),
            "case {:?}: expected the ARN under {:?}, got {arn:?}",
            case.label,
            case.app_instance
        );
        let channel_id = id_segment(&arn, "channel");

        match case.channel_id {
            // A caller-supplied id is the caller's value, never a minted one.
            Some(chosen) => assert_eq!(
                channel_id, chosen,
                "case {:?}: expected the ChannelId the caller chose, got {channel_id:?}",
                case.label
            ),
            None => {
                if let Some(violation) = resource_id_violation(&channel_id) {
                    panic!(
                        "case {:?}: the generated channel id {channel_id:?} is not a full hex \
                         digest: {violation}",
                        case.label
                    );
                }
                assert!(
                    generated.insert(channel_id.clone()),
                    "case {:?}: the generated channel id {channel_id:?} repeats an earlier one",
                    case.label
                );
            }
        }

        // The ARN the mint produced is the one the channel reads back by.
        let described = emulator.describe_channel(&arn).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: DescribeChannel must answer 200 for {arn:?}, got {} {:?}",
            case.label, described.status, described.body
        );
        assert_eq!(
            described.body["Channel"]["ChannelArn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: DescribeChannel must report the created ARN, got {:?}",
            case.label,
            described.body["Channel"]["ChannelArn"]
        );
    }
}

/// RULE, applied to the other ARN this service derives an id segment for: the channel flow
/// id `CreateChannelFlow` generates is a 64-character lowercase hex digest, unique per
/// flow, and it is the id the flow reads back by.
#[tokio::test]
async fn a_generated_channel_flow_id_is_a_full_hex_digest() {
    let emulator = Emulator::new();

    // (case, the app instance the flow is created under, how many processors it declares)
    //
    // The rows span the modelled `ProcessorList len 1..3` and both app-instance forms, so
    // the id shape is not tested under one request form only.
    let rows = [
        ("one processor", APP_INSTANCE, 1),
        ("two processors", SHORT_APP_INSTANCE, 2),
        ("three processors, the longest list the model allows", APP_INSTANCE, 3),
    ];

    let mut generated: HashSet<String> = HashSet::new();
    for (index, (case, app_instance, processors)) in rows.iter().enumerate() {
        let processors: Vec<Value> = (1..=*processors)
            .map(|order| {
                json!({
                    "Name": format!("processor-{order}"),
                    "ExecutionOrder": order,
                    "FallbackAction": "ABORT",
                    "Configuration": {
                        "Lambda": {
                            "ResourceArn": "arn:aws:lambda:us-east-1:111122223333:function:redact",
                            "InvocationType": "ASYNC",
                        }
                    }
                })
            })
            .collect();
        let created = emulator
            .call(
                "POST",
                "/channel-flows",
                Some(&json!({
                    "AppInstanceArn": app_instance,
                    "Name": format!("flow-{index}"),
                    "ClientRequestToken": format!("flow-token-{index}"),
                    "Processors": processors,
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateChannelFlow must answer 201, got {} {:?}",
            created.status, created.body
        );
        let arn = created.body["ChannelFlowArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ChannelFlowArn in {:?}", created.body))
            .to_string();
        let flow_id = id_segment(&arn, "channel-flow");
        if let Some(violation) = resource_id_violation(&flow_id) {
            panic!(
                "case {case:?}: the generated channel flow id {flow_id:?} is not a full hex \
                 digest: {violation}"
            );
        }
        assert!(
            generated.insert(flow_id.clone()),
            "case {case:?}: the generated channel flow id {flow_id:?} repeats an earlier one"
        );

        let described = emulator
            .call("GET", &format!("/channel-flows/{}", encode(&arn)), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeChannelFlow must answer 200 for {arn:?}, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.body["ChannelFlow"]["ChannelFlowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: DescribeChannelFlow must report the created ARN, got {:?}",
            described.body["ChannelFlow"]["ChannelFlowArn"]
        );
    }
}

/// RULE: the `MessageId` `SendChannelMessage` generates is a canonical hyphenated
/// lowercase UUID, unique per message, and it is the id the message reads back by.
#[tokio::test]
async fn a_generated_message_id_is_a_hyphenated_uuid() {
    let emulator = Emulator::new();
    let channel = emulator
        .channel(APP_INSTANCE, "message-ids", "message-ids-token")
        .await;

    // (case, Type, Persistence, whether the optional Metadata member is present)
    //
    // The rows span every modelled `ChannelMessageType` x
    // `ChannelMessagePersistenceType` pair, with `Metadata` present on half of them.
    let mut rows: Vec<(String, &str, &str, bool)> = Vec::new();
    for message_type in MESSAGE_TYPES {
        for persistence in PERSISTENCE_TYPES {
            rows.push((
                format!("Type {message_type} / Persistence {persistence}"),
                message_type,
                persistence,
                message_type == "STANDARD",
            ));
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    for (index, (case, message_type, persistence, with_metadata)) in rows.iter().enumerate() {
        let mut request = json!({
            "Content": format!("message {index}"),
            "Type": message_type,
            "Persistence": persistence,
            "ClientRequestToken": format!("send-token-{index}"),
        });
        if *with_metadata {
            request["Metadata"] = json!("{\"kind\":\"test\"}");
        }
        let sent = emulator.send_message(&channel, &request).await;
        assert_eq!(
            sent.status, 201,
            "case {case:?}: SendChannelMessage must answer 201, got {} {:?}",
            sent.status, sent.body
        );
        let message_id = sent.body["MessageId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no MessageId in {:?}", sent.body))
            .to_string();
        if let Some(violation) = uuid_violation(&message_id) {
            panic!(
                "case {case:?}: the generated MessageId {message_id:?} is not a hyphenated \
                 uuid: {violation}"
            );
        }
        assert!(
            seen.insert(message_id.clone()),
            "case {case:?}: the generated MessageId {message_id:?} repeats an earlier one"
        );

        // A persisted message reads back by the id the send returned — the hyphens
        // included, through the restJson1 label the id becomes.
        if *persistence == "PERSISTENT" {
            let got = emulator.get_message(&channel, &message_id).await;
            assert_eq!(
                got.status, 200,
                "case {case:?}: GetChannelMessage must answer 200 for {message_id:?}, got {} {:?}",
                got.status, got.body
            );
            assert_eq!(
                got.body["ChannelMessage"]["MessageId"].as_str(),
                Some(message_id.as_str()),
                "case {case:?}: GetChannelMessage must report the id the send returned, got {:?}",
                got.body["ChannelMessage"]["MessageId"]
            );
            assert_eq!(
                got.body["ChannelMessage"]["Content"].as_str(),
                Some(format!("message {index}").as_str()),
                "case {case:?}: GetChannelMessage must report the content that was sent, got {:?}",
                got.body["ChannelMessage"]["Content"]
            );
        }
    }

    // The uniqueness and the form hold over a run of sends, not just the table above: 40
    // more messages into the same channel, every id its own canonical uuid.
    for index in 0..40 {
        let sent = emulator
            .send_message(
                &channel,
                &json!({
                    "Content": format!("bulk {index}"),
                    "Type": "STANDARD",
                    "Persistence": "PERSISTENT",
                    "ClientRequestToken": format!("bulk-token-{index}"),
                }),
            )
            .await;
        assert_eq!(
            sent.status, 201,
            "bulk send {index}: SendChannelMessage must answer 201, got {} {:?}",
            sent.status, sent.body
        );
        let message_id = sent.body["MessageId"]
            .as_str()
            .unwrap_or_else(|| panic!("bulk send {index}: no MessageId in {:?}", sent.body))
            .to_string();
        if let Some(violation) = uuid_violation(&message_id) {
            panic!(
                "bulk send {index}: the generated MessageId {message_id:?} is not a hyphenated \
                 uuid: {violation}"
            );
        }
        assert!(
            seen.insert(message_id.clone()),
            "bulk send {index}: the generated MessageId {message_id:?} repeats an earlier one"
        );
    }
}

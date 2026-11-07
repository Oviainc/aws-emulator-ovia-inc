// SPDX-License-Identifier: CC0-1.0

//! The member every error document this service answers carries its detail text in, driven
//! over the wire (restJson1) through the same `MultiServiceRouter` the production router
//! serves with.
//!
//! The rule every assertion here is about: an error document AWS Chatbot answers ALWAYS
//! carries the error shape's message member, spelled as that shape declares it — the sentence
//! the error class words, or JSON `null` where the class words none. A client can then tell
//! "this class states no message" from "the member was not reported at all", which is what an
//! SDK reads an absent key as (it substitutes the empty string). The member's spelling is the
//! shape's: `Message` on `ResourceNotFoundException` and `InternalServiceError`, `message` on
//! `InvalidRequestException`, `UnauthorizedException`, `ConflictException`,
//! `TooManyTagsException` and `ServiceUnavailableException` (`smithy_query`
//! shape=ResourceNotFoundException,InvalidRequestException,UnauthorizedException,ConflictException),
//! so only the declared one is reported.
//!
//! The table spans the classes rather than one recorded request: it crosses the error classes
//! this service refuses with, both member spellings, a class that words a message and two that
//! word none, four different HTTP statuses, and a refusal rendered before any handler ran —
//! whose message is built from the request and is therefore never absent.

use serde_json::{json, Value};
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;

use crate::wire::{Emulator, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

/// The Slack workspace the channel configuration below is created in: a workspace has to be
/// authorized with the account first, and no operation in this model performs that handshake.
const SLACK_TEAM_ID: &str = "T0123ABCD";
const SLACK_TEAM_NAME: &str = "Primary Workspace";

/// The name the seeded resources carry, so a row can collide with one.
const PRESENT: &str = "present";

/// The most tags one resource carries (`smithy_query` shape=TagList: `@length` min 0 max 200),
/// which the tag-cap row has to reach before the refusal it asserts is reachable.
const MAX_TAGS: usize = 200;

/// Both spellings an error document's message member can occupy in this model; a row names
/// the one its shape declares and the other must be absent.
const MESSAGE_MEMBERS: [&str; 2] = ["Message", "message"];

fn custom_action_arn(action_name: &str) -> String {
    // A custom action is account-scoped, so its ARN carries an EMPTY region segment.
    format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{action_name}")
}

/// This service mounted alone, holding one custom action and one Slack channel configuration
/// named [`PRESENT`], plus a custom action already carrying the full 200 tags.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    chatbot::state::authorize_slack_workspace(&*emulator.engine, SLACK_TEAM_ID, SLACK_TEAM_NAME)
        .expect("the shared engine holds the chatbot store");

    let full_tags = Value::Array(
        (0..MAX_TAGS)
            .map(|n| json!({"TagKey": format!("key-{n:04}"), "TagValue": "v"}))
            .collect(),
    );
    for (path, request) in [
        (
            "/create-custom-action",
            json!({"ActionName": PRESENT, "Definition": {"CommandText": "s3 ls"}}),
        ),
        (
            "/create-custom-action",
            json!({
                "ActionName": "at-the-cap",
                "Definition": {"CommandText": "s3 ls"},
                "Tags": full_tags,
            }),
        ),
        (
            "/create-slack-channel-configuration",
            json!({
                "ConfigurationName": PRESENT,
                "SlackTeamId": SLACK_TEAM_ID,
                "SlackChannelId": "C0123ABCD",
                "IamRoleArn": IAM_ROLE_ARN,
            }),
        ),
    ] {
        let created = emulator.call(path, &request).await;
        assert_eq!(
            created.status, 200,
            "seeding {path} with {request}: expected 200, got {} with {}",
            created.status, created.body
        );
    }

    emulator
}

/// One row: a request that is refused, and the message member its error document must carry.
struct Case {
    label: String,
    path: &'static str,
    request: Value,
    status: u16,
    code: &'static str,
    /// The spelling the refused shape declares for its message member.
    member: &'static str,
    /// The value that member must carry: a sentence the class words, or a null where it
    /// words none.
    value: Value,
}

#[tokio::test]
async fn every_error_document_carries_its_shape_s_message_member() {
    let emulator = seeded().await;
    let absent = custom_action_arn("never-created");
    let at_the_cap = custom_action_arn("at-the-cap");

    let cases = vec![
        Case {
            label: "a read of an action the account does not hold".to_string(),
            path: "/get-custom-action",
            request: json!({"CustomActionArn": absent}),
            status: 404,
            code: "ResourceNotFoundException",
            member: "Message",
            value: json!(format!(
                "sdk action [never-created] not found for account [{ACCOUNT}]"
            )),
        },
        Case {
            label: "an update of an action the account does not hold".to_string(),
            path: "/update-custom-action",
            request: json!({
                "CustomActionArn": absent,
                "Definition": {"CommandText": "s3 ls"},
            }),
            status: 404,
            code: "ResourceNotFoundException",
            member: "Message",
            value: json!(format!(
                "Sdk action with id [never-created] does not exist in account [{ACCOUNT}]"
            )),
        },
        Case {
            // The one class on this resource AWS Chatbot words nothing for.
            label: "a delete of an action the account does not hold".to_string(),
            path: "/delete-custom-action",
            request: json!({"CustomActionArn": absent}),
            status: 404,
            code: "ResourceNotFoundException",
            member: "Message",
            value: Value::Null,
        },
        Case {
            label: "a delete addressed by an ARN no create can mint".to_string(),
            path: "/delete-custom-action",
            request: json!({
                "CustomActionArn":
                    format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:custom-action/{PRESENT}"),
            }),
            status: 403,
            code: "UnauthorizedException",
            member: "message",
            value: json!("Unauthorized"),
        },
        Case {
            label: "a create of a name the account already holds".to_string(),
            path: "/create-custom-action",
            request: json!({"ActionName": PRESENT, "Definition": {"CommandText": "s3 ls"}}),
            status: 409,
            code: "ConflictException",
            member: "message",
            value: json!(format!(
                "Resource with arn {} already exists.",
                custom_action_arn(PRESENT)
            )),
        },
        Case {
            // The other class this service words nothing for, under the other spelling.
            label: "a tag merge past the per-resource cap".to_string(),
            path: "/tag-resource",
            request: json!({
                "ResourceARN": at_the_cap,
                "Tags": [{"TagKey": "one-too-many", "TagValue": "v"}],
            }),
            status: 400,
            code: "InvalidRequestException",
            member: "message",
            value: Value::Null,
        },
        Case {
            // Refused BEFORE any handler ran: the message is built from the request, so it
            // is never absent and this seam leaves it exactly as the framework wrote it.
            label: "a read whose ARN does not satisfy the modelled pattern".to_string(),
            path: "/get-custom-action",
            request: json!({"CustomActionArn": "not-an-arn"}),
            status: 400,
            code: "InvalidRequestException",
            member: "message",
            value: json!("Invalid request body"),
        },
    ];

    for case in &cases {
        let answer = emulator.call(case.path, &case.request).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?} on {}: expected HTTP {}, got {} with {}",
            case.label, case.path, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            case.code,
            "case {:?} on {}: expected the modelled {}, got {}",
            case.label,
            case.path,
            case.code,
            answer.body
        );
        assert_eq!(
            answer.body.get(case.member),
            Some(&case.value),
            "case {:?} on {}: expected {} to be {}, got {}",
            case.label,
            case.path,
            case.member,
            case.value,
            answer.body
        );
        for other in MESSAGE_MEMBERS.iter().filter(|name| **name != case.member) {
            assert_eq!(
                answer.body.get(*other),
                None,
                "case {:?} on {}: the shape declares no {other} member, got {}",
                case.label,
                case.path,
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_success_body_carries_no_message_member_at_all() {
    // The rule is about the ERROR document: a success is serialized from the operation's
    // output shape, which declares no message member, so nothing is added to it.
    let emulator = seeded().await;

    for (path, request) in [
        (
            "/get-custom-action",
            json!({"CustomActionArn": custom_action_arn(PRESENT)}),
        ),
        (
            "/list-tags-for-resource",
            json!({"ResourceARN": custom_action_arn(PRESENT)}),
        ),
        (
            "/delete-custom-action",
            json!({"CustomActionArn": custom_action_arn(PRESENT)}),
        ),
    ] {
        let answer = emulator.call(path, &request).await;
        assert_eq!(
            answer.status, 200,
            "case {path:?}: expected HTTP 200, got {} with {}",
            answer.status, answer.body
        );
        for member in MESSAGE_MEMBERS {
            assert_eq!(
                answer.body.get(member),
                None,
                "case {path:?}: a success body carries no {member} member, got {}",
                answer.body
            );
        }
    }
}

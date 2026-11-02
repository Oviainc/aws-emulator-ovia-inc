// SPDX-License-Identifier: CC0-1.0

//! What a REFUSAL of a chatbot operation looks like on the wire (restJson1), through the
//! same `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: a refusal answers the error the model declares
//! for that operation's failure — its HTTP status, its `x-amzn-errortype` code and its
//! message member, spelled as the model spells it (`Message` on
//! `ResourceNotFoundException`, `message` on `ConflictException`/`UnauthorizedException`;
//! `smithy_query` shape=ResourceNotFoundException,ConflictException,UnauthorizedException) —
//! and the message reads as AWS Chatbot words that error class, with its variable parts taken
//! from the request and the account serving it. Addressing a resource this account's store
//! holds nothing under is therefore a 404 `ResourceNotFoundException`; only an ARN outside
//! the form this account's custom actions are minted under (a region segment filled in,
//! another account's id) is the authorization failure, decided before any lookup.
//!
//! The tables span the class rather than one recorded request: the not-found rows cross
//! every operation that addresses a resource by ARN with ARNs that differ in resource kind,
//! name length and charset, region segment and account, and the conflict rows cover every
//! implemented create.

use serde_json::{json, Value};

use crate::wire::{Emulator, OTHER_REGION, REGION};
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins what a client actually sees.
const ACCOUNT: &str = "000000000000";

/// The Slack workspace the channel configurations below are created in. A workspace has to
/// be authorized with the account before a channel can be configured in it, and no
/// operation in this model performs that handshake.
const SLACK_TEAM_ID: &str = "T0123ABCD";
const SLACK_TEAM_NAME: &str = "Primary Workspace";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

/// The name every seeded resource is created under, so a conflict row can collide with it.
const PRESENT: &str = "present";

const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";
const CONFLICT_EXCEPTION: &str = "ConflictException";
const UNAUTHORIZED_EXCEPTION: &str = "UnauthorizedException";

/// The fixed text an authorization refusal carries (`state::UNAUTHORIZED_MESSAGE`).
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// A custom action is account-scoped, so the ARN a create mints carries an EMPTY region
/// segment (`state::custom_action_arn`).
fn custom_action_arn(action_name: &str) -> String {
    format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{action_name}")
}

/// A chat configuration is regional, so its ARN carries the region it lives in and the chat
/// client it configures (`state::chat_configuration_arn`).
fn chat_configuration_arn(region: &str, kind: &str, name: &str) -> String {
    format!("arn:aws:chatbot:{region}:{ACCOUNT}:chat-configuration/{kind}/{name}")
}

/// A create request for a Slack channel configuration named `name`, carrying only the
/// required members.
fn slack_create(name: &str) -> Value {
    json!({
        "ConfigurationName": name,
        "SlackTeamId": SLACK_TEAM_ID,
        "SlackChannelId": "C0123ABCD",
        "IamRoleArn": IAM_ROLE_ARN,
    })
}

/// A create request for an Amazon Chime webhook configuration named `name`.
fn chime_create(name: &str) -> Value {
    json!({
        "ConfigurationName": name,
        "WebhookDescription": "chatbot emulator webhook",
        "WebhookUrl": "https://hooks.chime.aws/incomingwebhooks/abc-123?token=tok-456",
        "SnsTopicArns": [],
        "IamRoleArn": IAM_ROLE_ARN,
    })
}

/// A create request for a custom action named `name`.
fn custom_action_create(name: &str) -> Value {
    json!({
        "ActionName": name,
        "Definition": {"CommandText": "s3 ls"},
    })
}

/// This service mounted alone, holding one resource of every kind — a custom action, a
/// Slack channel configuration and an Amazon Chime webhook configuration — all named
/// [`PRESENT`], so a row can address something that exists as well as something that does
/// not.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    chatbot::state::authorize_slack_workspace(&*emulator.engine, SLACK_TEAM_ID, SLACK_TEAM_NAME)
        .expect("the shared engine holds the chatbot store");

    for (path, request) in [
        ("/create-custom-action", custom_action_create(PRESENT)),
        ("/create-slack-channel-configuration", slack_create(PRESENT)),
        ("/create-chime-webhook-configuration", chime_create(PRESENT)),
    ] {
        let created = emulator.call(path, &request).await;
        assert_eq!(
            created.status, 200,
            "seeding {path}: expected 200, got {} with {}",
            created.status, created.body
        );
    }

    emulator
}

/// One row of a refusal table: how the request is addressed, and the whole refusal it must
/// answer.
struct Case {
    label: String,
    path: &'static str,
    request: Value,
    status: u16,
    code: &'static str,
    /// The error's message member, as the model spells it for this error shape.
    message_member: &'static str,
    message: String,
}

/// Drive every row and assert the whole refusal: status, `x-amzn-errortype`, and the
/// message member the error shape declares.
async fn assert_refusals(emulator: &Emulator, cases: &[Case]) {
    for case in cases {
        let answer = emulator.call(case.path, &case.request).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?} on {}: expected HTTP {}, got {} with {}",
            case.label, case.path, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(case.code),
            "case {:?} on {}: expected x-amzn-errortype {}, got {:?} with {}",
            case.label,
            case.path,
            case.code,
            answer.error_type,
            answer.body
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
        let actual = match case.message_member {
            "Message" => answer.message_capitalized(),
            _ => answer.message(),
        };
        assert_eq!(
            actual, case.message,
            "case {:?} on {}: expected {} {:?}, got {actual:?} in {}",
            case.label, case.path, case.message_member, case.message, answer.body
        );
    }
}

/// The ARNs of the MINTED form (`state::custom_action_arn`: account-scoped, so no region
/// segment) that name no custom action: they differ in the name's length and charset, so no
/// row's refusal can follow from the spelling of the name.
fn unknown_custom_action_arns() -> Vec<(String, String)> {
    [
        custom_action_arn("never-created"),
        custom_action_arn("a"),
        custom_action_arn(&"z".repeat(64)),
        custom_action_arn("Mixed_Case-9"),
    ]
    .into_iter()
    .map(|arn| {
        let action_name = arn
            .rsplit('/')
            .next()
            .expect("the ARN names an action")
            .to_string();
        (arn, action_name)
    })
    .collect()
}

/// Well-formed `CustomActionArn`s (the modelled pattern
/// `^arn:aws:chatbot:…:custom-action/[a-zA-Z0-9_-]{1,64}$`) that NO create can mint for this
/// account, because a custom action is account-scoped: a region segment filled in, and
/// another account's id. They name the action that does exist, so what is refused is the
/// ARN's form and not the action's absence.
fn unmintable_custom_action_arns() -> Vec<String> {
    vec![
        format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:custom-action/{PRESENT}"),
        format!("arn:aws:chatbot:{OTHER_REGION}:{ACCOUNT}:custom-action/never-created"),
        format!("arn:aws:chatbot::999999999999:custom-action/{PRESENT}"),
    ]
}

#[tokio::test]
async fn a_custom_action_operation_refuses_an_unheld_arn_as_not_found() {
    let emulator = seeded().await;

    // A custom-action operation words its not-found around the ACTION: the name comes off
    // the requested ARN and the account from the account serving the request. The read and
    // the update are two error classes, so each has its own sentence.
    let mut cases = Vec::new();
    for (arn, action_name) in unknown_custom_action_arns() {
        cases.push(Case {
            label: format!("read {arn}"),
            path: "/get-custom-action",
            request: json!({"CustomActionArn": arn}),
            status: 404,
            code: RESOURCE_NOT_FOUND_EXCEPTION,
            message_member: "Message",
            message: format!("sdk action [{action_name}] not found for account [{ACCOUNT}]"),
        });
        cases.push(Case {
            label: format!("update {arn}"),
            path: "/update-custom-action",
            request: json!({
                "CustomActionArn": arn,
                "Definition": {"CommandText": "s3 ls"},
            }),
            status: 404,
            code: RESOURCE_NOT_FOUND_EXCEPTION,
            message_member: "Message",
            message: format!(
                "Sdk action with id [{action_name}] does not exist in account [{ACCOUNT}]"
            ),
        });
    }

    assert_refusals(&emulator, &cases).await;

    // The action that IS held is untouched by the refusals.
    let held = emulator
        .call(
            "/get-custom-action",
            &json!({"CustomActionArn": custom_action_arn(PRESENT)}),
        )
        .await;
    assert_eq!(
        held.status, 200,
        "the held action must still read back, got {} with {}",
        held.status, held.body
    );
}

#[tokio::test]
async fn an_arn_no_custom_action_of_this_account_can_be_minted_under_is_unauthorized() {
    let emulator = seeded().await;

    // A custom action is account-scoped, so the only ARN that can name one of this account's
    // actions is the region-less ARN the create mints. An ARN of any other form names no
    // target in this account's namespace, so every operation addressed by it is refused as an
    // authorization failure — decided before any lookup, which is why the message names
    // neither the ARN nor the action, and why the row naming an action that DOES exist is
    // refused just the same.
    let mut cases = Vec::new();
    for arn in unmintable_custom_action_arns() {
        for (path, request) in [
            ("/get-custom-action", json!({"CustomActionArn": arn})),
            (
                "/update-custom-action",
                json!({"CustomActionArn": arn, "Definition": {"CommandText": "s3 ls"}}),
            ),
            ("/list-tags-for-resource", json!({"ResourceARN": arn})),
            (
                "/tag-resource",
                json!({"ResourceARN": arn, "Tags": [{"TagKey": "k", "TagValue": "v"}]}),
            ),
            (
                "/untag-resource",
                json!({"ResourceARN": arn, "TagKeys": ["k"]}),
            ),
        ] {
            cases.push(Case {
                label: format!("{path} on {arn}"),
                path,
                request,
                status: 403,
                code: UNAUTHORIZED_EXCEPTION,
                message_member: "message",
                message: UNAUTHORIZED_MESSAGE.to_string(),
            });
        }
    }

    assert_refusals(&emulator, &cases).await;

    // None of those refusals touched the action the minted ARN names.
    let held = emulator
        .call(
            "/list-tags-for-resource",
            &json!({"ResourceARN": custom_action_arn(PRESENT)}),
        )
        .await;
    assert_eq!(
        held.status, 200,
        "the held action's tags must still read back, got {} with {}",
        held.status, held.body
    );
}

#[tokio::test]
async fn a_tagging_operation_refuses_an_unheld_arn_as_not_found_naming_the_arn() {
    let emulator = seeded().await;

    // A tagging operation takes any chatbot ARN, and words its not-found around that ARN
    // alone — the same sentence whichever kind of resource the ARN names, including an ARN
    // whose resource type this service holds nothing of at all.
    let unknown = [
        custom_action_arn("never-created"),
        chat_configuration_arn(REGION, "slack-channel", "never-created"),
        chat_configuration_arn(REGION, "chime-webhook", "never-created"),
        chat_configuration_arn(OTHER_REGION, "slack-channel", PRESENT),
        chat_configuration_arn(OTHER_REGION, "chime-webhook", PRESENT),
        format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:something-else/{PRESENT}"),
    ];

    let mut cases = Vec::new();
    for arn in unknown {
        let message = format!("Resource {arn} does not exist!");
        for (path, request) in [
            ("/list-tags-for-resource", json!({"ResourceARN": arn})),
            (
                "/tag-resource",
                json!({"ResourceARN": arn, "Tags": [{"TagKey": "k", "TagValue": "v"}]}),
            ),
            (
                "/untag-resource",
                json!({"ResourceARN": arn, "TagKeys": ["k"]}),
            ),
        ] {
            cases.push(Case {
                label: format!("{path} on {arn}"),
                path,
                request,
                status: 404,
                code: RESOURCE_NOT_FOUND_EXCEPTION,
                message_member: "Message",
                message: message.clone(),
            });
        }
    }

    assert_refusals(&emulator, &cases).await;
}

#[tokio::test]
async fn a_slack_channel_configuration_words_delete_and_update_not_found_differently() {
    let emulator = seeded().await;

    // The two operations are two error classes, so one shared sentence cannot serve both:
    // the delete speaks of the Slack destination behind the channel, the update of the chat
    // configuration itself.
    let unknown = [
        chat_configuration_arn(REGION, "slack-channel", "never-created"),
        chat_configuration_arn(OTHER_REGION, "slack-channel", PRESENT),
        chat_configuration_arn(REGION, "chime-webhook", PRESENT),
    ];

    let mut cases = Vec::new();
    for arn in unknown {
        cases.push(Case {
            label: format!("delete {arn}"),
            path: "/delete-slack-channel-configuration",
            request: json!({"ChatConfigurationArn": arn}),
            status: 404,
            code: RESOURCE_NOT_FOUND_EXCEPTION,
            message_member: "Message",
            message: format!("No Slack Destination exists for Wheatley Channel: {arn}."),
        });
        cases.push(Case {
            label: format!("update {arn}"),
            path: "/update-slack-channel-configuration",
            request: json!({"ChatConfigurationArn": arn, "SlackChannelId": "C0123ABCD"}),
            status: 404,
            code: RESOURCE_NOT_FOUND_EXCEPTION,
            message_member: "Message",
            message: format!("Chat Configuration with ARN {arn} not found."),
        });
    }

    assert_refusals(&emulator, &cases).await;

    // The configuration that IS held is still there: the refusals deleted nothing.
    let held_arn = chat_configuration_arn(REGION, "slack-channel", PRESENT);
    let deleted = emulator
        .call(
            "/delete-slack-channel-configuration",
            &json!({"ChatConfigurationArn": held_arn}),
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "the held configuration must still be deletable, got {} with {}",
        deleted.status, deleted.body
    );
}

#[tokio::test]
async fn a_create_for_a_resource_that_already_exists_conflicts_naming_its_arn() {
    let emulator = seeded().await;

    // Every create identifies the colliding resource by its ARN rather than by the name the
    // request carried, so the caller is handed the ARN it can address the existing resource
    // through.
    let cases = vec![
        Case {
            label: "a custom action name the account already holds".to_string(),
            path: "/create-custom-action",
            request: custom_action_create(PRESENT),
            status: 409,
            code: CONFLICT_EXCEPTION,
            message_member: "message",
            message: format!(
                "Resource with arn {} already exists.",
                custom_action_arn(PRESENT)
            ),
        },
        Case {
            label: "a Slack channel configuration name the region already holds".to_string(),
            path: "/create-slack-channel-configuration",
            request: slack_create(PRESENT),
            status: 409,
            code: CONFLICT_EXCEPTION,
            message_member: "message",
            message: format!(
                "Resource with arn {} already exists.",
                chat_configuration_arn(REGION, "slack-channel", PRESENT)
            ),
        },
        Case {
            label: "a Chime webhook configuration name the region already holds".to_string(),
            path: "/create-chime-webhook-configuration",
            request: chime_create(PRESENT),
            status: 409,
            code: CONFLICT_EXCEPTION,
            message_member: "message",
            message: format!(
                "Resource with arn {} already exists.",
                chat_configuration_arn(REGION, "chime-webhook", PRESENT)
            ),
        },
    ];

    assert_refusals(&emulator, &cases).await;

    // The same names in ANOTHER region are free for the regional resources, so the conflict
    // above was about the ARN and not about the name alone.
    for (path, request) in [
        ("/create-slack-channel-configuration", slack_create(PRESENT)),
        ("/create-chime-webhook-configuration", chime_create(PRESENT)),
    ] {
        let created = emulator.call_in_region(OTHER_REGION, path, &request).await;
        assert_eq!(
            created.status, 200,
            "case {path:?} in {OTHER_REGION}: expected 200, got {} with {}",
            created.status, created.body
        );
    }
}

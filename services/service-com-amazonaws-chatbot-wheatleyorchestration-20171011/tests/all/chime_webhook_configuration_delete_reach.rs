// SPDX-License-Identifier: CC0-1.0

//! How far a `DeleteChimeWebhookConfiguration` REACHES, driven over the wire (restJson1)
//! through the same `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: a delete removes the Amazon Chime webhook
//! configuration the `ChatConfigurationArn` resolves to and NOTHING else — so afterwards
//! every ARN-addressed reader of that configuration (the delete itself, the update, and
//! each tagging operation) refuses that ARN with the modelled 404
//! `ResourceNotFoundException`, the listing simply no longer reports it, and no resource
//! stored under another ARN — a second Chime webhook configuration, a Slack channel
//! configuration of the same name, a custom action of this account — is touched; an ARN that
//! resolves to a resource of another kind holds no Chime webhook configuration, so the
//! delete refuses it and leaves that resource standing.
//!
//! `tests/all/chime_webhook_configuration_delete.rs` covers what the delete removes; this
//! suite covers what it must NOT remove and what every reader answers once it has. The rows
//! span what the rule generalises over: the configuration name's length (each published
//! bound included) and charset, the optional members and tags the deleted configuration
//! carried, which reader is asked afterwards, and which other kind of chatbot resource
//! stands beside it.

use serde_json::{json, Value};

use crate::wire::{Emulator, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

const CREATE_PATH: &str = "/create-chime-webhook-configuration";
const DELETE_PATH: &str = "/delete-chime-webhook-configuration";
const UPDATE_PATH: &str = "/update-chime-webhook-configuration";
const DESCRIBE_PATH: &str = "/describe-chime-webhook-configurations";
const LIST_TAGS_PATH: &str = "/list-tags-for-resource";
const TAG_PATH: &str = "/tag-resource";
const UNTAG_PATH: &str = "/untag-resource";

const CREATE_SLACK_PATH: &str = "/create-slack-channel-configuration";
const DESCRIBE_SLACK_PATH: &str = "/describe-slack-channel-configurations";
const CREATE_CUSTOM_ACTION_PATH: &str = "/create-custom-action";
const GET_CUSTOM_ACTION_PATH: &str = "/get-custom-action";

/// The modelled error an ARN the Chime webhook store holds no configuration under is
/// refused with.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The Slack workspace the Slack channel configurations below are created in. A workspace
/// is authorized through the one-time Slack client handshake the console drives, which no
/// operation of this model performs, so the suite registers it through the state seam.
const SLACK_TEAM_ID: &str = "T0123ABCD";
const SLACK_TEAM_NAME: &str = "Primary Workspace";

/// A chat configuration's ARN carries the region it was created in and the chat client it
/// configures (`state::chat_configuration_arn`).
fn chat_configuration_arn(kind: &str, configuration_name: &str) -> String {
    format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/{kind}/{configuration_name}")
}

/// A custom action's ARN is account-scoped, so its region segment is empty
/// (`state::custom_action_arn`).
fn custom_action_arn(action_name: &str) -> String {
    format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{action_name}")
}

/// A `WebhookUrl` satisfying the published pattern (`smithy_query` ChimeWebhookUrl:
/// `^https://hooks\.chime\.aws/incomingwebhooks/[A-Za-z0-9\-]+?\?token=[A-Za-z0-9\-]+$`).
fn webhook_url(id: usize) -> String {
    format!("https://hooks.chime.aws/incomingwebhooks/hook-{id}?token=tok-{id}")
}

/// The `Message` AWS Chatbot answers for an ARN that resolves to no resource: the ARN from
/// the request inside fixed text.
fn does_not_exist_message(arn: &str) -> String {
    format!("Resource {arn} does not exist!")
}

/// One configuration the suite deletes, spanning the name lengths and the optional members
/// the rule generalises over: `ConfigurationName` is len 1..128 (`smithy_query`
/// shape=ConfigurationName), both bounds included below, and `LoggingLevel` and `Tags` are
/// optional on the create.
struct Row {
    label: &'static str,
    /// The row's position, which the names of the bystanders standing beside the deleted
    /// configuration vary along.
    index: usize,
    configuration_name: String,
    create: Value,
}

fn rows() -> Vec<Row> {
    let names = [
        "deleted-webhook".to_string(),
        "a".to_string(),
        "n".repeat(128),
        "Mixed_Case-9".to_string(),
    ];
    let extras = [
        json!({}),
        json!({"LoggingLevel": "ERROR"}),
        json!({"LoggingLevel": "NONE", "Tags": [{"TagKey": "env", "TagValue": "dev"}]}),
        json!({
            "LoggingLevel": "INFO",
            "Tags": [
                {"TagKey": "env", "TagValue": "prod"},
                {"TagKey": "team", "TagValue": "ops"},
            ],
        }),
    ];
    let labels = [
        "only the required members",
        "the shortest name, and a logging level",
        "the longest name, a logging level and one tag",
        "a mixed-case name, a logging level and two tags",
    ];

    names
        .into_iter()
        .enumerate()
        .zip(extras)
        .zip(labels)
        .map(|(((index, name), extras), label)| {
            let mut create = json!({
                "ConfigurationName": name,
                "WebhookDescription": format!("Room/{index}"),
                "WebhookUrl": webhook_url(index),
                "SnsTopicArns": (0..index)
                    .map(|topic| format!("arn:aws:sns:{REGION}:{ACCOUNT}:topic-{topic}"))
                    .collect::<Vec<_>>(),
                "IamRoleArn": IAM_ROLE_ARN,
            });
            for (member, value) in extras.as_object().expect("an object of extra members") {
                create[member] = value.clone();
            }
            Row {
                label,
                index,
                configuration_name: name,
                create,
            }
        })
        .collect()
}

/// POST `request` to `path` and assert it answered 200, handing back the body.
async fn ok(emulator: &Emulator, path: &str, request: &Value) -> Value {
    let answer = emulator.call(path, request).await;
    assert_eq!(
        answer.status, 200,
        "{path} {request}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body
}

/// The configurations a Chime describe answers for `request`.
async fn describe_chime(emulator: &Emulator, request: &Value) -> Vec<Value> {
    ok(emulator, DESCRIBE_PATH, request).await["WebhookConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("describe {request}: no WebhookConfigurations"))
        .clone()
}

/// Assert `path` refuses `request` with the modelled 404 naming `arn`.
async fn assert_not_found(emulator: &Emulator, case: &str, path: &str, request: &Value, arn: &str) {
    let answer = emulator.call(path, request).await;
    assert_eq!(
        answer.status, 404,
        "case {case:?} at {path}: expected HTTP 404, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "case {case:?} at {path}: expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION}, got {}",
        answer.body
    );
    assert_eq!(
        answer.body.get("Message").and_then(Value::as_str),
        Some(does_not_exist_message(arn).as_str()),
        "case {case:?} at {path}: the not-found Message must name the requested ARN, got {}",
        answer.body
    );
}

#[tokio::test]
async fn every_arn_addressed_reader_refuses_a_deleted_configuration() {
    for row in rows() {
        let emulator = Emulator::new();
        let arn = ok(&emulator, CREATE_PATH, &row.create).await["WebhookConfiguration"]
            ["ChatConfigurationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the create minted no ARN", row.label))
            .to_string();

        ok(&emulator, DELETE_PATH, &json!({"ChatConfigurationArn": arn})).await;

        // Every operation that addresses a configuration BY ARN now resolves nothing under
        // it, so each refuses with the modelled 404 — the delete included, which is why a
        // second delete is not a silent success.
        let readers = [
            (
                "the delete again",
                DELETE_PATH,
                json!({"ChatConfigurationArn": arn}),
            ),
            (
                "the update",
                UPDATE_PATH,
                json!({"ChatConfigurationArn": arn, "LoggingLevel": "ERROR"}),
            ),
            (
                "the tag listing",
                LIST_TAGS_PATH,
                json!({"ResourceARN": arn}),
            ),
            (
                "a tag write",
                TAG_PATH,
                json!({"ResourceARN": arn, "Tags": [{"TagKey": "env", "TagValue": "dev"}]}),
            ),
            (
                "a tag removal",
                UNTAG_PATH,
                json!({"ResourceARN": arn, "TagKeys": ["env"]}),
            ),
        ];
        for (reader, path, request) in &readers {
            let case = format!("{} / {reader}", row.label);
            assert_not_found(&emulator, &case, path, request, &arn).await;
        }

        // The LISTING has no ARN to refuse: a configuration that is gone is simply not
        // reported, by its ARN or unfiltered — and none of the refusals above put anything
        // back.
        assert_eq!(
            describe_chime(&emulator, &json!({"ChatConfigurationArn": arn})).await,
            Vec::<Value>::new(),
            "case {:?}: a describe by the deleted ARN must report nothing",
            row.label
        );
        assert_eq!(
            describe_chime(&emulator, &json!({})).await,
            Vec::<Value>::new(),
            "case {:?}: the store must hold no configuration after the delete",
            row.label
        );
    }
}

#[tokio::test]
async fn a_chime_delete_leaves_every_other_kind_of_resource_standing() {
    for row in rows() {
        let emulator = Emulator::new();
        service_com_amazonaws_chatbot_wheatleyorchestration_20171011::state::authorize_slack_workspace(
            &*emulator.engine,
            SLACK_TEAM_ID,
            SLACK_TEAM_NAME,
        )
        .expect("the shared engine holds the chatbot store");

        // The SAME name under two other kinds of chatbot resource: a Slack channel
        // configuration (another chat-configuration ARN) and a custom action (an
        // account-scoped ARN). Neither is a Chime webhook configuration.
        let slack_arn = ok(
            &emulator,
            CREATE_SLACK_PATH,
            &json!({
                "ConfigurationName": row.configuration_name,
                "SlackTeamId": SLACK_TEAM_ID,
                "SlackChannelId": "C0123ABCD",
                "IamRoleArn": IAM_ROLE_ARN,
            }),
        )
        .await["ChannelConfiguration"]["ChatConfigurationArn"]
            .as_str()
            .expect("the Slack create minted an ARN")
            .to_string();
        assert_eq!(
            slack_arn,
            chat_configuration_arn("slack-channel", &row.configuration_name),
            "case {:?}: the Slack configuration's ARN names its own chat client",
            row.label
        );
        // The custom action and the Chime bystander vary with the row, so "nothing else is
        // touched" is asserted over more than one arrangement of the store.
        let action_name = format!("shared-action-{}", row.index);
        ok(
            &emulator,
            CREATE_CUSTOM_ACTION_PATH,
            &json!({
                "ActionName": action_name,
                "Definition": {"CommandText": format!("s3 ls s3://bucket-{}", row.index)},
            }),
        )
        .await;

        // A second Chime webhook configuration, in the same store the delete removes from.
        let bystander_arn = ok(
            &emulator,
            CREATE_PATH,
            &json!({
                "ConfigurationName": format!("chime-bystander-{}", row.index),
                "WebhookDescription": "Room/Bystander",
                "WebhookUrl": webhook_url(99),
                "SnsTopicArns": [],
                "IamRoleArn": IAM_ROLE_ARN,
            }),
        )
        .await["WebhookConfiguration"]["ChatConfigurationArn"]
            .as_str()
            .expect("the bystander create minted an ARN")
            .to_string();

        let chime_arn = ok(&emulator, CREATE_PATH, &row.create).await["WebhookConfiguration"]
            ["ChatConfigurationArn"]
            .as_str()
            .expect("the Chime create minted an ARN")
            .to_string();

        ok(
            &emulator,
            DELETE_PATH,
            &json!({"ChatConfigurationArn": chime_arn}),
        )
        .await;

        // The deleted configuration is gone; the resources under every other ARN are not,
        // the Chime bystander in the same store included.
        assert_eq!(
            describe_chime(&emulator, &json!({}))
                .await
                .iter()
                .map(|configuration| configuration["ChatConfigurationArn"].clone())
                .collect::<Vec<_>>(),
            vec![json!(bystander_arn)],
            "case {:?}: only the Chime bystander may remain after the delete",
            row.label
        );
        let slack = ok(&emulator, DESCRIBE_SLACK_PATH, &json!({})).await;
        assert_eq!(
            slack["SlackChannelConfigurations"]
                .as_array()
                .map(|configurations| configurations
                    .iter()
                    .map(|configuration| configuration["ChatConfigurationArn"].clone())
                    .collect::<Vec<_>>()),
            Some(vec![json!(slack_arn)]),
            "case {:?}: the Slack channel configuration must survive the Chime delete, got {slack}",
            row.label
        );
        let action = ok(
            &emulator,
            GET_CUSTOM_ACTION_PATH,
            &json!({"CustomActionArn": custom_action_arn(&action_name)}),
        )
        .await;
        assert_eq!(
            action["CustomAction"]["CustomActionArn"],
            json!(custom_action_arn(&action_name)),
            "case {:?}: the custom action must survive the Chime delete, got {action}",
            row.label
        );

        // Asking the Chime delete for one of those other ARNs resolves no Chime webhook
        // configuration, so it refuses and removes nothing.
        let other_kinds = [
            ("a Slack channel configuration's ARN", slack_arn.clone()),
            ("a custom action's ARN", custom_action_arn(&action_name)),
        ];
        for (kind, arn) in &other_kinds {
            let case = format!("{} / {kind}", row.label);
            assert_not_found(
                &emulator,
                &case,
                DELETE_PATH,
                &json!({"ChatConfigurationArn": arn}),
                arn,
            )
            .await;
        }
        let slack_after = ok(&emulator, DESCRIBE_SLACK_PATH, &json!({})).await;
        assert_eq!(
            slack_after, slack,
            "case {:?}: the refused deletes must leave the Slack configuration exactly as it was",
            row.label
        );
        let action_after = ok(
            &emulator,
            GET_CUSTOM_ACTION_PATH,
            &json!({"CustomActionArn": custom_action_arn(&action_name)}),
        )
        .await;
        assert_eq!(
            action_after, action,
            "case {:?}: the refused deletes must leave the custom action exactly as it was",
            row.label
        );
        assert_eq!(
            describe_chime(&emulator, &json!({"ChatConfigurationArn": bystander_arn}))
                .await
                .len(),
            1,
            "case {:?}: the refused deletes must leave the Chime bystander in place",
            row.label
        );
    }
}

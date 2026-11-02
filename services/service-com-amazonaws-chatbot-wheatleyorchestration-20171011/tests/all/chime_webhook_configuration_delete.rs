// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteChimeWebhookConfiguration` and the read that proves what
//! it removed (`DescribeChimeWebhookConfigurations`), over the wire (restJson1) through the
//! same `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: a delete REMOVES the Amazon Chime webhook
//! configuration from the store, so a later describe no longer reports it — by ARN or
//! unfiltered — the record's tags go with it, and the configuration name is free in its
//! region again. A configuration is addressed by its ARN, which carries its region, so the
//! delete reaches it from a request signed for any region and leaves every other
//! configuration (including the same name in another region) alone; and an ARN the store
//! holds no Chime webhook configuration under is refused with the modelled 404
//! `ResourceNotFoundException` naming that ARN, having removed nothing.
//!
//! The table spans what the rule generalises over rather than one recorded request: the
//! configuration names differ in length (each published bound included) and charset, and the
//! deleted configurations differ in which optional members and how many tags they carry.

use serde_json::{json, Value};

use crate::wire::{Emulator, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

const CREATE_PATH: &str = "/create-chime-webhook-configuration";
const DELETE_PATH: &str = "/delete-chime-webhook-configuration";
const DESCRIBE_PATH: &str = "/describe-chime-webhook-configurations";
const LIST_TAGS_PATH: &str = "/list-tags-for-resource";

/// The modelled error an ARN the store holds no configuration under is refused with.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// A chat configuration's ARN carries the region it was created in
/// (`state::chime_webhook_configuration_arn`).
fn arn_in_region(region: &str, configuration_name: &str) -> String {
    format!(
        "arn:aws:chatbot:{region}:{ACCOUNT}:chat-configuration/chime-webhook/{configuration_name}"
    )
}

/// A `WebhookUrl` satisfying the published pattern (`smithy_query` ChimeWebhookUrl:
/// `^https://hooks\.chime\.aws/incomingwebhooks/[A-Za-z0-9\-]+?\?token=[A-Za-z0-9\-]+$`),
/// whose path segment therefore carries neither `_` nor an upper-case-only name.
fn webhook_url(id: usize) -> String {
    format!("https://hooks.chime.aws/incomingwebhooks/hook-{id}?token=tok-{id}")
}

/// The `Message` AWS Chatbot answers for an ARN that resolves to no resource: the ARN from
/// the request inside fixed text.
fn does_not_exist_message(arn: &str) -> String {
    format!("Resource {arn} does not exist!")
}

/// One row of the delete table: the create whose configuration is then deleted.
struct DeleteCase {
    label: &'static str,
    configuration_name: String,
    create: Value,
}

/// The delete cases, each differing from the others along what the delete rule generalises
/// over: the configuration name's length (`ConfigurationName`, len 1..128 — both published
/// bounds included) and charset, which optional members (`LoggingLevel`, `Tags`) the deleted
/// configuration carries, and how many SNS topics it names.
fn delete_cases() -> Vec<DeleteCase> {
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
        "a logging level",
        "a logging level and one tag",
        "a logging level and two tags",
    ];

    names
        .iter()
        .enumerate()
        .zip(extras)
        .zip(labels)
        .map(|(((index, name), extras), label)| {
            let mut create = json!({
                "ConfigurationName": name,
                "WebhookDescription": format!("Room/{index}"),
                "WebhookUrl": webhook_url(index),
                "SnsTopicArns": (0..index)
                    .map(|topic| format!("arn:aws:sns:us-east-1:{ACCOUNT}:topic-{topic}"))
                    .collect::<Vec<_>>(),
                "IamRoleArn": IAM_ROLE_ARN,
            });
            for (member, value) in extras.as_object().expect("an object of extra members") {
                create[member] = value.clone();
            }
            DeleteCase {
                label,
                configuration_name: name.clone(),
                create,
            }
        })
        .collect()
}

/// Create the configuration `request` names and answer its `ChatConfigurationArn`.
async fn create_ok(emulator: &Emulator, request: &Value) -> String {
    let created = emulator.call(CREATE_PATH, request).await;
    assert_eq!(
        created.status, 200,
        "create {request}: expected 200, got {} with {}",
        created.status, created.body
    );
    created.body["WebhookConfiguration"]["ChatConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("create {request}: no ChatConfigurationArn in {}", created.body))
        .to_string()
}

/// The configurations a describe answers for `request`.
async fn describe(emulator: &Emulator, request: &Value) -> Vec<Value> {
    let answer = emulator.call(DESCRIBE_PATH, request).await;
    assert_eq!(
        answer.status, 200,
        "describe {request}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["WebhookConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("describe {request}: no WebhookConfigurations in {}", answer.body))
        .clone()
}

#[tokio::test]
async fn deleting_a_configuration_removes_it_and_leaves_every_other_one_alone() {
    for case in delete_cases() {
        // A fresh store per row, so each row deletes out of the same arrangement.
        let emulator = Emulator::new();
        let arn = create_ok(&emulator, &case.create).await;
        assert_eq!(
            arn,
            arn_in_region(REGION, &case.configuration_name),
            "case {:?}: the create must mint the region-qualified ARN, got {arn}",
            case.label
        );

        // A bystander in this region and the SAME name in another region: a delete is
        // addressed by ARN, so neither may go with it.
        let bystander = create_ok(
            &emulator,
            &json!({
                "ConfigurationName": "bystander",
                "WebhookDescription": "Room/bystander",
                "WebhookUrl": webhook_url(99),
                "SnsTopicArns": [],
                "IamRoleArn": IAM_ROLE_ARN,
            }),
        )
        .await;
        let elsewhere = emulator
            .call_in_region(OTHER_REGION, CREATE_PATH, &case.create)
            .await;
        assert_eq!(
            elsewhere.status, 200,
            "case {:?}: the same name in {OTHER_REGION} is another configuration, got {} with {}",
            case.label, elsewhere.status, elsewhere.body
        );

        let deleted = emulator
            .call(DELETE_PATH, &json!({"ChatConfigurationArn": arn}))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: the delete answers 200, got {} with {}",
            case.label, deleted.status, deleted.body
        );

        assert_eq!(
            describe(&emulator, &json!({"ChatConfigurationArn": arn})).await,
            Vec::<Value>::new(),
            "case {:?}: the deleted configuration must be gone from a describe by its ARN",
            case.label
        );
        let remaining = describe(&emulator, &json!({})).await;
        assert_eq!(
            remaining
                .iter()
                .map(|configuration| configuration["ChatConfigurationArn"].clone())
                .collect::<Vec<_>>(),
            vec![json!(bystander)],
            "case {:?}: only the bystander may remain in {REGION}, got {remaining:?}",
            case.label
        );

        // The same name in the other region is a different configuration, so it stands.
        let elsewhere = emulator
            .call_in_region(OTHER_REGION, DESCRIBE_PATH, &json!({}))
            .await;
        assert_eq!(
            elsewhere.body["WebhookConfigurations"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "case {:?}: the configuration in {OTHER_REGION} must stand, got {}",
            case.label,
            elsewhere.body
        );
    }
}

#[tokio::test]
async fn a_deleted_configuration_frees_its_name_with_its_tags() {
    for case in delete_cases() {
        let emulator = Emulator::new();
        let arn = create_ok(&emulator, &case.create).await;

        let deleted = emulator
            .call(DELETE_PATH, &json!({"ChatConfigurationArn": arn}))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: the delete answers 200, got {} with {}",
            case.label, deleted.status, deleted.body
        );

        // The name is free again: the re-created configuration gets the same ARN and
        // carries none of the deleted one's state — the record owned its tags, so they
        // went with it.
        let recreated = create_ok(
            &emulator,
            &json!({
                "ConfigurationName": case.configuration_name,
                "WebhookDescription": "Recreated/Webhook",
                "WebhookUrl": webhook_url(7),
                "SnsTopicArns": [],
                "IamRoleArn": IAM_ROLE_ARN,
            }),
        )
        .await;
        assert_eq!(
            recreated, arn,
            "case {:?}: re-creating the deleted name must mint the same ARN, got {recreated}",
            case.label
        );
        let described = describe(&emulator, &json!({"ChatConfigurationArn": arn})).await;
        assert_eq!(
            described,
            vec![json!({
                "ChatConfigurationArn": arn,
                "ConfigurationName": case.configuration_name,
                "WebhookDescription": "Recreated/Webhook",
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": [],
                "Tags": [],
                "State": "ENABLED",
            })],
            "case {:?}: the re-created configuration must carry only what its create named",
            case.label
        );
        let listed = emulator
            .call(LIST_TAGS_PATH, &json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            listed.body.get("Tags"),
            Some(&json!([])),
            "case {:?}: the deleted configuration's tags must not survive it, got {}",
            case.label,
            listed.body
        );
    }
}

#[tokio::test]
async fn a_delete_is_addressed_by_arn_alone_whatever_region_it_is_signed_for() {
    let emulator = Emulator::new();
    let case = &delete_cases()[0];
    let arn = create_ok(&emulator, &case.create).await;

    // The ARN carries the configuration's region, so a request signed for another region
    // reaches it just the same.
    let deleted = emulator
        .call_in_region(
            OTHER_REGION,
            DELETE_PATH,
            &json!({"ChatConfigurationArn": arn}),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "a delete signed for {OTHER_REGION} must reach the configuration in {REGION}, got {} \
         with {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        describe(&emulator, &json!({})).await,
        Vec::<Value>::new(),
        "the configuration must be gone from its own region's describe"
    );
}

#[tokio::test]
async fn deleting_an_arn_the_store_holds_no_chime_configuration_under_is_not_found() {
    let emulator = Emulator::new();
    let case = &delete_cases()[0];
    let present = create_ok(&emulator, &case.create).await;
    let stored = describe(&emulator, &json!({})).await;

    // Each row is a well-formed `ChatConfigurationArn` the Chime webhook store holds
    // nothing under: they differ in the configuration name, in the region segment and in
    // the chat client the ARN names.
    let unknown = [
        (
            "a name nothing was created under",
            arn_in_region(REGION, "absent"),
        ),
        (
            "the created name in another region",
            arn_in_region(OTHER_REGION, &case.configuration_name),
        ),
        (
            "a Slack channel configuration's ARN",
            format!(
                "arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/{}",
                case.configuration_name
            ),
        ),
    ];

    for (label, arn) in &unknown {
        let answer = emulator
            .call(DELETE_PATH, &json!({"ChatConfigurationArn": arn}))
            .await;
        assert_eq!(
            answer.status, 404,
            "case {label:?} ({arn}): expected HTTP 404, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            RESOURCE_NOT_FOUND_EXCEPTION,
            "case {label:?} ({arn}): expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION}, got {}",
            answer.body
        );
        assert_eq!(
            answer.body.get("Message").and_then(Value::as_str),
            Some(does_not_exist_message(arn).as_str()),
            "case {label:?} ({arn}): the not-found Message must name the requested ARN, got {}",
            answer.body
        );
    }

    // None of those refusals removed anything.
    assert_eq!(
        describe(&emulator, &json!({})).await,
        stored,
        "the refused deletes must leave the stored configuration in place"
    );
    assert_eq!(
        describe(&emulator, &json!({"ChatConfigurationArn": present})).await.len(),
        1,
        "the configuration that IS there must still be described"
    );
}

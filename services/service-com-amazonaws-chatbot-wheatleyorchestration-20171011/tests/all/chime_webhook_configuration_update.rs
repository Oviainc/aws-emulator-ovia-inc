// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateChimeWebhookConfiguration` and the read that proves what
//! it persisted (`DescribeChimeWebhookConfigurations`), over the wire (restJson1) through
//! the same `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: an update WRITES the members the request carries
//! through to the stored configuration and answers that stored record, so a later describe
//! reports the supplied values and not the ones the configuration was created with. Every
//! member but the ARN is optional, so a member the request omits names no new value and
//! leaves the stored one exactly as it was; the configuration's name and region (which the
//! ARN fixes) and its tags (which only the tagging operations write) are not an update's to
//! move; and an ARN the store holds no Chime webhook configuration under is refused with
//! the modelled 404 `ResourceNotFoundException` and persists nothing.
//!
//! The table spans what the rule generalises over rather than one recorded request: which
//! members the update carries (down to none at all), each published `LoggingLevel` value,
//! how many SNS topics the list holds, and the description at each published length bound.

use serde_json::{json, Map, Value};

use crate::wire::{Emulator, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";
const OTHER_IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-other-role";

const CREATE_PATH: &str = "/create-chime-webhook-configuration";
const UPDATE_PATH: &str = "/update-chime-webhook-configuration";
const DESCRIBE_PATH: &str = "/describe-chime-webhook-configurations";

/// The modelled error an ARN the store holds no configuration under is refused with.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The `Message` a Chime webhook configuration's ARN answers when it holds no
/// configuration: the ARN from the request inside fixed text.
fn does_not_exist_message(arn: &str) -> String {
    format!("Resource {arn} does not exist!")
}

/// The tag the baseline create carries, which no update may disturb.
fn baseline_tags() -> Value {
    json!([{"TagKey": "team", "TagValue": "ops"}])
}

/// A chat configuration's ARN carries the region it was created in
/// (`state::chime_webhook_configuration_arn`).
fn arn_in_region(region: &str, configuration_name: &str) -> String {
    format!(
        "arn:aws:chatbot:{region}:{ACCOUNT}:chat-configuration/chime-webhook/{configuration_name}"
    )
}

/// A `WebhookUrl` satisfying the published pattern (`smithy_query` ChimeWebhookUrl:
/// `^https://hooks\.chime\.aws/incomingwebhooks/[A-Za-z0-9\-]+?\?token=[A-Za-z0-9\-]+$`).
fn webhook_url(id: &str) -> String {
    format!("https://hooks.chime.aws/incomingwebhooks/{id}?token=tok-{id}")
}

/// The create every update row starts from: it carries EVERY member, so a row that leaves
/// one out shows the update keeping the stored value rather than clearing it.
fn baseline_create(configuration_name: &str) -> Value {
    json!({
        "ConfigurationName": configuration_name,
        "WebhookDescription": "Ops Room/Pager Webhook",
        "WebhookUrl": webhook_url(configuration_name),
        "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:alerts"],
        "IamRoleArn": IAM_ROLE_ARN,
        "LoggingLevel": "INFO",
        "Tags": baseline_tags(),
    })
}

/// Create the baseline configuration `configuration_name` and answer the
/// `ChimeWebhookConfiguration` the create reports.
async fn create_baseline(emulator: &Emulator, configuration_name: &str) -> Value {
    let created = emulator
        .call(CREATE_PATH, &baseline_create(configuration_name))
        .await;
    assert_eq!(
        created.status, 200,
        "creating {configuration_name:?} answers 200, got {} with {}",
        created.status, created.body
    );
    created.body["WebhookConfiguration"].clone()
}

/// The configuration the store holds under `arn`, read back through Describe.
async fn described(emulator: &Emulator, arn: &str) -> Value {
    let answer = emulator
        .call(DESCRIBE_PATH, &json!({"ChatConfigurationArn": arn}))
        .await;
    assert_eq!(
        answer.status, 200,
        "describing {arn} answers 200, got {} with {}",
        answer.status, answer.body
    );
    let listed = answer.body["WebhookConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("describing {arn}: no WebhookConfigurations in {}", answer.body))
        .clone();
    assert_eq!(
        listed.len(),
        1,
        "describing {arn} by ARN must answer exactly one configuration, got {}",
        answer.body
    );
    listed[0].clone()
}

/// The configuration `stored` becomes once `update`'s members are written through it: the
/// rule itself, applied to the record the describe must report.
///
/// `WebhookUrl` is dropped because the modelled `ChimeWebhookConfiguration` has no such
/// member (`smithy_query` shape=ChimeWebhookConfiguration), so an update of it is visible
/// only in that no response carries it.
fn merged(stored: &Value, update: &Value) -> Value {
    let mut expected = stored.clone();
    let members = update.as_object().cloned().unwrap_or_else(Map::new);
    for (member, value) in members {
        if member == "ChatConfigurationArn" || member == "WebhookUrl" {
            continue;
        }
        expected[member] = value;
    }
    expected
}

/// One row of the update table: a label and the members the update carries beside
/// `ChatConfigurationArn`.
struct UpdateCase {
    label: &'static str,
    update: Value,
}

/// The update cases, each differing from the others along what the update rule generalises
/// over: which members the request carries (including none at all), which published
/// `LoggingLevel` value it names (`^(ERROR|INFO|NONE)$`, `smithy_query`:
/// CustomerCwLogLevel — the baseline create covers `INFO`), how many SNS topics the list
/// holds, and the description's length at each published bound (`ChimeWebhookDescription`,
/// len 1..255).
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "only the description",
            update: json!({"WebhookDescription": "Renamed Room/Webhook"}),
        },
        UpdateCase {
            label: "only the logging level, ERROR",
            update: json!({"LoggingLevel": "ERROR"}),
        },
        UpdateCase {
            label: "the description and the logging level together",
            update: json!({
                "WebhookDescription": "Incident Room/Pager Webhook",
                "LoggingLevel": "ERROR",
            }),
        },
        UpdateCase {
            label: "logging NONE and three SNS topics",
            update: json!({
                "LoggingLevel": "NONE",
                "SnsTopicArns": [
                    "arn:aws:sns:us-east-1:000000000000:alerts",
                    "arn:aws:sns:us-west-2:000000000000:pages",
                    "arn:aws:sns:eu-west-1:000000000000:audit",
                ],
            }),
        },
        UpdateCase {
            label: "an empty SNS topic list",
            update: json!({"SnsTopicArns": []}),
        },
        UpdateCase {
            label: "another IAM role",
            update: json!({"IamRoleArn": OTHER_IAM_ROLE_ARN}),
        },
        UpdateCase {
            label: "only the webhook URL, which no response carries",
            update: json!({"WebhookUrl": webhook_url("rotated")}),
        },
        UpdateCase {
            label: "the shortest allowed description",
            update: json!({"WebhookDescription": "a"}),
        },
        UpdateCase {
            label: "the longest allowed description",
            update: json!({"WebhookDescription": "d".repeat(255)}),
        },
        UpdateCase {
            label: "every member at once",
            update: json!({
                "WebhookDescription": "Everything/Replaced",
                "WebhookUrl": webhook_url("everything"),
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:everything"],
                "IamRoleArn": OTHER_IAM_ROLE_ARN,
                "LoggingLevel": "INFO",
            }),
        },
        UpdateCase {
            label: "no member but the ARN",
            update: json!({}),
        },
    ]
}

#[tokio::test]
async fn an_update_writes_the_supplied_members_through_and_leaves_the_others() {
    for case in update_cases() {
        // A fresh store per row, so every row updates the same baseline configuration.
        let emulator = Emulator::new();
        let configuration_name = "updated-webhook";
        let created = create_baseline(&emulator, configuration_name).await;
        let arn = created["ChatConfigurationArn"]
            .as_str()
            .expect("the create answers an ARN")
            .to_string();

        let mut request = case.update.clone();
        request["ChatConfigurationArn"] = json!(arn);
        let answer = emulator.call(UPDATE_PATH, &request).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the update answers 200, got {} with {}",
            case.label, answer.status, answer.body
        );

        let expected = merged(&created, &case.update);
        assert_eq!(
            answer.body.get("WebhookConfiguration"),
            Some(&expected),
            "case {:?}: the update must answer the updated configuration, got {}",
            case.label,
            answer.body
        );

        // The read must come out of the store, not out of the update request.
        let stored = described(&emulator, &arn).await;
        assert_eq!(
            stored, expected,
            "case {:?}: the described configuration differs from the expected one, got {stored}",
            case.label
        );

        // Neither the update nor the read hands the webhook destination back.
        for url in [webhook_url(configuration_name), webhook_url("rotated"), webhook_url("everything")] {
            assert!(
                !answer.body.to_string().contains(&url)
                    && !stored.to_string().contains(&url),
                "case {:?}: no response may carry the WebhookUrl {url}, got {} and {stored}",
                case.label,
                answer.body
            );
        }

        // Tags are the tagging operations' to write, so the update leaves them alone.
        let listed = emulator
            .call("/list-tags-for-resource", &json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            listed.body.get("Tags"),
            Some(&baseline_tags()),
            "case {:?}: the update must leave the tags alone, got {}",
            case.label,
            listed.body
        );
    }
}

#[tokio::test]
async fn successive_updates_each_build_on_the_configuration_the_previous_one_left() {
    // One configuration, every row applied in turn: each read must report the stored
    // configuration with that row's members written through it and nothing else moved.
    let emulator = Emulator::new();
    let created = create_baseline(&emulator, "serially-updated-1").await;
    let arn = created["ChatConfigurationArn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();
    let mut stored = created;

    for case in update_cases() {
        let mut request = case.update.clone();
        request["ChatConfigurationArn"] = json!(arn);
        let answer = emulator.call(UPDATE_PATH, &request).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the update answers 200, got {} with {}",
            case.label, answer.status, answer.body
        );

        stored = merged(&stored, &case.update);
        let described = described(&emulator, &arn).await;
        assert_eq!(
            described, stored,
            "case {:?}: the stored configuration must carry this update over the previous \
             ones, got {described}",
            case.label
        );
    }
}

#[tokio::test]
async fn an_update_is_addressed_by_arn_alone_whatever_region_it_is_signed_for() {
    let emulator = Emulator::new();
    // A chat configuration is REGIONAL: its ARN carries the region it was created in and
    // fully qualifies it, so a request signed for another region updates it just the same.
    let created = create_baseline(&emulator, "signed-elsewhere").await;
    let arn = created["ChatConfigurationArn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();
    assert_eq!(
        arn,
        arn_in_region(REGION, "signed-elsewhere"),
        "the created ARN must carry the region the create was signed for, got {arn}"
    );

    let answer = emulator
        .call_in_region(
            OTHER_REGION,
            UPDATE_PATH,
            &json!({
                "ChatConfigurationArn": arn,
                "WebhookDescription": "Signed/Elsewhere",
                "LoggingLevel": "NONE",
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "an update signed for {OTHER_REGION} must reach the configuration, got {} with {}",
        answer.status, answer.body
    );

    let stored = described(&emulator, &arn).await;
    assert_eq!(
        stored["WebhookDescription"],
        json!("Signed/Elsewhere"),
        "the update signed for {OTHER_REGION} must be the one stored, got {stored}"
    );
    assert_eq!(
        stored["LoggingLevel"],
        json!("NONE"),
        "the update signed for {OTHER_REGION} must be the one stored, got {stored}"
    );
}

#[tokio::test]
async fn updating_an_arn_the_store_holds_no_chime_configuration_under_is_not_found() {
    let emulator = Emulator::new();
    let created = create_baseline(&emulator, "present").await;
    let present = created["ChatConfigurationArn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // Each row is a well-formed `ChatConfigurationArn` the Chime webhook store holds
    // nothing under: they differ in the configuration name, in the region segment and in
    // the chat client the ARN names, so the refusal follows from the store rather than
    // from the spelling of the ARN.
    let unknown = [
        (
            "a name nothing was created under",
            arn_in_region(REGION, "absent"),
        ),
        (
            "the created name in another region",
            arn_in_region(OTHER_REGION, "present"),
        ),
        (
            "a Slack channel configuration's ARN",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/present"),
        ),
    ];

    for (label, arn) in &unknown {
        let answer = emulator
            .call(
                UPDATE_PATH,
                &json!({
                    "ChatConfigurationArn": arn,
                    "WebhookDescription": "Never/Applied",
                    "LoggingLevel": "ERROR",
                }),
            )
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

    // A refused update creates nothing and leaves the configuration that IS there as it
    // was.
    let stored = described(&emulator, &present).await;
    assert_eq!(
        stored, created,
        "the refused updates must leave the stored configuration unchanged, got {stored}"
    );
    let all = emulator.call(DESCRIBE_PATH, &json!({})).await;
    assert_eq!(
        all.body["WebhookConfigurations"],
        json!([created]),
        "a refused update must persist nothing, got {}",
        all.body
    );
}

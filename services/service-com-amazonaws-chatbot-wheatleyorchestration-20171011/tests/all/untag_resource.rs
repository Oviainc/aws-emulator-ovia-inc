// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UntagResource`, driven over the wire (restJson1) through the
//! same `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: detaching tags removes from the resource's
//! stored tag set exactly the keys the request names, matched on the whole key, leaving
//! every other key attached — so a key the resource does not carry removes nothing and the
//! call still succeeds, and a later read of the resource answers the remaining set. Which
//! resource the tags belong to comes from the ARN, so an ARN the store does not hold is
//! refused the same way the other tagging operations refuse it.
//!
//! The rule is exercised over a TABLE spanning what it generalises over — how many keys
//! one request names, whether each is attached, the key's length and case, a key repeated
//! in one request — crossed with every kind of chatbot resource that carries tags (a Slack
//! channel configuration, an Amazon Chime webhook configuration and a custom action). The
//! recorded case is one row.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;

use crate::wire::{Emulator, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

/// The Slack workspace the channel configurations below are created in, with the display
/// name it is authorized under. A workspace has to be authorized with the account before a
/// channel can be configured in it, and no operation in this model performs that handshake.
const SLACK_TEAM_ID: &str = "T0123ABCD";
const SLACK_TEAM_NAME: &str = "Primary Workspace";

const UNTAG_PATH: &str = "/untag-resource";
const TAG_PATH: &str = "/tag-resource";
const LIST_TAGS_PATH: &str = "/list-tags-for-resource";

/// The modelled 404 an ARN the store does not hold is refused with, for every kind of
/// chatbot resource the ARN can name.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The modelled 403 a custom-action ARN no create can mint for this account is refused with,
/// the ARN naming no target in this account's namespace.
const UNAUTHORIZED_EXCEPTION: &str = "UnauthorizedException";

/// This service mounted alone, with the Slack workspace the rows below configure channels
/// in already authorized through the state seam.
fn emulator() -> Emulator {
    let emulator = Emulator::new();
    chatbot::state::authorize_slack_workspace(&*emulator.engine, SLACK_TEAM_ID, SLACK_TEAM_NAME)
        .expect("the shared engine holds the chatbot store");
    emulator
}

/// A key at the published maximum `TagKey` length (`smithy_query`: TagKey, length 1..128).
fn longest_key() -> String {
    "k".repeat(128)
}

/// The tag set every resource under test starts from, keyed by `TagKey`.
///
/// The keys span what a key can be (`smithy_query`: TagKey, length 1..128): a short name, a
/// hyphenated one, a one-character one at the published minimum, and one at the published
/// maximum of 128 characters. One value is the empty string, which `TagValue` (length
/// 0..256) allows.
fn base_tags() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("env".to_string(), "prod".to_string()),
        ("team".to_string(), "sre".to_string()),
        ("owner".to_string(), "chatbot".to_string()),
        ("cost-center".to_string(), "42".to_string()),
        ("t".to_string(), String::new()),
        (longest_key(), "longest-key".to_string()),
    ])
}

/// A tag map as the modelled `Tag` list a read answers, in `TagKey` order.
fn tag_list(tags: &BTreeMap<String, String>) -> Value {
    Value::Array(
        tags.iter()
            .map(|(tag_key, tag_value)| json!({"TagKey": tag_key, "TagValue": tag_value}))
            .collect(),
    )
}

/// The `Tag` list a read must answer when exactly `remaining` of the base keys are still
/// attached. Panics on a key the base set never carried, so a typo in a row is not silently
/// asserted.
fn expected_tags(remaining: &[String]) -> Value {
    let base = base_tags();
    let kept: BTreeMap<String, String> = remaining
        .iter()
        .map(|tag_key| {
            let tag_value = base
                .get(tag_key)
                .unwrap_or_else(|| panic!("{tag_key:?} is not one of the base tag keys"));
            (tag_key.clone(), tag_value.clone())
        })
        .collect();
    tag_list(&kept)
}

/// Every base key but `removed`, in key order — the shorthand the rows below state their
/// expectation with.
fn all_but(removed: &[&str]) -> Vec<String> {
    base_tags()
        .into_keys()
        .filter(|tag_key| !removed.contains(&tag_key.as_str()))
        .collect()
}

fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(ToString::to_string).collect()
}

/// One row of the untag table: the keys one `UntagResource` call names, and the base keys
/// still attached afterwards.
struct UntagCase {
    label: &'static str,
    tag_keys: Vec<String>,
    remaining: Vec<String>,
}

/// The untag cases, each differing from the others along what the rule generalises over:
/// how many keys the request names, whether each one is attached, the length and the case
/// of a key, and whether a key is repeated within one request.
fn untag_cases() -> Vec<UntagCase> {
    vec![
        UntagCase {
            // `TagKeyList` is `length 0..200` (smithy_query: TagKeyList), so an empty list
            // is a well-formed request that names no key to detach.
            label: "an empty key list",
            tag_keys: Vec::new(),
            remaining: all_but(&[]),
        },
        UntagCase {
            label: "one attached key",
            tag_keys: keys(&["env"]),
            remaining: all_but(&["env"]),
        },
        UntagCase {
            label: "three attached keys at once",
            tag_keys: keys(&["env", "owner", "t"]),
            remaining: all_but(&["env", "owner", "t"]),
        },
        UntagCase {
            label: "every attached key",
            tag_keys: base_tags().into_keys().collect(),
            remaining: Vec::new(),
        },
        UntagCase {
            label: "a key the resource does not carry",
            tag_keys: keys(&["not-attached"]),
            remaining: all_but(&[]),
        },
        UntagCase {
            label: "an attached and an unattached key together",
            tag_keys: keys(&["team", "not-attached"]),
            remaining: all_but(&["team"]),
        },
        UntagCase {
            label: "the same key twice in one request",
            tag_keys: keys(&["cost-center", "cost-center"]),
            remaining: all_but(&["cost-center"]),
        },
        UntagCase {
            label: "the 128-character key",
            tag_keys: vec![longest_key()],
            remaining: all_but(&[]).into_iter().filter(|k| *k != longest_key()).collect(),
        },
        UntagCase {
            label: "a key differing from an attached one only in case",
            tag_keys: keys(&["ENV", "Team"]),
            remaining: all_but(&[]),
        },
        UntagCase {
            label: "a key that is a prefix of an attached one",
            tag_keys: keys(&["cost"]),
            remaining: all_but(&[]),
        },
    ]
}

/// Which kind of chatbot resource a row is applied to. Every kind carries tags and each
/// kind keeps its own store, so the rule has to hold for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    SlackChannel,
    ChimeWebhook,
    CustomAction,
}

const KINDS: [Kind; 3] = [Kind::SlackChannel, Kind::ChimeWebhook, Kind::CustomAction];

fn chat_configuration_arn(region: &str, client: &str, name: &str) -> String {
    format!("arn:aws:chatbot:{region}:{ACCOUNT}:chat-configuration/{client}/{name}")
}

fn custom_action_arn(name: &str) -> String {
    // A custom action is account-scoped, so its ARN carries an EMPTY region segment.
    format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{name}")
}

/// Create a resource of `kind` named `name`, carrying `tags`, and answer its ARN.
async fn create(emulator: &Emulator, kind: Kind, name: &str, tags: &Value) -> String {
    let (path, request, arn_at) = match kind {
        Kind::SlackChannel => (
            "/create-slack-channel-configuration",
            json!({
                "ConfigurationName": name,
                "SlackTeamId": SLACK_TEAM_ID,
                "SlackChannelId": "C0123ABCD",
                "IamRoleArn": IAM_ROLE_ARN,
                "Tags": tags,
            }),
            vec!["ChannelConfiguration", "ChatConfigurationArn"],
        ),
        Kind::ChimeWebhook => (
            "/create-chime-webhook-configuration",
            json!({
                "ConfigurationName": name,
                "WebhookDescription": "Ops/Room",
                "WebhookUrl": format!("https://hooks.chime.aws/incomingwebhooks/{name}?token=tok-1"),
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:alerts"],
                "IamRoleArn": IAM_ROLE_ARN,
                "Tags": tags,
            }),
            vec!["WebhookConfiguration", "ChatConfigurationArn"],
        ),
        Kind::CustomAction => (
            "/create-custom-action",
            json!({
                "ActionName": name,
                "Definition": {"CommandText": "ssm list-documents"},
                "Tags": tags,
            }),
            vec!["CustomActionArn"],
        ),
    };

    let created = emulator.call(path, &request).await;
    assert_eq!(
        created.status, 200,
        "kind {kind:?}: creating {name:?} must answer 200, got {} with {}",
        created.status, created.body
    );
    let mut value = &created.body;
    for member in arn_at {
        value = value
            .get(member)
            .unwrap_or_else(|| panic!("kind {kind:?}: no {member} in {}", created.body));
    }
    value
        .as_str()
        .unwrap_or_else(|| panic!("kind {kind:?}: the ARN is not a string in {}", created.body))
        .to_string()
}

/// The tags `arn` currently carries, read back through `ListTagsForResource`.
async fn listed_tags(emulator: &Emulator, label: &str, arn: &str) -> Value {
    let listed = emulator.call(LIST_TAGS_PATH, &json!({"ResourceARN": arn})).await;
    assert_eq!(
        listed.status, 200,
        "case {label:?}: reading {arn}'s tags must answer 200, got {} with {}",
        listed.status, listed.body
    );
    listed
        .body
        .get("Tags")
        .cloned()
        .unwrap_or_else(|| panic!("case {label:?}: no Tags member in {}", listed.body))
}

/// Call `UntagResource` for `arn` with `tag_keys` and assert it succeeded, answering no
/// members (`UntagResourceResponse` models none).
async fn untag_ok(emulator: &Emulator, label: &str, arn: &str, tag_keys: &[String]) {
    let answer = emulator
        .call(
            UNTAG_PATH,
            &json!({"ResourceARN": arn, "TagKeys": tag_keys}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: detaching {tag_keys:?} must answer 200, got {} with {}",
        answer.status, answer.body
    );
    assert!(
        answer.body.is_null() || answer.body == json!({}),
        "case {label:?}: UntagResource answers no members, got {}",
        answer.body
    );
}

#[tokio::test]
async fn untag_detaches_exactly_the_keys_named_and_leaves_the_rest_attached() {
    let emulator = emulator();
    let tags = tag_list(&base_tags());

    for kind in KINDS {
        for (row, case) in untag_cases().into_iter().enumerate() {
            // A fresh resource per row, so each row starts from the same tag set.
            let name = format!("untag-{kind:?}-{row}");
            let arn = create(&emulator, kind, &name, &tags).await;
            let label = format!("{kind:?} / {}", case.label);

            assert_eq!(
                listed_tags(&emulator, &label, &arn).await,
                tag_list(&base_tags()),
                "case {label:?}: the resource must start from the base tag set"
            );

            untag_ok(&emulator, &label, &arn, &case.tag_keys).await;

            let expected = expected_tags(&case.remaining);
            let actual = listed_tags(&emulator, &label, &arn).await;
            assert_eq!(
                actual, expected,
                "case {label:?}: detaching {:?} must leave {expected}, got {actual}",
                case.tag_keys
            );

            // Detaching is by key, so the SAME request again detaches nothing more.
            untag_ok(&emulator, &label, &arn, &case.tag_keys).await;
            let after_repeat = listed_tags(&emulator, &label, &arn).await;
            assert_eq!(
                after_repeat, expected,
                "case {label:?}: repeating the request must leave {expected}, got {after_repeat}"
            );
        }
    }
}

#[tokio::test]
async fn the_resource_itself_reports_the_remaining_tags() {
    let emulator = emulator();
    let tags = tag_list(&base_tags());

    // A chat configuration carries its tags in its own shape too, so the read of the
    // resource and the read of its tags cannot disagree.
    let cases = [
        (
            Kind::SlackChannel,
            "/describe-slack-channel-configurations",
            "SlackChannelConfigurations",
        ),
        (
            Kind::ChimeWebhook,
            "/describe-chime-webhook-configurations",
            "WebhookConfigurations",
        ),
    ];

    for (kind, describe_path, listed_member) in cases {
        let arn = create(&emulator, kind, "described", &tags).await;
        let label = format!("{kind:?} describe");

        untag_ok(&emulator, &label, &arn, &keys(&["env", "owner", "absent"])).await;
        let expected = expected_tags(&all_but(&["env", "owner"]));

        let described = emulator
            .call(describe_path, &json!({"ChatConfigurationArn": arn}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: the describe must answer 200, got {} with {}",
            described.status, described.body
        );
        let configurations = described
            .body
            .get(listed_member)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {label:?}: no {listed_member} in {}", described.body));
        assert_eq!(
            configurations.len(),
            1,
            "case {label:?}: the describe must answer exactly one configuration, got {}",
            described.body
        );
        assert_eq!(
            configurations[0]["Tags"], expected,
            "case {label:?}: the configuration must carry {expected}, got {}",
            configurations[0]
        );
    }
}

#[tokio::test]
async fn untagging_one_resource_leaves_its_neighbours_tags_alone() {
    let emulator = emulator();
    let tags = tag_list(&base_tags());

    // One resource of each kind, all carrying the same keys: the tags belong to the ARN, so
    // detaching from one must not touch another — not even the one of the same kind.
    let mut arns: Vec<(String, String)> = Vec::new();
    for kind in KINDS {
        for suffix in ["first", "second"] {
            let name = format!("{kind:?}-{suffix}");
            let arn = create(&emulator, kind, &name, &tags).await;
            arns.push((name, arn));
        }
    }

    let (target_name, target_arn) = arns[0].clone();
    untag_ok(&emulator, &target_name, &target_arn, &keys(&["env"])).await;

    for (name, arn) in &arns {
        let expected = if arn == &target_arn {
            expected_tags(&all_but(&["env"]))
        } else {
            tag_list(&base_tags())
        };
        let actual = listed_tags(&emulator, name, arn).await;
        assert_eq!(
            actual, expected,
            "case {name:?}: expected {expected}, got {actual}"
        );
    }
}

#[tokio::test]
async fn a_detached_key_can_be_attached_again() {
    let emulator = emulator();
    let arn = create(&emulator, Kind::SlackChannel, "round-trip", &tag_list(&base_tags())).await;

    for round in 0..3 {
        let label = format!("round {round}");
        untag_ok(&emulator, &label, &arn, &keys(&["env"])).await;
        assert_eq!(
            listed_tags(&emulator, &label, &arn).await,
            expected_tags(&all_but(&["env"])),
            "case {label:?}: the detached key must be gone"
        );

        let tagged = emulator
            .call(
                TAG_PATH,
                &json!({"ResourceARN": arn, "Tags": [{"TagKey": "env", "TagValue": "prod"}]}),
            )
            .await;
        assert_eq!(
            tagged.status, 200,
            "case {label:?}: re-attaching must answer 200, got {} with {}",
            tagged.status, tagged.body
        );
        assert_eq!(
            listed_tags(&emulator, &label, &arn).await,
            tag_list(&base_tags()),
            "case {label:?}: the re-attached key must be back"
        );
    }
}

#[tokio::test]
async fn an_arn_the_store_does_not_hold_is_refused_as_its_arn_form_calls_for() {
    let emulator = emulator();
    // Every chatbot ARN of a form this account's resources are minted under that the store
    // holds nothing under is the modelled 404; only an ARN no create can mint for this
    // account (a region segment on an account-scoped custom action) is the authorization
    // failure.
    let held = create(
        &emulator,
        Kind::SlackChannel,
        "present",
        &tag_list(&base_tags()),
    )
    .await;

    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a Slack channel configuration that was never created",
            chat_configuration_arn(REGION, "slack-channel", "never-created"),
            404,
            RESOURCE_NOT_FOUND_EXCEPTION,
        ),
        (
            "a Chime webhook configuration that was never created",
            chat_configuration_arn(REGION, "chime-webhook", "never-created"),
            404,
            RESOURCE_NOT_FOUND_EXCEPTION,
        ),
        (
            "another region's ARN for a configuration this region holds",
            chat_configuration_arn(OTHER_REGION, "slack-channel", "present"),
            404,
            RESOURCE_NOT_FOUND_EXCEPTION,
        ),
        (
            "an ARN naming no chatbot resource kind at all",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:something-else/present"),
            404,
            RESOURCE_NOT_FOUND_EXCEPTION,
        ),
        (
            "a custom action that was never created",
            custom_action_arn("never-created"),
            404,
            RESOURCE_NOT_FOUND_EXCEPTION,
        ),
        (
            "a region-qualified ARN for a custom action",
            format!("arn:aws:chatbot:{OTHER_REGION}:{ACCOUNT}:custom-action/never-created"),
            403,
            UNAUTHORIZED_EXCEPTION,
        ),
    ];

    for (label, arn, status, code) in &cases {
        let answer = emulator
            .call(
                UNTAG_PATH,
                &json!({"ResourceARN": arn, "TagKeys": ["env"]}),
            )
            .await;
        assert_eq!(
            answer.status, *status,
            "case {label:?}: expected HTTP {status}, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            *code,
            "case {label:?}: expected the modelled {code}, got {}",
            answer.body
        );
    }

    // The refusals left the configuration the store does hold exactly as it was.
    assert_eq!(
        listed_tags(&emulator, "the held configuration", &held).await,
        tag_list(&base_tags()),
        "a refused untag must detach nothing from any resource"
    );
}

#[tokio::test]
async fn an_arn_whose_resource_was_deleted_is_no_longer_a_resource() {
    let emulator = emulator();
    let arn = create(&emulator, Kind::SlackChannel, "doomed", &tag_list(&base_tags())).await;

    let deleted = emulator
        .call(
            "/delete-slack-channel-configuration",
            &json!({"ChatConfigurationArn": arn}),
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "the delete answers 204, got {} with {}",
        deleted.status, deleted.body
    );

    let answer = emulator
        .call(UNTAG_PATH, &json!({"ResourceARN": arn, "TagKeys": ["env"]}))
        .await;
    assert_eq!(
        answer.status, 404,
        "untagging a deleted configuration is a 404, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "untagging a deleted configuration is a {RESOURCE_NOT_FOUND_EXCEPTION}, got {}",
        answer.body
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the per-resource tag cap `TagResource` enforces, driven over the
//! wire (restJson1) through the same `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: `TagResource` merges the request's tags onto the
//! resource's stored set by `TagKey`, and the merge is refused — leaving the stored set
//! exactly as it was — when the TOTAL it would leave the resource carrying exceeds the 200
//! tags the modelled `TagList` bounds (`smithy_query` shape=TagList: `@length` min 0 max
//! 200). That total is a request-constraint violation, so the refusal is this service's
//! generic client-input class: HTTP 400 `InvalidRequestException`, stating no message — the
//! shape's `message` member (`smithy_query` shape=InvalidRequestException) reported as the
//! explicit null this service's error document carries for a class that words none. It is
//! NOT the modelled `TooManyTagsException`, which words the request LIST being too long — a
//! condition the list's own `@length` bound already refuses before any handler runs.
//!
//! The rule is exercised over a TABLE spanning what it generalises over — how many tags the
//! resource already carries, how many the request names, whether each named key is new or
//! already stored, a key repeated inside one request, and by how much the total overruns —
//! crossed with every kind of chatbot resource that carries tags (a Slack channel
//! configuration, an Amazon Chime webhook configuration and a custom action). Both published
//! bounds of the cap are included: a merge landing exactly on it succeeds and a merge one
//! past it is refused.

use serde_json::{json, Value};
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;

use crate::wire::Emulator;

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

/// The Slack workspace the channel configurations below are created in, with the display
/// name it is authorized under: a workspace has to be authorized with the account before a
/// channel can be configured in it, and no operation in this model performs that handshake.
const SLACK_TEAM_ID: &str = "T0123ABCD";
const SLACK_TEAM_NAME: &str = "Primary Workspace";

const TAG_PATH: &str = "/tag-resource";
const UNTAG_PATH: &str = "/untag-resource";
const LIST_TAGS_PATH: &str = "/list-tags-for-resource";

/// The most tags one resource carries: the modelled `TagList` is `@length` min 0 max 200
/// (`smithy_query` shape=TagList).
const MAX_TAGS: usize = 200;

/// The error class a request whose input does not meet this service's constraints is refused
/// with (`smithy_query` shape=InvalidRequestException: `@error("client")`, `@httpError(400)`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The error class the model binds to this operation for a request LIST that carries too many
/// tags, which is NOT the class the per-resource total is refused with.
const TOO_MANY_TAGS_EXCEPTION: &str = "TooManyTagsException";

/// This service mounted alone, with the Slack workspace the rows below configure channels in
/// already authorized through the state seam.
fn emulator() -> Emulator {
    let emulator = Emulator::new();
    chatbot::state::authorize_slack_workspace(&*emulator.engine, SLACK_TEAM_ID, SLACK_TEAM_NAME)
        .expect("the shared engine holds the chatbot store");
    emulator
}

/// Which kind of chatbot resource a row is applied to. Every kind carries tags and each kind
/// keeps its own store, so the cap has to hold for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    SlackChannel,
    ChimeWebhook,
    CustomAction,
}

const KINDS: [Kind; 3] = [Kind::SlackChannel, Kind::ChimeWebhook, Kind::CustomAction];

/// The `Tag` list for the keys `range` names, each carrying `value`.
fn tags_in(range: std::ops::Range<usize>, value: &str) -> Value {
    Value::Array(
        range
            .map(|n| json!({"TagKey": format!("key-{n:04}"), "TagValue": value}))
            .collect(),
    )
}

/// Create a resource of `kind` named `name` carrying `tags`, and answer its ARN.
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
                "SnsTopicArns": [format!("arn:aws:sns:us-east-1:{ACCOUNT}:alerts")],
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

/// The tags `arn` carries, read back through `ListTagsForResource`, as a sorted list of
/// `(TagKey, TagValue)` pairs.
async fn stored_tags(emulator: &Emulator, label: &str, arn: &str) -> Vec<(String, String)> {
    let listed = emulator
        .call(LIST_TAGS_PATH, &json!({"ResourceARN": arn}))
        .await;
    assert_eq!(
        listed.status, 200,
        "case {label:?}: reading {arn}'s tags must answer 200, got {} with {}",
        listed.status, listed.body
    );
    let mut pairs: Vec<(String, String)> = listed
        .body
        .get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label:?}: no Tags member in {}", listed.body))
        .iter()
        .map(|tag| {
            (
                tag["TagKey"].as_str().unwrap_or_default().to_string(),
                tag["TagValue"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// One row of the cap table.
struct CapCase {
    label: &'static str,
    /// How many distinct keys the resource is created carrying.
    stored: usize,
    /// The tags one `TagResource` call names.
    request: Value,
    /// The total the resource carries afterwards, or `None` when the merge is refused for
    /// overrunning the cap (in which case the stored set is unchanged).
    expected_total: Option<usize>,
}

/// The cap cases. Each differs from the others along what the rule generalises over: the
/// stored count, the request's length, whether its keys are new or already stored, whether a
/// key repeats inside one request, and by how much the total would overrun — including both
/// published bounds of the cap (landing exactly on 200, and one past it).
fn cap_cases() -> Vec<CapCase> {
    vec![
        CapCase {
            label: "one brand-new key past a full resource",
            stored: MAX_TAGS,
            request: tags_in(MAX_TAGS..MAX_TAGS + 1, "v"),
            expected_total: None,
        },
        CapCase {
            label: "a hundred brand-new keys past a full resource",
            stored: MAX_TAGS,
            request: tags_in(MAX_TAGS..MAX_TAGS + 100, "v"),
            expected_total: None,
        },
        CapCase {
            label: "one stored key and one new key on a full resource",
            stored: MAX_TAGS,
            request: json!([
                {"TagKey": "key-0000", "TagValue": "replaced"},
                {"TagKey": "key-0200", "TagValue": "v"},
            ]),
            expected_total: None,
        },
        CapCase {
            label: "one new key twice in one request on a full resource",
            stored: MAX_TAGS,
            request: json!([
                {"TagKey": "key-0200", "TagValue": "first"},
                {"TagKey": "key-0200", "TagValue": "second"},
            ]),
            expected_total: None,
        },
        CapCase {
            label: "two new keys one short of the cap",
            stored: MAX_TAGS - 1,
            request: tags_in(MAX_TAGS - 1..MAX_TAGS + 1, "v"),
            expected_total: None,
        },
        CapCase {
            label: "fifty-one new keys onto a hundred and fifty",
            stored: 150,
            request: tags_in(150..201, "v"),
            expected_total: None,
        },
        CapCase {
            label: "one new key landing exactly on the cap",
            stored: MAX_TAGS - 1,
            request: tags_in(MAX_TAGS - 1..MAX_TAGS, "v"),
            expected_total: Some(MAX_TAGS),
        },
        CapCase {
            label: "fifty new keys landing exactly on the cap",
            stored: 150,
            request: tags_in(150..MAX_TAGS, "v"),
            expected_total: Some(MAX_TAGS),
        },
        CapCase {
            label: "one new key twice in one request landing exactly on the cap",
            stored: MAX_TAGS - 1,
            request: json!([
                {"TagKey": "key-0199", "TagValue": "first"},
                {"TagKey": "key-0199", "TagValue": "v"},
            ]),
            expected_total: Some(MAX_TAGS),
        },
        CapCase {
            label: "replacing one stored key's value on a full resource",
            stored: MAX_TAGS,
            request: json!([{"TagKey": "key-0000", "TagValue": "v"}]),
            expected_total: Some(MAX_TAGS),
        },
        CapCase {
            label: "replacing every stored key on a full resource",
            stored: MAX_TAGS,
            request: tags_in(0..MAX_TAGS, "v"),
            expected_total: Some(MAX_TAGS),
        },
        CapCase {
            label: "an empty tag list on a full resource",
            stored: MAX_TAGS,
            request: json!([]),
            expected_total: Some(MAX_TAGS),
        },
        CapCase {
            label: "a full list of new keys onto an untagged resource",
            stored: 0,
            request: tags_in(0..MAX_TAGS, "v"),
            expected_total: Some(MAX_TAGS),
        },
    ]
}

/// Assert `answer` is the refusal a merge overrunning the per-resource cap carries: HTTP 400,
/// this service's generic client-input error class, and a `message` member stating nothing —
/// the explicit null this service's error document carries for a class that words none, under
/// the spelling the shape declares (`smithy_query` shape=InvalidRequestException: member
/// `message`) and never the other one.
fn assert_cap_refusal(label: &str, answer: &crate::wire::Answer) {
    assert_eq!(
        answer.status, 400,
        "case {label:?}: expected HTTP 400, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        INVALID_REQUEST_EXCEPTION,
        "case {label:?}: expected {INVALID_REQUEST_EXCEPTION}, got {}",
        answer.body
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some(INVALID_REQUEST_EXCEPTION),
        "case {label:?}: expected x-amzn-errortype {INVALID_REQUEST_EXCEPTION}, got {:?}",
        answer.error_type
    );
    assert_ne!(
        answer.code(),
        TOO_MANY_TAGS_EXCEPTION,
        "case {label:?}: the per-resource total is not the request list's own error class, \
         got {}",
        answer.body
    );
    assert_eq!(
        answer.body.get("message"),
        Some(&Value::Null),
        "case {label:?}: this refusal states no message, so the member is a null, got {}",
        answer.body
    );
    assert_eq!(
        answer.body.get("Message"),
        None,
        "case {label:?}: this error class declares no capitalised member, got {}",
        answer.body
    );
}

#[tokio::test]
async fn a_merge_past_the_per_resource_tag_cap_is_refused_as_an_invalid_request() {
    let emulator = emulator();

    for kind in KINDS {
        for (row, case) in cap_cases().into_iter().enumerate() {
            // A fresh resource per row, so each row merges onto the arrangement it names.
            let name = format!("cap-{kind:?}-{row}");
            let arn = create(&emulator, kind, &name, &tags_in(0..case.stored, "stored")).await;
            let label = format!("{kind:?} / {}", case.label);

            let before = stored_tags(&emulator, &label, &arn).await;
            assert_eq!(
                before.len(),
                case.stored,
                "case {label:?}: the resource must start from {} tags, got {}",
                case.stored,
                before.len()
            );

            let answer = emulator
                .call(
                    TAG_PATH,
                    &json!({"ResourceARN": arn, "Tags": case.request}),
                )
                .await;
            let after = stored_tags(&emulator, &label, &arn).await;

            match case.expected_total {
                None => {
                    assert_cap_refusal(&label, &answer);
                    // A refused merge applies nothing at all, not even the keys that fit.
                    assert_eq!(
                        after, before,
                        "case {label:?}: a refused merge must leave the stored tags as they \
                         were, got {after:?}"
                    );
                }
                Some(total) => {
                    assert_eq!(
                        answer.status, 200,
                        "case {label:?}: expected HTTP 200, got {} with {}",
                        answer.status, answer.body
                    );
                    assert_eq!(
                        after.len(),
                        total,
                        "case {label:?}: expected {total} stored tags, got {}",
                        after.len()
                    );
                    // Every key the request named carries the value it named, the last
                    // occurrence winning when one request repeats a key.
                    let requested = case.request.as_array().cloned().unwrap_or_default();
                    for tag in &requested {
                        let key = tag["TagKey"].as_str().unwrap_or_default().to_string();
                        let expected_value = requested
                            .iter()
                            .rfind(|t| t["TagKey"] == tag["TagKey"])
                            .map(|t| t["TagValue"].as_str().unwrap_or_default().to_string())
                            .unwrap_or_default();
                        let stored = after
                            .iter()
                            .find(|(stored_key, _)| stored_key == &key)
                            .map(|(_, value)| value.clone());
                        assert_eq!(
                            stored,
                            Some(expected_value.clone()),
                            "case {label:?}: {key:?} must carry {expected_value:?}, got \
                             {stored:?}"
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn a_full_resource_takes_a_new_key_again_once_one_is_detached() {
    // The cap is on the stored total, not on the history of the resource: whatever freed a
    // slot, the next merge that fits is served. This is the positive control for the refusal
    // above, over every kind of resource that carries tags.
    let emulator = emulator();

    for kind in KINDS {
        let label = format!("{kind:?} / detach then attach");
        let arn = create(
            &emulator,
            kind,
            &format!("freed-{kind:?}"),
            &tags_in(0..MAX_TAGS, "stored"),
        )
        .await;

        let refused = emulator
            .call(
                TAG_PATH,
                &json!({"ResourceARN": arn, "Tags": tags_in(MAX_TAGS..MAX_TAGS + 1, "v")}),
            )
            .await;
        assert_cap_refusal(&label, &refused);

        let detached = emulator
            .call(
                UNTAG_PATH,
                &json!({"ResourceARN": arn, "TagKeys": ["key-0000"]}),
            )
            .await;
        assert_eq!(
            detached.status, 200,
            "case {label:?}: detaching one key must answer 200, got {} with {}",
            detached.status, detached.body
        );

        let accepted = emulator
            .call(
                TAG_PATH,
                &json!({"ResourceARN": arn, "Tags": tags_in(MAX_TAGS..MAX_TAGS + 1, "v")}),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {label:?}: the same merge must be served once a slot is free, got {} with {}",
            accepted.status, accepted.body
        );
        let after = stored_tags(&emulator, &label, &arn).await;
        assert_eq!(
            after.len(),
            MAX_TAGS,
            "case {label:?}: expected {MAX_TAGS} stored tags, got {}",
            after.len()
        );
        assert!(
            after.contains(&("key-0200".to_string(), "v".to_string())),
            "case {label:?}: the newly attached key must be stored, got {after:?}"
        );
        assert!(
            !after.iter().any(|(key, _)| key == "key-0000"),
            "case {label:?}: the detached key must stay detached, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_request_list_longer_than_the_modelled_bound_is_refused_before_the_resource_matters() {
    // The modelled `TagList` bound is on the REQUEST's list, so a list longer than it is
    // refused whatever the resource holds — and with the same client-input class and status,
    // which is why the stored total has no separate class of its own. The resource is left
    // untouched either way.
    let emulator = emulator();

    for kind in KINDS {
        for (label, stored) in [("an untagged resource", 0usize), ("a full resource", MAX_TAGS)] {
            let case = format!("{kind:?} / {label}");
            let arn = create(
                &emulator,
                kind,
                &format!("overlong-{kind:?}-{stored}"),
                &tags_in(0..stored, "stored"),
            )
            .await;
            let before = stored_tags(&emulator, &case, &arn).await;

            let answer = emulator
                .call(
                    TAG_PATH,
                    &json!({"ResourceARN": arn, "Tags": tags_in(1000..1000 + MAX_TAGS + 1, "v")}),
                )
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                INVALID_REQUEST_EXCEPTION,
                "case {case:?}: expected {INVALID_REQUEST_EXCEPTION}, got {}",
                answer.body
            );
            assert_eq!(
                stored_tags(&emulator, &case, &arn).await,
                before,
                "case {case:?}: a refused request must store nothing"
            );
        }
    }
}

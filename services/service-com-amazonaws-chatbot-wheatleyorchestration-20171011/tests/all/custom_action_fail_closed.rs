// SPDX-License-Identifier: CC0-1.0

//! How this service answers an operation addressed to a CUSTOM ACTION by ARN when this
//! account holds no action under it, and how a custom action's tags behave — over the wire
//! (restJson1) through the same `MultiServiceRouter` the production router serves with.
//!
//! The rule every assertion here is about: AWS Chatbot decides a custom-action request
//! against the ARN it names BEFORE any existence lookup, so a well-formed `custom-action`
//! ARN this account holds no action under is refused with the modelled
//! `UnauthorizedException` (HTTP 403) — on every operation that addresses it, reading it or
//! tagging it alike — and the answer discloses nothing about whether the action exists. The
//! resource type in the ARN is what decides that: a `chat-configuration` ARN (Slack channel
//! or Chime webhook) keeps the modelled `ResourceNotFoundException` (HTTP 404) that the
//! tagging operations declare.
//!
//! The tables span the class rather than one recorded request: the refused ARNs differ in
//! the action name's length and charset and in the region segment, and each one is then
//! CREATED and re-addressed, so the refusal provably follows from the store rather than
//! from the shape of the ARN.

use serde_json::{json, Value};

use crate::wire::{Emulator, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

/// The modelled 403 a custom-action ARN this account holds nothing for is refused with.
const UNAUTHORIZED_EXCEPTION: &str = "UnauthorizedException";

/// The modelled 404 the tagging operations answer for a chat configuration they do not
/// hold.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

fn custom_action_arn_in_region(region: &str, action_name: &str) -> String {
    format!("arn:aws:chatbot:{region}:{ACCOUNT}:custom-action/{action_name}")
}

fn custom_action_arn(action_name: &str) -> String {
    custom_action_arn_in_region(REGION, action_name)
}

/// The implemented operations that address a resource by ARN: the member the ARN travels
/// in, and the path.
const ARN_ADDRESSED: [(&str, &str); 3] = [
    ("CustomActionArn", "/get-custom-action"),
    ("ResourceARN", "/tag-resource"),
    ("ResourceARN", "/list-tags-for-resource"),
];

/// The request body addressing `arn` through `member` on `path`.
fn arn_request(member: &str, path: &str, arn: &str) -> Value {
    let mut request = json!({member: arn});
    if path == "/tag-resource" {
        request["Tags"] = json!([{"TagKey": "k", "TagValue": "v"}]);
    }
    request
}

#[tokio::test]
async fn a_custom_action_arn_this_account_holds_no_action_for_is_unauthorized() {
    let emulator = Emulator::new();

    // Well-formed `CustomActionArn`s (the modelled pattern
    // `^arn:aws:chatbot:…:custom-action/[a-zA-Z0-9_-]{1,64}$`) that no create ever minted:
    // they differ in the action name's length and charset and in the region segment, so the
    // refusal cannot follow from the spelling of the ARN.
    let unknown = [
        custom_action_arn("sim2real-never-created-action"),
        custom_action_arn("a"),
        custom_action_arn(&"z".repeat(64)),
        custom_action_arn("Mixed_Case-9"),
        custom_action_arn_in_region(OTHER_REGION, "never-created"),
    ];

    for arn in &unknown {
        for (member, path) in ARN_ADDRESSED {
            let answer = emulator.call(path, &arn_request(member, path, arn)).await;
            assert_eq!(
                answer.status, 403,
                "case {arn:?} on {path}: expected HTTP 403, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                UNAUTHORIZED_EXCEPTION,
                "case {arn:?} on {path}: expected the modelled {UNAUTHORIZED_EXCEPTION}, got {}",
                answer.body
            );
        }
    }

    // The same ARNs, once the action exists, are served — so the 403 above came from the
    // store and not from the ARN itself.
    for arn in &unknown {
        let action_name = arn.rsplit('/').next().expect("the ARN names an action");
        let region = if arn.contains(OTHER_REGION) {
            OTHER_REGION
        } else {
            REGION
        };
        let created = emulator
            .call_in_region(
                region,
                "/create-custom-action",
                &json!({
                    "ActionName": action_name,
                    "Definition": {"CommandText": "ssm list-documents"},
                }),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {arn:?}: the create answers 201, got {} with {}",
            created.status, created.body
        );

        for (member, path) in ARN_ADDRESSED {
            let answer = emulator.call(path, &arn_request(member, path, arn)).await;
            assert_eq!(
                answer.status, 200,
                "case {arn:?} on {path}: an action that exists is served, got {} with {}",
                answer.status, answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_chat_configuration_arn_no_configuration_exists_for_is_still_not_found() {
    let emulator = Emulator::new();
    // Fail-closed is the CUSTOM ACTION's answer: a chat-configuration ARN keeps the
    // modelled 404 the tagging operations declare, so the two resource kinds stay
    // distinguishable. Both chat clients are covered, since each keeps its own store.
    let unknown = [
        format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/never-created"),
        format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/chime-webhook/never-created"),
    ];

    for arn in &unknown {
        for (member, path) in ARN_ADDRESSED {
            if path == "/get-custom-action" {
                // `CustomActionArn`'s pattern cannot carry a chat-configuration ARN at all.
                continue;
            }
            let answer = emulator.call(path, &arn_request(member, path, arn)).await;
            assert_eq!(
                answer.status, 404,
                "case {arn:?} on {path}: expected HTTP 404, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                RESOURCE_NOT_FOUND_EXCEPTION,
                "case {arn:?} on {path}: expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION}, \
                 got {}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn tags_on_a_custom_action_merge_by_key_and_are_reported_by_list_tags() {
    let emulator = Emulator::new();
    let arn = custom_action_arn("tagged-action");
    let created = emulator
        .call(
            "/create-custom-action",
            &json!({
                "ActionName": "tagged-action",
                "Definition": {"CommandText": "ssm list-documents"},
                "Tags": [{"TagKey": "team", "TagValue": "ops"}],
            }),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the create answers 201, got {} with {}",
        created.status, created.body
    );

    // Each row merges onto the set the previous one left, so the expectation is the whole
    // tag set after it: a repeated key replaces its value and a new key is added.
    let merges: Vec<(&str, Value, Value)> = vec![
        (
            "a tag the create already carried, with another value",
            json!([{"TagKey": "team", "TagValue": "platform"}]),
            json!([{"TagKey": "team", "TagValue": "platform"}]),
        ),
        (
            "two more keys at once",
            json!([
                {"TagKey": "env", "TagValue": "prod"},
                {"TagKey": "owner", "TagValue": "chatbot"},
            ]),
            json!([
                {"TagKey": "env", "TagValue": "prod"},
                {"TagKey": "owner", "TagValue": "chatbot"},
                {"TagKey": "team", "TagValue": "platform"},
            ]),
        ),
        (
            "an empty tag value",
            json!([{"TagKey": "owner", "TagValue": ""}]),
            json!([
                {"TagKey": "env", "TagValue": "prod"},
                {"TagKey": "owner", "TagValue": ""},
                {"TagKey": "team", "TagValue": "platform"},
            ]),
        ),
        (
            "no tags at all",
            json!([]),
            json!([
                {"TagKey": "env", "TagValue": "prod"},
                {"TagKey": "owner", "TagValue": ""},
                {"TagKey": "team", "TagValue": "platform"},
            ]),
        ),
    ];

    for (label, tags, expected) in merges {
        let tagged = emulator
            .call("/tag-resource", &json!({"ResourceARN": arn, "Tags": tags}))
            .await;
        assert_eq!(
            tagged.status, 200,
            "case {label:?}: tagging a custom action answers 200, got {} with {}",
            tagged.status, tagged.body
        );

        let listed = emulator
            .call("/list-tags-for-resource", &json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: reading a custom action's tags answers 200, got {} with {}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.body.get("Tags"),
            Some(&expected),
            "case {label:?}: expected the merged tag set, got {}",
            listed.body
        );
    }

    // Tagging does not disturb the action itself.
    let fetched = emulator
        .call("/get-custom-action", &json!({"CustomActionArn": arn}))
        .await;
    assert_eq!(
        fetched.body.get("CustomAction"),
        Some(&json!({
            "CustomActionArn": arn,
            "Definition": {"CommandText": "ssm list-documents"},
            "ActionName": "tagged-action",
        })),
        "tagging must leave the action as it was, got {}",
        fetched.body
    );
}

#[tokio::test]
async fn a_chime_webhook_configuration_is_tagged_like_any_other_chat_configuration() {
    let emulator = Emulator::new();
    // A chat configuration ARN carries the chat client it configures
    // (`chat-configuration/{slack-channel|chime-webhook}/{name}`), and each kind has its own
    // store, so the tagging operations must resolve either one.
    let created = emulator
        .call(
            "/create-chime-webhook-configuration",
            &json!({
                "ConfigurationName": "chime-tagged",
                "WebhookDescription": "chatbot emulator webhook",
                "WebhookUrl": "https://hooks.chime.aws/incomingwebhooks/abc-123?token=tok-456",
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:chatbot-topic"],
                "IamRoleArn": "arn:aws:iam::000000000000:role/chatbot-role",
            }),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the Chime webhook create answers 201, got {} with {}",
        created.status, created.body
    );
    let arn = created.body["WebhookConfiguration"]["ChatConfigurationArn"].clone();

    let tagged = emulator
        .call(
            "/tag-resource",
            &json!({"ResourceARN": arn, "Tags": [{"TagKey": "team", "TagValue": "ops"}]}),
        )
        .await;
    assert_eq!(
        tagged.status, 200,
        "tagging a Chime webhook configuration answers 200, got {} with {}",
        tagged.status, tagged.body
    );

    let listed = emulator
        .call("/list-tags-for-resource", &json!({"ResourceARN": arn}))
        .await;
    assert_eq!(
        listed.body.get("Tags"),
        Some(&json!([{"TagKey": "team", "TagValue": "ops"}])),
        "expected the tag set on the Chime webhook configuration, got {}",
        listed.body
    );
}

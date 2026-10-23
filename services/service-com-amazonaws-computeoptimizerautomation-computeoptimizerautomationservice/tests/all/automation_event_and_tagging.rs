// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartAutomationEvent`, `GetAutomationEvent`,
//! `UntagResource`, and the authorization of the resource string every
//! resource-naming operation takes.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! dispatched through the same [`MultiServiceRouter`] the emulator binary mounts
//! — through the harness `automation_rule_lifecycle` already owns. The one thing
//! these suites reach into the store for is SEEDING a recommended action: a
//! recommended action is produced by Compute Optimizer's analysis of an
//! account's resources, which this emulator does not perform, so no API call can
//! create one. Everything asserted afterwards is read back over the wire.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::RecommendedAction;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

// The router+engine harness the event suite already owns, so there is one
// builder for a case that has to seed the store.
use super::automation_events::emulator as emulator_with_engine;
use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator, error_code, is_rule_id, member, minimal_rule,
    string_member,
};

/// One recommended action, as Compute Optimizer's analysis would have produced
/// it. `recommended_action_id` must satisfy the modelled `RecommendedActionId`
/// (`^[0-9A-Za-z]{16}$`) because a request names it.
fn recommended_action(id: &str, action_type: &str, volume: &str) -> RecommendedAction {
    RecommendedAction {
        recommended_action_id: Some(id.to_string()),
        resource_arn: Some(format!(
            "arn:aws:ec2:us-east-1:{}:volume/{volume}",
            engine::EMULATOR_ACCOUNT_ID
        )),
        resource_id: Some(volume.to_string()),
        account_id: Some(engine::EMULATOR_ACCOUNT_ID.to_string()),
        region: Some("us-east-1".to_string()),
        // The model's only `ResourceType` value.
        resource_type: Some("EbsVolume".to_string()),
        recommended_action_type: Some(action_type.to_string()),
        ..Default::default()
    }
}

/// Put `action` in the store the mounted service reads from.
fn seed_action(engine: &Arc<engine::InMemoryEngine>, action: &RecommendedAction) {
    let service_state = state::service_state(&**engine).expect("the service state is reachable");
    let id = action
        .recommended_action_id
        .clone()
        .expect("a seeded action carries its id");
    service_state
        .lock()
        .recommended_actions
        .insert(id, action.clone());
}

/// Create a rule carrying `tags` and answer `(ruleArn, ruleRevision)`.
async fn rule_with_tags(router: &MultiServiceRouter, name: &str, tags: &Value, case: &str) -> (String, i64) {
    let mut request = minimal_rule(name);
    request["tags"] = tags.clone();
    let created = call_ok(router, "CreateAutomationRule", &request, case).await;
    (
        string_member(&created, "ruleArn"),
        member(&created, "ruleRevision").as_i64().unwrap_or_default(),
    )
}

/// The tag keys `GetAutomationRule` reports for `rule_arn`, sorted.
async fn stored_tag_keys(router: &MultiServiceRouter, rule_arn: &str, case: &str) -> Vec<String> {
    let rule = call_ok(router, "GetAutomationRule", &json!({"ruleArn": rule_arn}), case).await;
    let mut keys: Vec<String> = member(&rule, "tags")
        .as_array()
        .map(|tags| tags.iter().map(|tag| string_member(tag, "key")).collect())
        .unwrap_or_default();
    keys.sort();
    keys
}

// ---------------------------------------------------------------------------
// StartAutomationEvent
// ---------------------------------------------------------------------------

/// A well-formed `recommendedActionId` that no stored recommended action
/// carries is the modelled `ResourceNotFoundException` (404) — not a 501 and not
/// a parameter rejection — whatever else the store holds and whether or not the
/// request carries a client token.
#[tokio::test]
async fn starting_an_automation_for_an_unknown_action_is_not_found() {
    let router = emulator();
    // A rule in the store must not make an action exist: the two are different
    // resources.
    call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("start-event-rule"),
        "setup",
    )
    .await;

    // Every row satisfies `RecommendedActionId` (`^[0-9A-Za-z]{16}$`), differing
    // in case, digits and letters, so the 404 is about the store and not about
    // the characters.
    let ids = [
        "AAAAAAAAAAAAAAAA",
        "0000000000000000",
        "0123456789abcdef",
        "zZ9aA0bB1cC2dD3e",
    ];
    for id in ids {
        for token in [None, Some("start-token-1")] {
            let case = format!("unknown-action-{id}-token-{token:?}");
            let mut request = json!({"recommendedActionId": id});
            if let Some(token) = token {
                request["clientToken"] = json!(format!("{token}-{id}"));
            }
            assert_refused(
                call(&router, "StartAutomationEvent", &request).await,
                404,
                "ResourceNotFoundException",
                &case,
            );
        }
    }

    // A seeded action is found, so the 404 above is a lookup and not a constant.
    let (seeded, engine) = emulator_with_engine();
    let action = recommended_action("AAAAAAAAAAAAAAAA", "UpgradeEbsVolumeType", "vol-0a1b2c3d");
    seed_action(&engine, &action);
    let started = call_ok(
        &seeded,
        "StartAutomationEvent",
        &json!({"recommendedActionId": "AAAAAAAAAAAAAAAA"}),
        "seeded-action",
    )
    .await;
    assert!(
        is_rule_id(&string_member(&started, "eventId")),
        "case \"seeded-action\": expected a minted eventId matching ^[0-9A-Za-z]{{16}}$, got {started}"
    );
}

/// A `recommendedActionId` outside the modelled `^[0-9A-Za-z]{16}$` is refused
/// on the parameter (400), which is a different answer from "no such action".
#[tokio::test]
async fn an_action_id_outside_its_modelled_form_is_a_parameter_rejection() {
    let router = emulator();

    // (case, recommendedActionId): one row per way the pattern can fail.
    let cases = [
        ("too-short", "AAAAAAAAAAAAAAA"),
        ("too-long", "AAAAAAAAAAAAAAAAA"),
        ("hyphen", "AAAAAAAAAAAAAAA-"),
        ("empty", ""),
    ];
    for (case, id) in cases {
        assert_refused(
            call(
                &router,
                "StartAutomationEvent",
                &json!({"recommendedActionId": id}),
            )
            .await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }
}

/// Starting an automation for a stored recommended action mints ONE automation
/// event, built from the action and readable through `GetAutomationEvent`; the
/// event is not terminal at the start call and reaches the modelled `Failed`
/// status, with the reason why, once the clock makes that due.
#[tokio::test]
async fn a_started_event_is_built_from_the_action_and_settles_on_the_clock() {
    let (router, engine) = emulator_with_engine();

    // Two actions differing in action type and resource, so what the event
    // reports is the action's own data rather than anything fixed.
    let actions = [
        recommended_action(
            "0123456789abcdef",
            "UpgradeEbsVolumeType",
            "vol-0123456789abcdef0",
        ),
        recommended_action(
            "fedcba9876543210",
            "SnapshotAndDeleteUnattachedEbsVolume",
            "vol-9f8e7d6c5b4a3210",
        ),
    ];
    for action in &actions {
        seed_action(&engine, action);
    }

    let mut event_ids = Vec::new();
    for action in &actions {
        let id = action.recommended_action_id.clone().unwrap_or_default();
        let case = format!("start-{id}");
        let started = call_ok(
            &router,
            "StartAutomationEvent",
            &json!({"recommendedActionId": &id}),
            &case,
        )
        .await;

        let event_id = string_member(&started, "eventId");
        assert!(
            is_rule_id(&event_id),
            "case {case:?}: expected an eventId matching ^[0-9A-Za-z]{{16}}$, got {started}"
        );
        assert_eq!(
            string_member(&started, "recommendedActionId"),
            id,
            "case {case:?}: the event must report the action it automates, got {started}"
        );
        assert_eq!(
            string_member(&started, "eventStatus"),
            "InProgress",
            "case {case:?}: a just-started automation is not terminal, got {started}"
        );

        // The event reads back with the action's own resource data.
        let event = call_ok(
            &router,
            "GetAutomationEvent",
            &json!({"eventId": &event_id}),
            &case,
        )
        .await;
        for (field, expected) in [
            ("eventId", event_id.clone()),
            ("recommendedActionId", id.clone()),
            (
                "eventType",
                action.recommended_action_type.clone().unwrap_or_default(),
            ),
            ("eventStatus", "InProgress".to_string()),
            ("resourceArn", action.resource_arn.clone().unwrap_or_default()),
            ("resourceId", action.resource_id.clone().unwrap_or_default()),
            (
                "resourceType",
                action.resource_type.clone().unwrap_or_default(),
            ),
            ("region", action.region.clone().unwrap_or_default()),
            ("accountId", action.account_id.clone().unwrap_or_default()),
        ] {
            assert_eq!(
                string_member(&event, field),
                expected,
                "case {case:?}: expected {field} {expected:?}, got {event}"
            );
        }
        assert!(
            member(&event, "createdTimestamp").is_number(),
            "case {case:?}: a stored event carries its creation time, got {event}"
        );
        assert!(
            member(&event, "completedTimestamp").is_null(),
            "case {case:?}: an event still in progress has not completed, got {event}"
        );
        event_ids.push(event_id);
    }

    // Two starts of the SAME action with no client token are two automations.
    let repeated = call_ok(
        &router,
        "StartAutomationEvent",
        &json!({"recommendedActionId": "0123456789abcdef"}),
        "second-start",
    )
    .await;
    assert!(
        !event_ids.contains(&string_member(&repeated, "eventId")),
        "case \"second-start\": a second start without a token mints its own event, got {repeated}"
    );

    // The terminal status the clock owes: this emulator performs no resource
    // automation, so the event fails with the reason rather than completing.
    std::thread::sleep(state::EVENT_SETTLEMENT_DELAY + Duration::from_millis(250));
    for event_id in &event_ids {
        let case = format!("settled-{event_id}");
        let event = call_ok(
            &router,
            "GetAutomationEvent",
            &json!({"eventId": event_id}),
            &case,
        )
        .await;
        assert_eq!(
            string_member(&event, "eventStatus"),
            "Failed",
            "case {case:?}: expected the modelled failure status once settled, got {event}"
        );
        assert_eq!(
            string_member(&event, "eventStatusReason"),
            state::EVENT_UNPERFORMED_REASON,
            "case {case:?}: a failed event states why, got {event}"
        );
        assert!(
            member(&event, "completedTimestamp").is_number(),
            "case {case:?}: a settled event carries its completion time, got {event}"
        );
    }
}

/// A client token makes a start idempotent: the same token replays the one event
/// it started, the same token with another action is a mismatch, and an event id
/// nothing minted is a 404.
#[tokio::test]
async fn a_client_token_makes_a_start_idempotent() {
    let (router, engine) = emulator_with_engine();
    for action in [
        recommended_action("1111111111111111", "UpgradeEbsVolumeType", "vol-1111"),
        recommended_action(
            "2222222222222222",
            "SnapshotAndDeleteUnattachedEbsVolume",
            "vol-2222",
        ),
    ] {
        seed_action(&engine, &action);
    }

    let request = json!({"recommendedActionId": "1111111111111111", "clientToken": "token-abc"});
    let first = call_ok(&router, "StartAutomationEvent", &request, "first").await;
    let replayed = call_ok(&router, "StartAutomationEvent", &request, "replay").await;
    assert_eq!(
        string_member(&first, "eventId"),
        string_member(&replayed, "eventId"),
        "case \"replay\": the same client token must replay one event, got {first} then {replayed}"
    );

    assert_refused(
        call(
            &router,
            "StartAutomationEvent",
            &json!({"recommendedActionId": "2222222222222222", "clientToken": "token-abc"}),
        )
        .await,
        409,
        "IdempotentParameterMismatchException",
        "token-reused-with-another-action",
    );

    assert_refused(
        call(
            &router,
            "GetAutomationEvent",
            &json!({"eventId": "9999999999999999"}),
        )
        .await,
        404,
        "ResourceNotFoundException",
        "unknown-event",
    );

    // A replay after the event has settled reports the status it settled to,
    // never the one the first call answered with.
    std::thread::sleep(state::EVENT_SETTLEMENT_DELAY + Duration::from_millis(250));
    let settled_replay = call_ok(&router, "StartAutomationEvent", &request, "replay-settled").await;
    assert_eq!(
        (
            string_member(&settled_replay, "eventId"),
            string_member(&settled_replay, "eventStatus")
        ),
        (string_member(&first, "eventId"), "Failed".to_string()),
        "case \"replay-settled\": expected the same event in its settled status, got {settled_replay}"
    );
}

// ---------------------------------------------------------------------------
// UntagResource
// ---------------------------------------------------------------------------

/// A `tagKeys` entry outside the modelled `TagKey` form is refused with
/// `InvalidParameterValueException` (400) and removes NOTHING, while a key of a
/// form the model admits is removed. `TagKeyList`'s member is a plain `String`,
/// so this rule is the service's own.
#[tokio::test]
async fn untag_judges_every_key_against_the_modelled_tag_key_form() {
    // Keys the `TagKey` pattern `^[\w\s\.\-\:\/\=\+\@]+$` with `@length(1, 128)`
    // ADMITS (`smithy_query(query="shape",shape="TagKey")`): a word, every
    // punctuation character of the class, whitespace, and the length bound.
    let valid_keys = [
        ("word", "team".to_string()),
        ("punctuation", "aws:cost/center=1+2@x.y-z_0".to_string()),
        ("whitespace", "cost center".to_string()),
        ("at-max-length", "k".repeat(128)),
    ];
    // Keys it REFUSES: a character outside the class, and either side of the
    // length bound.
    let invalid_keys = [
        // The recorded case.
        ("exclamation", "not a valid key!".to_string()),
        ("hash", "bad#key".to_string()),
        ("comma", "comma,key".to_string()),
        ("quote", "quote\"key".to_string()),
        ("empty", String::new()),
        ("past-max-length", "k".repeat(129)),
    ];

    for (case, key) in &valid_keys {
        let router = emulator();
        let (rule_arn, revision) = rule_with_tags(
            &router,
            "untag-valid",
            &json!([{"key": key, "value": "sim2real"}, {"key": "keep", "value": "me"}]),
            case,
        )
        .await;
        call_ok(
            &router,
            "UntagResource",
            &json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tagKeys": [key]}),
            case,
        )
        .await;
        let keys = stored_tag_keys(&router, &rule_arn, case).await;
        assert_eq!(
            keys,
            vec!["keep".to_string()],
            "case {case:?}: expected only the named key removed, got {keys:?}"
        );
    }

    for (case, key) in &invalid_keys {
        let router = emulator();
        let (rule_arn, revision) = rule_with_tags(
            &router,
            "untag-invalid",
            &json!([{"key": "team", "value": "sim2real"}]),
            case,
        )
        .await;
        // The malformed key alone, and beside a key the rule really carries:
        // either way the request is refused before anything is removed.
        for (variant, tag_keys) in [
            ("alone", vec![key.clone()]),
            ("with-a-valid-key", vec!["team".to_string(), key.clone()]),
        ] {
            let case = format!("{case}-{variant}");
            assert_refused(
                call(
                    &router,
                    "UntagResource",
                    &json!({
                        "resourceArn": &rule_arn,
                        "ruleRevision": revision,
                        "tagKeys": tag_keys
                    }),
                )
                .await,
                400,
                "InvalidParameterValueException",
                &case,
            );
            let keys = stored_tag_keys(&router, &rule_arn, &case).await;
            assert_eq!(
                keys,
                vec!["team".to_string()],
                "case {case:?}: a refused untag must leave every tag intact, got {keys:?}"
            );
        }
    }
}

/// `UntagResource` is checked against the rule's current revision, removes only
/// the keys it names, leaves the revision where it was (so the same request may
/// be repeated, as `@idempotent` requires), and answers 404 for a well-formed
/// ARN no rule has.
#[tokio::test]
async fn untag_is_revision_checked_and_repeatable() {
    let router = emulator();
    let (rule_arn, revision) = rule_with_tags(
        &router,
        "untag-revision",
        &json!([
            {"key": "team", "value": "sim2real"},
            {"key": "owner", "value": "platform"},
            {"key": "keep", "value": "me"}
        ]),
        "setup",
    )
    .await;

    // A revision other than the current one is refused, and nothing is removed.
    for (case, sent) in [("stale", revision - 1), ("ahead", revision + 1)] {
        let (status, body) = call(
            &router,
            "UntagResource",
            &json!({"resourceArn": &rule_arn, "ruleRevision": sent, "tagKeys": ["team"]}),
        )
        .await;
        assert_refused((status, body.clone()), 400, "InvalidParameterValueException", case);
        assert!(
            string_member(&body, "message").contains(&revision.to_string()),
            "case {case:?}: the rejection must state the current revision {revision}, got {body}"
        );
        let keys = stored_tag_keys(&router, &rule_arn, case).await;
        assert_eq!(
            keys,
            vec!["keep".to_string(), "owner".to_string(), "team".to_string()],
            "case {case:?}: a refused untag removes nothing, got {keys:?}"
        );
    }

    // Removing two keys at once, twice: the second call is the same request and
    // leaves the same state, because the removal does not advance the revision.
    for attempt in ["first", "repeat"] {
        call_ok(
            &router,
            "UntagResource",
            &json!({
                "resourceArn": &rule_arn,
                "ruleRevision": revision,
                "tagKeys": ["team", "owner"]
            }),
            attempt,
        )
        .await;
        let keys = stored_tag_keys(&router, &rule_arn, attempt).await;
        assert_eq!(
            keys,
            vec!["keep".to_string()],
            "case {attempt:?}: expected both named keys gone and the rest kept, got {keys:?}"
        );
        let rule = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            attempt,
        )
        .await;
        assert_eq!(
            member(&rule, "ruleRevision").as_i64(),
            Some(revision),
            "case {attempt:?}: removing a tag is not a rule revision, got {rule}"
        );
    }

    // A key the rule does not carry removes nothing and is still a success.
    call_ok(
        &router,
        "UntagResource",
        &json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tagKeys": ["absent"]}),
        "absent-key",
    )
    .await;
    let keys = stored_tag_keys(&router, &rule_arn, "absent-key").await;
    assert_eq!(
        keys,
        vec!["keep".to_string()],
        "case \"absent-key\": removing a key the rule never had changes nothing, got {keys:?}"
    );

    // A well-formed ARN no rule has is a 404, not a tag removal.
    assert_refused(
        call(
            &router,
            "UntagResource",
            &json!({
                "resourceArn": format!(
                    "arn:aws:compute-optimizer::{}:automation-rule/0000000000000000",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "ruleRevision": 1,
                "tagKeys": ["team"]
            }),
        )
        .await,
        404,
        "ResourceNotFoundException",
        "unknown-rule",
    );
}

// ---------------------------------------------------------------------------
// Authorization of the resource a request names
// ---------------------------------------------------------------------------

/// A resource string that names no automation rule of this service is refused
/// with the modelled `AccessDeniedException` (403) on EVERY operation that takes
/// one, implemented or not: authorization is evaluated against the string as
/// supplied, before the service parses it. A string that does name one is not
/// refused that way.
#[tokio::test]
async fn a_string_naming_no_rule_of_this_service_is_unauthorized() {
    let router = emulator();
    let (untouched_arn, _) = rule_with_tags(
        &router,
        "authorization-untouched",
        &json!([{"key": "team", "value": "sim2real"}]),
        "setup",
    )
    .await;

    let account = engine::EMULATOR_ACCOUNT_ID;
    // (case, resource string): one row per way the modelled `RuleArn` form
    // (`^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`)
    // can fail to name a rule of this service.
    let unauthorized = [
        ("not-an-arn", "not-an-arn".to_string()),
        ("empty", String::new()),
        ("arn-prefix-only", "arn:aws:compute-optimizer".to_string()),
        (
            "another-service",
            format!("arn:aws:ec2::{account}:automation-rule/abcdefghij123456"),
        ),
        (
            "account-too-short",
            "arn:aws:compute-optimizer::12345678901:automation-rule/abcdefghij123456".to_string(),
        ),
        (
            "region-filled-in",
            format!("arn:aws:compute-optimizer:us-east-1:{account}:automation-rule/abcdefghij12345"),
        ),
        (
            "another-resource-type",
            format!("arn:aws:compute-optimizer::{account}:rule/abcdefghij123456"),
        ),
        (
            "no-rule-id",
            format!("arn:aws:compute-optimizer::{account}:automation-rule/"),
        ),
        (
            "rule-id-with-a-space",
            format!("arn:aws:compute-optimizer::{account}:automation-rule/bad id"),
        ),
        (
            "uppercased",
            format!("ARN:AWS:COMPUTE-OPTIMIZER::{account}:AUTOMATION-RULE/abcdefghij123456"),
        ),
    ];

    /// The request `operation` takes for `resource`, with every other required
    /// member of that operation filled in.
    fn request_for(operation: &str, resource: &str, revision: i64) -> Value {
        match operation {
            "GetAutomationRule" => json!({"ruleArn": resource}),
            "UpdateAutomationRule" => {
                json!({"ruleArn": resource, "ruleRevision": revision, "status": "Inactive"})
            }
            "DeleteAutomationRule" => json!({"ruleArn": resource, "ruleRevision": revision}),
            "ListTagsForResource" => json!({"resourceArn": resource}),
            "TagResource" => json!({
                "resourceArn": resource,
                "ruleRevision": revision,
                "tags": [{"key": "team", "value": "sim2real"}]
            }),
            "UntagResource" => json!({
                "resourceArn": resource,
                "ruleRevision": revision,
                "tagKeys": ["team"]
            }),
            other => panic!("no request is defined for {other}"),
        }
    }

    // Every operation whose input names a resource by ARN: the member is
    // `ruleArn` on the rule operations and `resourceArn` on the tagging ones
    // (`smithy_query(query="operation",operation="...")`). Authorization
    // answers before dispatch, so the list is the same whether or not an
    // operation is implemented.
    let operations = [
        "GetAutomationRule",
        "UpdateAutomationRule",
        "DeleteAutomationRule",
        "ListTagsForResource",
        "TagResource",
        "UntagResource",
    ];

    for operation in operations {
        for (case, resource) in &unauthorized {
            let case = format!("{operation}-{case}");
            assert_refused(
                call(&router, operation, &request_for(operation, resource, 1)).await,
                403,
                "AccessDeniedException",
                &case,
            );
        }

        // The control, on a rule of this operation's own so a mutating one
        // cannot disturb another row: a string that DOES name a rule of this
        // service is never answered as unauthorized — it reaches the operation,
        // which answers on its own terms.
        let case = format!("{operation}-names-a-rule");
        let (own_arn, own_revision) = rule_with_tags(
            &router,
            &format!("control-{operation}"),
            &json!([{"key": "team", "value": "sim2real"}]),
            &case,
        )
        .await;
        let (status, body) = call(
            &router,
            operation,
            &request_for(operation, &own_arn, own_revision),
        )
        .await;
        assert_ne!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: a well-formed rule ARN is authorized, got {status} and {body}"
        );
    }

    // And the rule nothing addressed is still there, with its tag: not one of
    // the refused calls ran.
    let keys = stored_tag_keys(&router, &untouched_arn, "nothing-ran").await;
    assert_eq!(
        keys,
        vec!["team".to_string()],
        "case \"nothing-ran\": a refused request must change nothing, got {keys:?}"
    );
}

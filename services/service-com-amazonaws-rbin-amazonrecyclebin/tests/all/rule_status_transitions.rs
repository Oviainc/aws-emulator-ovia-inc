// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the two rule-record contracts a retention rule's
//! response has to keep:
//!
//! 1. STATUS. `RuleStatus` models exactly `[pending, available]` and "Only retention
//!    rules that are in the `available` state retain resources", so a rule that has
//!    just been created or modified is reported `pending` and reaches `available`
//!    because the CLOCK makes that transition due — never because somebody read it.
//!    Every reader that models `Status` reports the transitioned value, and the status
//!    never runs backwards.
//!    Source: smithy_query(query="shape", shape="RuleStatus,CreateRuleResponse,
//!    UpdateRuleResponse,GetRuleResponse").
//! 2. TAGS. `CreateRuleResponse$Tags` is the OPTIONAL member reporting "the tags
//!    assigned to the retention rule", so a rule carrying no tags has the member
//!    ABSENT rather than present as an empty list — while the resource-selection
//!    members (`ResourceTags`, `ExcludeResourceTags`) are part of a rule's record
//!    either way and are reported empty.
//!
//! Every case drives the service through the real router, exactly as a signed SDK
//! request would, and each row states its own case so a failure names the input.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::resource_tags::list_tags;
use crate::retention_rule_lifecycle::{
    create_rule, get_rule, lock_configuration, retention, router, update_rule,
};
use service_com_amazonaws_rbin_amazonrecyclebin::ops::types::RULE_STATUS_VALUES;
use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    MAX_TAGS_PER_RULE, RULE_STATUS_AVAILABLE, RULE_STATUS_PENDING, STATUS_TRANSITION,
};

/// `ResourceType` accepts exactly these values.
/// Source: smithy_query(query="shape", shape="ResourceType") — enum values.
const RESOURCE_TYPES: [&str; 3] = ["EBS_SNAPSHOT", "EBS_VOLUME", "EC2_IMAGE"];

/// Published bounds of `RetentionPeriod$RetentionPeriodValue`.
/// Source: smithy_query(query="shape", shape="RetentionPeriod") — range {min: 1, max: 3650}.
const RETENTION_VALUE_MIN: i64 = 1;
const RETENTION_VALUE_MAX: i64 = 3650;

/// Published bounds of `UnlockDelay$UnlockDelayValue`.
/// Source: smithy_query(query="shape", shape="UnlockDelay") — range {min: 7, max: 30}.
const UNLOCK_DELAY_MIN: i64 = 7;

/// Long enough that the `pending` -> `available` transition the clock owes is past due
/// for every rule created before the wait started.
fn past_the_transition() -> Duration {
    STATUS_TRANSITION + Duration::from_millis(250)
}

/// A response's `Status`, or `"<none>"` when the member is absent.
fn status_of(body: &Value) -> &str {
    body.get("Status").and_then(Value::as_str).unwrap_or("<none>")
}

/// A response's `Tags` as a key -> value map, or `None` when the member is ABSENT —
/// the difference this suite is about, so an absent member and an empty list are not
/// collapsed into one another.
fn tags_of(body: &Value) -> Option<BTreeMap<String, String>> {
    match body.get("Tags") {
        None | Some(Value::Null) => None,
        Some(Value::Array(list)) => Some(
            list.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
        ),
        Some(other) => panic!("Tags is neither absent nor a list: {other:?}"),
    }
}

/// One `CreateRule` body per shape of rule the service mints, so the status contract is
/// asserted over the whole class rather than over one recorded request: every modelled
/// resource type, both published retention bounds, tag-level and Region-level rules,
/// exclusion tags, rule tags, a description, and a rule locked at creation.
fn create_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "region-level snapshot rule at the minimum retention period",
            json!({
                "ResourceType": RESOURCE_TYPES[0],
                "RetentionPeriod": retention(RETENTION_VALUE_MIN),
            }),
        ),
        (
            "region-level volume rule at the maximum retention period",
            json!({
                "ResourceType": RESOURCE_TYPES[1],
                "RetentionPeriod": retention(RETENTION_VALUE_MAX),
                "Description": "keep volumes for ten years",
            }),
        ),
        (
            "tag-level image rule with resource tags and rule tags",
            json!({
                "ResourceType": RESOURCE_TYPES[2],
                "RetentionPeriod": retention(45),
                "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
        ),
        (
            "region-level snapshot rule with exclusion tags",
            json!({
                "ResourceType": RESOURCE_TYPES[0],
                "RetentionPeriod": retention(7),
                "ExcludeResourceTags": [{ "ResourceTagKey": "scratch", "ResourceTagValue": "1" }],
            }),
        ),
        (
            "volume rule locked at creation",
            json!({
                "ResourceType": RESOURCE_TYPES[1],
                "RetentionPeriod": retention(366),
                "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN),
            }),
        ),
    ]
}

#[tokio::test]
async fn every_status_this_service_reports_is_one_the_model_declares() {
    // One source of truth for the enum's values: the spellings the handlers name must
    // be members of the generated wire list, and together they must cover it — so a
    // model change cannot leave this service reporting an unmodelled status.
    for spelling in [RULE_STATUS_PENDING, RULE_STATUS_AVAILABLE] {
        assert!(
            RULE_STATUS_VALUES.contains(&spelling),
            "case {spelling:?}: RuleStatus declares {RULE_STATUS_VALUES:?}, \
             which does not include it"
        );
    }
    assert_eq!(
        RULE_STATUS_VALUES.len(),
        2,
        "RuleStatus declares {RULE_STATUS_VALUES:?}, so this suite no longer covers \
         every state a rule can report"
    );
}

#[tokio::test]
async fn a_created_rule_is_pending_and_the_clock_alone_makes_it_available() {
    let router = router();
    let mut minted = Vec::new();
    for (case, body) in create_bodies() {
        let started = Instant::now();
        let (status, created) = create_rule(&router, body.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateRule, got {status} / {created:?}"
        );
        // The create response is minted inside the transition span by construction, so
        // it reports `pending` for every shape of rule — no clock reading needed.
        assert_eq!(
            status_of(&created),
            RULE_STATUS_PENDING,
            "case {case:?}: expected a freshly created rule to report \
             Status={RULE_STATUS_PENDING}, got {:?}",
            created["Status"]
        );
        let identifier = created["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no Identifier"))
            .to_string();

        // A caller acting straight after the create still observes the transient state.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
        );
        if started.elapsed() < STATUS_TRANSITION {
            assert_eq!(
                status_of(&fetched),
                RULE_STATUS_PENDING,
                "case {case:?}: GetRule inside the transition span must report \
                 Status={RULE_STATUS_PENDING}, got {:?}",
                fetched["Status"]
            );
        }
        minted.push((case, identifier));
    }

    // Nobody touches the rules while the clock runs; the transition is owed anyway.
    tokio::time::sleep(past_the_transition()).await;
    for (case, identifier) in &minted {
        let (status, fetched) = get_rule(&router, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
        );
        assert_eq!(
            status_of(&fetched),
            RULE_STATUS_AVAILABLE,
            "case {case:?}: a rule past its transition span must report \
             Status={RULE_STATUS_AVAILABLE}, got {:?}",
            fetched["Status"]
        );
        // A read never moves the status, so reading again reports the same state.
        let (_, again) = get_rule(&router, identifier).await;
        assert_eq!(
            status_of(&again),
            RULE_STATUS_AVAILABLE,
            "case {case:?}: re-reading an available rule must keep it \
             {RULE_STATUS_AVAILABLE}, got {:?}",
            again["Status"]
        );
    }
}

#[tokio::test]
async fn update_rule_returns_the_rule_to_pending_until_the_clock_settles_it_again() {
    let router = router();
    // Each row modifies a different part of the rule; the rule under test holds for
    // every completed modification, whatever it changed.
    let patches: Vec<(&str, Value)> = vec![
        (
            "retention period at the minimum",
            json!({ "RetentionPeriod": retention(RETENTION_VALUE_MIN) }),
        ),
        (
            "retention period at the maximum",
            json!({ "RetentionPeriod": retention(RETENTION_VALUE_MAX) }),
        ),
        ("description only", json!({ "Description": "renamed" })),
        (
            "resource tags only",
            json!({
                "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "always" }]
            }),
        ),
        (
            "retention period and description together",
            json!({ "RetentionPeriod": retention(90), "Description": "both" }),
        ),
    ];

    for (case, patch) in patches {
        let (status, created) = create_rule(
            &router,
            json!({
                "ResourceType": RESOURCE_TYPES[0],
                "RetentionPeriod": retention(7),
                "Description": "bootstrap rule",
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: setup CreateRule returned {status} / {created:?}"
        );
        let identifier = created["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no Identifier"))
            .to_string();

        // Let the create settle first, so what the update reports is the update's doing.
        tokio::time::sleep(past_the_transition()).await;
        let (_, settled) = get_rule(&router, &identifier).await;
        assert_eq!(
            status_of(&settled),
            RULE_STATUS_AVAILABLE,
            "case {case:?}: setup expected the created rule to settle to \
             {RULE_STATUS_AVAILABLE}, got {:?}",
            settled["Status"]
        );

        let started = Instant::now();
        let (status, updated) = update_rule(&router, &identifier, patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateRule, got {status} / {updated:?}"
        );
        assert_eq!(
            status_of(&updated),
            RULE_STATUS_PENDING,
            "case {case:?}: expected a just-modified rule to report \
             Status={RULE_STATUS_PENDING}, got {:?}",
            updated["Status"]
        );
        let (_, fetched) = get_rule(&router, &identifier).await;
        if started.elapsed() < STATUS_TRANSITION {
            assert_eq!(
                status_of(&fetched),
                RULE_STATUS_PENDING,
                "case {case:?}: GetRule inside the transition span must report \
                 Status={RULE_STATUS_PENDING}, got {:?}",
                fetched["Status"]
            );
        }

        tokio::time::sleep(past_the_transition()).await;
        let (_, resettled) = get_rule(&router, &identifier).await;
        assert_eq!(
            status_of(&resettled),
            RULE_STATUS_AVAILABLE,
            "case {case:?}: a modified rule past its transition span must report \
             Status={RULE_STATUS_AVAILABLE}, got {:?}",
            resettled["Status"]
        );
    }
}

/// `(case, the request's Tags member, the tags the rule must end up carrying)`. A row
/// whose expected map is empty must come back with NO `Tags` member at all.
fn tag_rows() -> Vec<(&'static str, Option<Value>, BTreeMap<String, String>)> {
    let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
    // The quota the store holds one rule to, so the top of the count range is covered.
    // Source: `service_com_amazonaws_rbin_amazonrecyclebin::state::MAX_TAGS_PER_RULE`.
    let many: Vec<Value> = (0..MAX_TAGS_PER_RULE)
        .map(|nth| json!({ "Key": format!("key{nth}"), "Value": format!("value{nth}") }))
        .collect();
    vec![
        ("no Tags member at all", None, BTreeMap::new()),
        (
            "an explicitly empty Tags list",
            Some(json!([])),
            BTreeMap::new(),
        ),
        (
            "one tag",
            Some(json!([{ "Key": "env", "Value": "prod" }])),
            BTreeMap::from([pair("env", "prod")]),
        ),
        (
            "a tag with an empty value",
            Some(json!([{ "Key": "team", "Value": "" }])),
            BTreeMap::from([pair("team", "")]),
        ),
        (
            "two tags",
            Some(json!([
                { "Key": "env", "Value": "dev" },
                { "Key": "owner", "Value": "platform" },
            ])),
            BTreeMap::from([pair("env", "dev"), pair("owner", "platform")]),
        ),
        (
            "the same key twice keeps the last value",
            Some(json!([
                { "Key": "env", "Value": "dev" },
                { "Key": "env", "Value": "staging" },
            ])),
            BTreeMap::from([pair("env", "staging")]),
        ),
        (
            "tags up to the quota of tags per resource",
            Some(Value::Array(many)),
            (0..MAX_TAGS_PER_RULE)
                .map(|nth| (format!("key{nth}"), format!("value{nth}")))
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn create_rule_reports_the_tags_member_only_for_a_rule_that_carries_tags() {
    let router = router();
    for (case, requested, expected) in tag_rows() {
        let mut body = json!({
            "ResourceType": RESOURCE_TYPES[0],
            "RetentionPeriod": retention(7),
        });
        if let Some(tags) = requested {
            body["Tags"] = tags;
        }
        let (status, created) = create_rule(&router, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateRule, got {status} / {created:?}"
        );

        let reported = tags_of(&created);
        if expected.is_empty() {
            assert!(
                reported.is_none(),
                "case {case:?}: an untagged rule must omit the Tags member, got {:?}",
                created["Tags"]
            );
        } else {
            assert_eq!(
                reported.as_ref(),
                Some(&expected),
                "case {case:?}: expected CreateRule to report Tags {expected:?}, got {:?}",
                created["Tags"]
            );
        }

        // The resource-selection members are part of a rule's record either way, so
        // they stay present even when the rule names none of them.
        for member in ["ResourceTags", "ExcludeResourceTags"] {
            assert_eq!(
                created.get(member),
                Some(&json!([])),
                "case {case:?}: expected CreateRule to report an empty {member}, got {:?}",
                created.get(member)
            );
        }

        // ROUND TRIP: the omission is a serialization choice, not lost state —
        // `ListTagsForResource` reads the same tags back out of the store.
        let rule_arn = created["RuleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no RuleArn"));
        let (status, listed) = list_tags(&router, rule_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} / {listed:?}"
        );
        let listed_tags = tags_of(&listed).unwrap_or_default();
        assert_eq!(
            listed_tags, expected,
            "case {case:?}: expected ListTagsForResource to report {expected:?}, got {:?}",
            listed["Tags"]
        );
    }
}

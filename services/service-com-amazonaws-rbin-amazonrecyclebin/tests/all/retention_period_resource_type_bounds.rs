// SPDX-License-Identifier: CC0-1.0

//! The coupling between what a retention rule retains and how long it may retain it:
//! a `RetentionPeriod` is accepted only when its value is within the period the rule's
//! `ResourceType` supports, at both doors onto a rule's retention period — `CreateRule`,
//! where the request names the type, and `UpdateRule`, where the type is the stored one.
//!
//! The rule is asserted over a TABLE spanning the class: every modelled resource type,
//! each at the minimum, just inside its published maximum, at that maximum, one day past
//! it, at the other type's maximum and at the endpoint of the model's own looser
//! `@range`. The harness (`router`, `call`, the per-operation helpers) is shared with
//! `retention_rule_lifecycle`, which owns it, so every case drives the service through
//! the real router.

use service_com_amazonaws_rbin_amazonrecyclebin::state::max_retention_period_days;

use super::retention_rule_lifecycle::{
    RESOURCE_TYPES, RETENTION_VALUE_MAX, RETENTION_VALUE_MIN, create_rule, get_rule, list_of,
    list_rules, retention, router, update_rule,
};

/// The longest retention period, in days, each modelled resource type supports.
///
/// Source: smithy_query(query="doc", shape="RetentionPeriod", member="RetentionPeriodValue")
/// — "The supported retention periods are: EBS volumes: 1 - 7 days / EBS snapshots and
/// EBS-backed AMIs: 1 - 365 days". The member's own `@range` is the looser
/// {min: 1, max: 3650}, the union over the types, so it admits periods no single type
/// supports.
const SUPPORTED_MAXIMUM: [(&str, i64); 3] = [
    ("EBS_VOLUME", 7),
    ("EBS_SNAPSHOT", 365),
    ("EC2_IMAGE", 365),
];

/// The published maximum for `resource_type`, as the table above states it.
fn supported_maximum(resource_type: &str) -> i64 {
    SUPPORTED_MAXIMUM
        .iter()
        .find(|(named, _)| *named == resource_type)
        .map(|(_, max)| *max)
        .unwrap_or_else(|| panic!("resource type {resource_type:?} has no published maximum"))
}

/// One retention period to try against `resource_type`, and the status it must answer.
struct Row {
    case: String,
    value: i64,
    expected: u16,
}

/// Every retention period this suite tries against `resource_type`: each published bound,
/// a value inside it, the value just past it, the OTHER type's maximum and the endpoint of
/// the model's own range.
fn rows_for(resource_type: &str) -> Vec<Row> {
    let max = supported_maximum(resource_type);
    let other = SUPPORTED_MAXIMUM
        .iter()
        .map(|(_, bound)| *bound)
        .find(|bound| *bound != max)
        .expect("the published maxima differ between resource types");
    let mut rows = vec![
        Row {
            case: format!("{resource_type} at the minimum retention period"),
            value: RETENTION_VALUE_MIN,
            expected: 201,
        },
        Row {
            case: format!("{resource_type} one day inside its supported maximum"),
            value: max - 1,
            expected: 201,
        },
        Row {
            case: format!("{resource_type} at its supported maximum"),
            value: max,
            expected: 201,
        },
        Row {
            case: format!("{resource_type} one day past its supported maximum"),
            value: max + 1,
            expected: 400,
        },
        Row {
            case: format!("{resource_type} at the endpoint of the model's own range"),
            value: RETENTION_VALUE_MAX,
            expected: 400,
        },
    ];
    // The maximum published for ANOTHER resource type: accepted only when this type's own
    // maximum leaves room for it, so the bound is read off the type rather than shared.
    rows.push(Row {
        case: format!("{resource_type} at the maximum published for another type ({other})"),
        value: other,
        expected: if other <= max { 201 } else { 400 },
    });
    rows
}

/// The message of an error body, whichever casing the envelope used.
fn message_of(body: &serde_json::Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Assert a refusal is the modelled `ValidationException` that names the retention
/// period's member path and the bound `resource_type` publishes.
fn assert_refused_for_type(case: &str, resource_type: &str, body: &serde_json::Value) {
    assert_eq!(
        body["Reason"].as_str(),
        Some("INVALID_PARAMETER_VALUE"),
        "case {case:?}: expected Reason=INVALID_PARAMETER_VALUE, got {:?}",
        body["Reason"]
    );
    let message = message_of(body);
    let expected = format!(
        "1 validation error detected: Value at \
         'retentionPeriod.retentionPeriodValue' failed to satisfy constraint: Member must \
         have value less than or equal to {}",
        supported_maximum(resource_type)
    );
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message to state the bound {resource_type} publishes, \
         got {message:?}"
    );
}

/// How many rules of `resource_type` the store holds.
async fn rule_count(router: &smithy_server_runtime::MultiServiceRouter, resource_type: &str) -> usize {
    let (status, listed) = list_rules(
        router,
        serde_json::json!({ "ResourceType": resource_type, "MaxResults": 1000 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListRules({resource_type}) must answer HTTP 200, got {status} / {listed:?}"
    );
    list_of(&listed, "Rules").len()
}

#[tokio::test]
async fn the_handler_reads_the_published_maximum_off_the_resource_type() {
    for (resource_type, published) in SUPPORTED_MAXIMUM {
        let served = max_retention_period_days(resource_type).map(i64::from);
        assert_eq!(
            served,
            Some(published),
            "case {resource_type:?}: expected the published maximum {published}, got {served:?}"
        );
    }
    // Every modelled resource type carries a published maximum, so none is left to the
    // model's looser range by accident.
    for resource_type in RESOURCE_TYPES {
        assert!(
            max_retention_period_days(resource_type).is_some(),
            "case {resource_type:?}: every modelled resource type must publish a maximum"
        );
    }
}

#[tokio::test]
async fn create_rule_accepts_only_a_retention_period_the_requested_type_supports() {
    let router = router();
    for resource_type in RESOURCE_TYPES {
        for Row {
            case,
            value,
            expected,
        } in rows_for(resource_type)
        {
            let before = rule_count(&router, resource_type).await;
            let (status, body) = create_rule(
                &router,
                serde_json::json!({
                    "ResourceType": resource_type,
                    "RetentionPeriod": retention(value),
                }),
            )
            .await;
            assert_eq!(
                status, expected,
                "case {case:?} (RetentionPeriodValue={value}): expected HTTP {expected}, \
                 got {status} / {body:?}"
            );
            let after = rule_count(&router, resource_type).await;
            if expected == 400 {
                assert_refused_for_type(&case, resource_type, &body);
                // A refused create mints nothing: the store holds exactly what it held.
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused CreateRule must mint no rule, \
                     got {after} rules where there were {before}"
                );
                continue;
            }
            assert_eq!(
                after,
                before + 1,
                "case {case:?}: an accepted CreateRule must mint one rule, \
                 got {after} rules where there were {before}"
            );
            // ROUND TRIP: the accepted period is the one a later read reports.
            let identifier = body["Identifier"].as_str().unwrap_or_default().to_string();
            let (status, fetched) = get_rule(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
            );
            assert_eq!(
                fetched["RetentionPeriod"]["RetentionPeriodValue"].as_i64(),
                Some(value),
                "case {case:?}: expected GetRule to report the stored period {value}, got {:?}",
                fetched["RetentionPeriod"]
            );
            assert_eq!(
                fetched["ResourceType"].as_str(),
                Some(resource_type),
                "case {case:?}: expected GetRule to report ResourceType={resource_type}, got {:?}",
                fetched["ResourceType"]
            );
        }
    }
}

#[tokio::test]
async fn update_rule_accepts_only_a_retention_period_the_stored_type_supports() {
    let router = router();
    for resource_type in RESOURCE_TYPES {
        for Row {
            case,
            value,
            expected,
        } in rows_for(resource_type)
        {
            // The rule to patch starts at the minimum period, which every type supports.
            let (status, created) = create_rule(
                &router,
                serde_json::json!({
                    "ResourceType": resource_type,
                    "RetentionPeriod": retention(RETENTION_VALUE_MIN),
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: the rule to patch must be created, got {status} / {created:?}"
            );
            let identifier = created["Identifier"].as_str().unwrap_or_default().to_string();

            let (status, body) = update_rule(
                &router,
                &identifier,
                serde_json::json!({ "RetentionPeriod": retention(value) }),
            )
            .await;
            // `UpdateRule` answers 200 where `CreateRule` answers 201; the refusal is the
            // same 400 for the same reason.
            let expected_update = if expected == 201 { 200 } else { expected };
            assert_eq!(
                status, expected_update,
                "case {case:?} (RetentionPeriodValue={value}): expected HTTP \
                 {expected_update}, got {status} / {body:?}"
            );
            let settled = if expected_update == 200 {
                value
            } else {
                assert_refused_for_type(&case, resource_type, &body);
                RETENTION_VALUE_MIN
            };
            // A refused update leaves the stored period exactly as it was; an accepted one
            // is readable back.
            let (status, fetched) = get_rule(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
            );
            assert_eq!(
                fetched["RetentionPeriod"]["RetentionPeriodValue"].as_i64(),
                Some(settled),
                "case {case:?}: expected GetRule to report the period {settled}, got {:?}",
                fetched["RetentionPeriod"]
            );
        }
    }
}

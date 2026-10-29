// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the one rule every refused `UpdateRule` is held to: a
//! modification the service could never hold is refused BEFORE anything is written, so
//! the stored rule the request addressed is left exactly as it was — every member it
//! carried AND the status it had reached.
//!
//! The refusal class this suite leads with is the coupling between what a rule retains
//! and how long it may retain it. `RetentionPeriod$RetentionPeriodValue` publishes a
//! per-type period ("The supported retention periods are: EBS volumes: 1 - 7 days / EBS
//! snapshots and EBS-backed AMIs: 1 - 365 days"), while the member's own `@range` is the
//! looser union {min: 1, max: 3650}; `UpdateRuleRequest$ResourceType` is "currently not
//! supported. You can't update a retention rule's resource type after creation", so the
//! type the bound is read against is the STORED one.
//! Source: smithy_query(query="doc", shape="RetentionPeriod",
//! member="RetentionPeriodValue") and smithy_query(query="doc", shape="UpdateRuleRequest",
//! member="ResourceType").
//!
//! `tests/all/retention_period_resource_type_bounds.rs` owns which periods each type
//! admits, at both doors onto a rule's period. What this suite adds is what a refusal
//! must leave behind: the stored record is untouched, including the status, so a request
//! the service refuses never knocks an `available` rule back to `pending` and never
//! applies the members that travelled beside the refused one. A second refusal class —
//! a merged resource selection naming both tag sets — is driven through the same
//! assertions, so the rule under test is the general one about a refused `UpdateRule`
//! rather than one validation check's own doing. (The resource-tag-pair quota refusal is
//! held to the same invariant by `tests/all/resource_tag_pair_quota.rs`, and the
//! lock-protected refusal by `tests/all/update_rule_lock_invariant.rs`.)
//!
//! Every case drives the service through the real router, exactly as a signed SDK request
//! would, and every row states its own case so a failure names the input.

use std::time::Duration;

use serde_json::{json, Value};

use crate::error_response_bodies::{error_code, message, one_validation_error};
use crate::retention_rule_lifecycle::{
    create_rule, get_rule, retention, router, supported_retention_max, update_rule,
    RESOURCE_TYPES, RETENTION_VALUE_MAX, RETENTION_VALUE_MIN,
};
use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    RULE_STATUS_AVAILABLE, RULE_STATUS_PENDING, STATUS_TRANSITION,
};

/// The member path a refused retention period is named by, as a validation clause spells
/// a member: each modelled member name in lower camel case, joined by dots.
/// Source: smithy_query(query="operation", operation="UpdateRule") — input member
/// `RetentionPeriod`, whose `RetentionPeriodValue` carries the bound.
const RETENTION_PERIOD_VALUE_MEMBER: &str = "retentionPeriod.retentionPeriodValue";

/// The member path a refused `ExcludeResourceTags` is named by.
/// Source: smithy_query(query="operation", operation="UpdateRule") — input member
/// `ExcludeResourceTags`.
const EXCLUDE_RESOURCE_TAGS_MEMBER: &str = "excludeResourceTags";

/// The requirement an exclusion tag fails on a rule that selects by resource tags:
/// exclusion tags are "[Region-level retention rules only] ... You can't specify exclusion
/// tags for tag-level retention rules" (smithy_query(query="doc",
/// shape="UpdateRuleRequest", member="ExcludeResourceTags")).
const TAG_LEVEL_REQUIREMENT: &str = "Member must not be specified for a tag-level retention rule";

/// The fault class a refused parameter value is reported under.
/// Source: smithy_query(query="shape", shape="ValidationExceptionReason").
const INVALID_PARAMETER_VALUE: &str = "INVALID_PARAMETER_VALUE";

/// Long enough that the `pending` -> `available` transition the clock owes a rule
/// modified before the wait started is past due.
fn past_the_transition() -> Duration {
    STATUS_TRANSITION + Duration::from_millis(250)
}

/// One `UpdateRule` body the service must refuse, and the message the refusal must carry.
struct Refusal {
    case: String,
    patch: Value,
    expected_message: String,
}

/// Every refused patch this suite sends to a stored rule of `resource_type`: a period one
/// day past the bound its type publishes, a period at the endpoint of the model's own
/// looser range, the same unsupported period travelling beside members that would
/// otherwise be applied, and a merged selection no rule can hold.
fn refusals_for(resource_type: &str) -> Vec<Refusal> {
    let max = supported_retention_max(resource_type);
    let past_the_bound = one_validation_error(
        RETENTION_PERIOD_VALUE_MEMBER,
        &format!("Member must have value less than or equal to {max}"),
    );
    vec![
        Refusal {
            case: format!("{resource_type}: one day past the period its stored type supports"),
            patch: json!({ "RetentionPeriod": retention(max + 1) }),
            expected_message: past_the_bound.clone(),
        },
        Refusal {
            case: format!("{resource_type}: at the endpoint of the model's own range"),
            patch: json!({ "RetentionPeriod": retention(RETENTION_VALUE_MAX) }),
            expected_message: past_the_bound.clone(),
        },
        Refusal {
            case: format!(
                "{resource_type}: an unsupported period beside members that would otherwise apply"
            ),
            patch: json!({
                "RetentionPeriod": retention(max + 1),
                "Description": "a description a refused request must not apply",
                "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "later" }],
            }),
            expected_message: past_the_bound,
        },
        Refusal {
            case: format!(
                "{resource_type}: exclusion tags onto a rule that selects by resource tags"
            ),
            patch: json!({
                "ExcludeResourceTags": [{ "ResourceTagKey": "scratch", "ResourceTagValue": "1" }],
            }),
            expected_message: one_validation_error(
                EXCLUDE_RESOURCE_TAGS_MEMBER,
                TAG_LEVEL_REQUIREMENT,
            ),
        },
    ]
}

/// Assert one refusal: the modelled `ValidationException`, its fault class and the clause
/// naming the member that could not hold and the requirement it failed.
fn assert_refused(case: &str, status: u16, body: &Value, expected_message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {body:?}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(INVALID_PARAMETER_VALUE),
        "case {case:?}: expected Reason={INVALID_PARAMETER_VALUE}, got {:?}",
        body["Reason"]
    );
    assert_eq!(
        message(body),
        expected_message,
        "case {case:?}: expected the message {expected_message:?}, got {:?}",
        message(body)
    );
}

#[tokio::test]
async fn a_refused_update_leaves_the_stored_rule_exactly_as_it_was() {
    let router = router();
    for resource_type in RESOURCE_TYPES {
        let max = supported_retention_max(resource_type);
        // The rule under test starts at the longest period its type supports — the bound
        // itself is accepted — and carries every member a refusal could disturb.
        let (status, created) = create_rule(
            &router,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(max),
                "Description": "the rule a refused update must leave alone",
                "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {resource_type:?}: the rule to patch must be created, got {status} / {created:?}"
        );
        let identifier = created["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {resource_type:?}: CreateRule reported no Identifier"))
            .to_string();

        // Let the create settle, so a refused update that wrongly re-entered `pending`
        // would show in the status a later reader sees.
        tokio::time::sleep(past_the_transition()).await;
        let (status, baseline) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {resource_type:?}: expected HTTP 200 from GetRule, got {status} / {baseline:?}"
        );
        assert_eq!(
            baseline["Status"].as_str(),
            Some(RULE_STATUS_AVAILABLE),
            "case {resource_type:?}: the rule to patch must have settled to \
             {RULE_STATUS_AVAILABLE}, got {:?}",
            baseline["Status"]
        );

        for Refusal {
            case,
            patch,
            expected_message,
        } in refusals_for(resource_type)
        {
            let (status, body) = update_rule(&router, &identifier, patch.clone()).await;
            assert_refused(&case, status, &body, &expected_message);

            // The whole stored record is what it was before the refused request: the
            // period, the description, both selections and the status the clock had
            // already reached.
            let (status, fetched) = get_rule(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
            );
            assert_eq!(
                fetched, baseline,
                "case {case:?}: a refused UpdateRule must leave the stored rule {baseline:?}, \
                 got {fetched:?}"
            );
        }

        // The complement: an update the stored type DOES support is applied, so the
        // invariant above is the refusal's doing and not an `UpdateRule` that writes
        // nothing at all.
        let (status, updated) = update_rule(
            &router,
            &identifier,
            json!({
                "RetentionPeriod": retention(RETENTION_VALUE_MIN),
                "Description": "an accepted update",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {resource_type:?}: an update inside the stored type's bound must be \
             accepted, got {status} / {updated:?}"
        );
        assert_eq!(
            updated["Status"].as_str(),
            Some(RULE_STATUS_PENDING),
            "case {resource_type:?}: a just-modified rule must report \
             Status={RULE_STATUS_PENDING}, got {:?}",
            updated["Status"]
        );
        // ROUND TRIP: the accepted members are the ones a later read reports.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {resource_type:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["RetentionPeriod"]["RetentionPeriodValue"].as_i64(),
            Some(RETENTION_VALUE_MIN),
            "case {resource_type:?}: expected the accepted period {RETENTION_VALUE_MIN}, got {:?}",
            fetched["RetentionPeriod"]
        );
        assert_eq!(
            fetched["Description"].as_str(),
            Some("an accepted update"),
            "case {resource_type:?}: expected the accepted description, got {:?}",
            fetched["Description"]
        );
    }
}

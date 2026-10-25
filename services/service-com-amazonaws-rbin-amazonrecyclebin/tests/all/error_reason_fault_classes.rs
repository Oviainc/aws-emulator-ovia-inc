// SPDX-License-Identifier: CC0-1.0

//! RULE: an error body carries only the members the service populates for the condition
//! it reports, and where it populates a `Reason` that value is one the model's own enum
//! for that error declares — so a reported reason is never invented, and a member the
//! service leaves unset is never synthesized from the enum.
//!
//! The suites around this one assert each error's STATUS and MESSAGE; this one asserts
//! the reason member alone, across every modelled error of the service:
//! `every_modelled_error_reports_its_own_fault_class` spans the four modelled errors,
//! each raised by a different operation, and checks each reports a reason its own enum
//! declares — or, for the 404 of a rule this Region does not hold, no `Reason` at all,
//! which is the member this service does not populate.
//! `tests/all/not_found_error_body_members.rs` asserts that 404 body over the whole class
//! of rule-addressed operations and forms of absence.
//!
//! Every case drives the service through the real router, so the wire protocol, the route
//! table, the input-validation gate and the handlers are exercised exactly as a signed
//! SDK request would exercise them.

use serde_json::{json, Value};

use service_com_amazonaws_rbin_amazonrecyclebin::ops::types::{
    CONFLICT_EXCEPTION_REASON_VALUES, SERVICE_QUOTA_EXCEEDED_EXCEPTION_REASON_VALUES,
    VALIDATION_EXCEPTION_REASON_VALUES,
};
use service_com_amazonaws_rbin_amazonrecyclebin::state::MAX_TAGS_PER_RULE;

use crate::error_response_bodies::{created_rule, error_code};
use crate::retention_rule_lifecycle::{
    UNLOCK_DELAY_MIN, create_rule, get_rule, lock_configuration, retention, router, update_rule,
};
use crate::rule_lock_transitions::lock_rule;

/// The `Reason` an error body reports, or `None` where the member is absent or null.
fn reason(body: &Value) -> Option<&str> {
    body.get("Reason").and_then(Value::as_str)
}

/// The sole value a single-value reason enum declares, taken from the model's own value
/// set rather than written out here — so a value set that grows a second member fails
/// this instead of silently narrowing the assertion.
fn sole_value(enum_name: &str, values: &'static [&'static str]) -> &'static str {
    assert_eq!(
        values.len(),
        1,
        "{enum_name} is expected to declare exactly one value, got {values:?}"
    );
    values[0]
}

/// One row of the fault-class table: the refusal a request answered with, and the reason
/// enum the model declares for that error.
struct FaultClassRow {
    case: &'static str,
    status: u16,
    body: Value,
    expected_status: u16,
    expected_code: &'static str,
    /// The reason this error must report, or `None` for the one error whose `Reason` this
    /// service does not populate.
    expected_reason: Option<&'static str>,
    /// The values the model's own enum for this error declares, which a REPORTED reason
    /// must be one of.
    modelled_values: &'static [&'static str],
}

/// RULE: each modelled error of this service reports a reason its own enum declares —
/// the single value where the enum names one, and the fault class the condition belongs
/// to where it names several — except the 404 for an addressed rule this Region does not
/// hold, whose optional `Reason` the service does not populate at all.
#[tokio::test]
async fn every_modelled_error_reports_its_own_fault_class() {
    let router = router();

    // A tag-level rule, which cannot be locked: the conflict condition.
    let (tag_level, _) = created_rule(
        &router,
        "a tag-level rule",
        json!({
            "ResourceType": "EC2_IMAGE",
            "RetentionPeriod": retention(10),
            "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
        }),
    )
    .await;

    let mut rows: Vec<FaultClassRow> = Vec::new();

    let (status, body) = get_rule(&router, "0000000000x").await;
    rows.push(FaultClassRow {
        case: "GetRule for an identifier no rule holds",
        status,
        body,
        expected_status: 404,
        expected_code: "ResourceNotFoundException",
        // The one error body of this service that carries no `Reason`; the whole class of
        // rule-addressed 404s is asserted in
        // `tests/all/not_found_error_body_members.rs`.
        expected_reason: None,
        modelled_values: &[],
    });

    let (status, body) = lock_rule(
        &router,
        &tag_level,
        json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN) }),
    )
    .await;
    rows.push(FaultClassRow {
        case: "LockRule on a rule whose own state forbids it",
        status,
        body,
        expected_status: 409,
        expected_code: "ConflictException",
        expected_reason: Some(sole_value(
            "ConflictExceptionReason",
            CONFLICT_EXCEPTION_REASON_VALUES,
        )),
        modelled_values: CONFLICT_EXCEPTION_REASON_VALUES,
    });

    let over_quota: Vec<(String, String)> = (0..=MAX_TAGS_PER_RULE)
        .map(|nth| (format!("k{nth}"), format!("v{nth}")))
        .collect();
    let (status, body) = create_rule(
        &router,
        json!({
            "ResourceType": "EBS_VOLUME",
            "RetentionPeriod": retention(7),
            "Tags": over_quota
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect::<Vec<_>>(),
        }),
    )
    .await;
    rows.push(FaultClassRow {
        case: "CreateRule with one tag past the tags-per-rule quota",
        status,
        body,
        expected_status: 402,
        expected_code: "ServiceQuotaExceededException",
        expected_reason: Some(sole_value(
            "ServiceQuotaExceededExceptionReason",
            SERVICE_QUOTA_EXCEEDED_EXCEPTION_REASON_VALUES,
        )),
        modelled_values: SERVICE_QUOTA_EXCEEDED_EXCEPTION_REASON_VALUES,
    });

    // The one reason enum of this model that names TWO fault classes: a parameter value
    // outside a modelled constraint is the parameter-value class.
    let (status, body) = update_rule(
        &router,
        &tag_level,
        json!({ "Description": "tab\tseparated" }),
    )
    .await;
    rows.push(FaultClassRow {
        case: "UpdateRule with a description outside its pattern",
        status,
        body,
        expected_status: 400,
        expected_code: "ValidationException",
        expected_reason: Some("INVALID_PARAMETER_VALUE"),
        modelled_values: VALIDATION_EXCEPTION_REASON_VALUES,
    });

    for FaultClassRow {
        case,
        status,
        body,
        expected_status,
        expected_code,
        expected_reason,
        modelled_values,
    } in &rows
    {
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} / {body:?}"
        );
        assert_eq!(
            &error_code(body),
            expected_code,
            "case {case:?}: expected the modelled {expected_code}, got {body:?}"
        );
        assert_eq!(
            reason(body),
            *expected_reason,
            "case {case:?}: expected Reason {expected_reason:?}, got {:?}",
            body.get("Reason")
        );
        if let Some(stated) = reason(body) {
            assert!(
                modelled_values.contains(&stated),
                "case {case:?}: Reason {stated:?} is outside the modelled value set \
                 {modelled_values:?}"
            );
        }
    }
}

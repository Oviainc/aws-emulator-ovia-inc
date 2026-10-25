// SPDX-License-Identifier: CC0-1.0

//! RULE: every modelled error of this service reports, in its `Reason` member, the fault
//! class its reason enum names for the condition that was refused — so an error whose
//! reason enum declares exactly ONE value carries that value on every refusal of that
//! class, and never leaves the member absent.
//!
//! The suites around this one assert each error's STATUS and MESSAGE; this one asserts
//! the reason member alone, across every modelled error of the service and every
//! operation that raises it:
//!
//! 1. `an_absent_rule_reports_the_one_value_its_reason_enum_names` drives all EIGHT
//!    rule-addressed operations — the five addressed by `Identifier` and the three
//!    addressed by `ResourceArn` — over identifiers that differ in form (lowercase,
//!    uppercase, mixed, all digits) and in why they name no rule (never minted, or
//!    deleted), plus well-formed ARNs whose Region, account or rule id names no rule this
//!    Region holds.
//! 2. `every_modelled_error_reports_its_own_fault_class` spans the four modelled errors
//!    of the service, each raised by a different operation, and checks the reason it
//!    reports is one the model's own enum for that error declares.
//!
//! Every case drives the service through the real router, so the wire protocol, the route
//! table, the input-validation gate and the handlers are exercised exactly as a signed
//! SDK request would exercise them.

use serde_json::{json, Value};

use service_com_amazonaws_rbin_amazonrecyclebin::ops::types::{
    CONFLICT_EXCEPTION_REASON_VALUES, RESOURCE_NOT_FOUND_EXCEPTION_REASON_VALUES,
    SERVICE_QUOTA_EXCEEDED_EXCEPTION_REASON_VALUES, VALIDATION_EXCEPTION_REASON_VALUES,
};
use service_com_amazonaws_rbin_amazonrecyclebin::state::MAX_TAGS_PER_RULE;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_response_bodies::{created_rule, error_code, with_arn_segment};
use crate::resource_tags::{keys, list_tags, pairs, tag_resource, untag_resource};
use crate::retention_rule_lifecycle::{
    UNLOCK_DELAY_MIN, create_rule, delete_rule, get_rule, lock_configuration, retention, router,
    update_rule,
};
use crate::rule_lock_transitions::{lock_rule, unlock_rule};

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

/// Every response the eight rule-addressed operations give for a rule addressed by
/// `identifier` (and by `arn`, for the three ARN-addressed ones), keyed by operation.
async fn rule_addressed_refusals(
    router: &MultiServiceRouter,
    identifier: &str,
    arn: &str,
) -> Vec<(&'static str, (u16, Value))> {
    vec![
        ("GetRule", get_rule(router, identifier).await),
        (
            "UpdateRule",
            update_rule(router, identifier, json!({ "Description": "anything" })).await,
        ),
        ("DeleteRule", delete_rule(router, identifier).await),
        (
            "LockRule",
            lock_rule(
                router,
                identifier,
                json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN) }),
            )
            .await,
        ),
        ("UnlockRule", unlock_rule(router, identifier).await),
        ("ListTagsForResource", list_tags(router, arn).await),
        (
            "TagResource",
            tag_resource(router, arn, &pairs(&[("team", "platform")])).await,
        ),
        (
            "UntagResource",
            untag_resource(router, arn, &keys(&["team"])).await,
        ),
    ]
}

/// RULE, part 1: a 404 for a rule this Region does not hold reports the one value
/// `ResourceNotFoundExceptionReason` declares, whichever operation addressed the rule and
/// however the request addressed it.
#[tokio::test]
async fn an_absent_rule_reports_the_one_value_its_reason_enum_names() {
    let router = router();
    let expected = sole_value(
        "ResourceNotFoundExceptionReason",
        RESOURCE_NOT_FOUND_EXCEPTION_REASON_VALUES,
    );

    // A rule that exists, so no refusal below comes from an empty store, and its ARN as
    // the template every absent rule's ARN is built from.
    let (held, held_arn) = created_rule(
        &router,
        "a rule this Region holds",
        json!({ "ResourceType": "EBS_SNAPSHOT", "RetentionPeriod": retention(7) }),
    )
    .await;
    // One created and then deleted: the second way a rule can be absent.
    let (deleted, _) = created_rule(
        &router,
        "a rule that is then deleted",
        json!({ "ResourceType": "EBS_VOLUME", "RetentionPeriod": retention(9) }),
    )
    .await;
    let (status, _) = delete_rule(&router, &deleted).await;
    assert_eq!(status, 204, "setup: DeleteRule expected HTTP 204, got {status}");

    let rows = [
        ("never minted, all lowercase", "zzzzzzzzzzz".to_string()),
        ("never minted, all uppercase", "ZZZZZZZZZZZ".to_string()),
        ("never minted, mixed case", "0a1B2c3D4e5".to_string()),
        ("never minted, all digits", "99999999999".to_string()),
        ("deleted", deleted.clone()),
    ];

    for (why, identifier) in &rows {
        // The same rule, addressed the way a tag operation addresses it.
        let arn = held_arn.replace(&held, identifier);
        for (operation, (status, body)) in
            rule_addressed_refusals(&router, identifier, &arn).await
        {
            let case = format!("{operation} on an identifier {why} ({identifier})");
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} / {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected the modelled ResourceNotFoundException, got {body:?}"
            );
            assert_eq!(
                reason(&body),
                Some(expected),
                "case {case:?}: expected Reason {expected:?}, got {:?}",
                body.get("Reason")
            );
        }
    }

    // The same rule, absent for a reason only an ARN can express: a well-formed rule ARN
    // whose Region or account names no rule this Region and account hold.
    let other_region = with_arn_segment(&held_arn, 3, "us-gov-east-1");
    let other_account = with_arn_segment(&held_arn, 4, "111122223333");
    for (why, absent_arn) in [
        ("another Region", other_region.as_str()),
        ("another account", other_account.as_str()),
    ] {
        for (operation, (status, body)) in [
            ("ListTagsForResource", list_tags(&router, absent_arn).await),
            (
                "TagResource",
                tag_resource(&router, absent_arn, &pairs(&[("team", "platform")])).await,
            ),
            (
                "UntagResource",
                untag_resource(&router, absent_arn, &keys(&["team"])).await,
            ),
        ] {
            let case = format!("{operation} on an ARN naming {why} ({absent_arn})");
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} / {body:?}"
            );
            assert_eq!(
                reason(&body),
                Some(expected),
                "case {case:?}: expected Reason {expected:?}, got {:?}",
                body.get("Reason")
            );
        }
    }

    // The rule that DOES exist still reads back, so none of the refusals above touched it.
    let (status, fetched) = get_rule(&router, &held).await;
    assert_eq!(
        status, 200,
        "case \"the held rule after every refusal\": expected HTTP 200, got {status} / {fetched:?}"
    );
}

/// One row of the fault-class table: the refusal a request answered with, and the reason
/// enum the model declares for that error.
struct FaultClassRow {
    case: &'static str,
    status: u16,
    body: Value,
    expected_status: u16,
    expected_code: &'static str,
    expected_reason: &'static str,
    /// The values the model's own enum for this error declares, which the reported reason
    /// must be one of.
    modelled_values: &'static [&'static str],
}

/// RULE, part 2: each modelled error of this service reports a reason its own enum
/// declares — the single value where the enum names one, and the fault class the
/// condition belongs to where it names several.
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
        expected_reason: sole_value(
            "ResourceNotFoundExceptionReason",
            RESOURCE_NOT_FOUND_EXCEPTION_REASON_VALUES,
        ),
        modelled_values: RESOURCE_NOT_FOUND_EXCEPTION_REASON_VALUES,
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
        expected_reason: sole_value("ConflictExceptionReason", CONFLICT_EXCEPTION_REASON_VALUES),
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
        expected_reason: sole_value(
            "ServiceQuotaExceededExceptionReason",
            SERVICE_QUOTA_EXCEEDED_EXCEPTION_REASON_VALUES,
        ),
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
        expected_reason: "INVALID_PARAMETER_VALUE",
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
            Some(*expected_reason),
            "case {case:?}: expected Reason {expected_reason:?}, got {:?}",
            body.get("Reason")
        );
        let stated = reason(body).unwrap_or_default();
        assert!(
            modelled_values.contains(&stated),
            "case {case:?}: Reason {stated:?} is outside the modelled value set \
             {modelled_values:?}"
        );
    }
}

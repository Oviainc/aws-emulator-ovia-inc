// SPDX-License-Identifier: CC0-1.0

//! The spelling a read reports a saved filter's criterion conditions in.
//!
//! `Condition` models every comparison TWICE: the current member and the
//! `@deprecated` member that preceded it — `Equals`/`Eq`, `NotEquals`/`Neq`,
//! `GreaterThan`/`Gt`, `GreaterThanOrEqual`/`Gte`, `LessThan`/`Lt`,
//! `LessThanOrEqual`/`Lte` (`smithy_query(shape="Condition")`). The two spellings of a
//! pair are the same comparison, so the rule under test is:
//!
//! > a read of a saved filter reports every comparison the filter carries under BOTH
//! > spellings of its pair, whichever one the create or the update wrote it in — except
//! > a threshold the deprecated member's `Integer` type cannot hold, which only the
//! > `Long`-typed current member reports, and `Matches`/`NotMatches`, which have no
//! > second spelling.
//!
//! The rows span both spellings of all six pairs, the two members that have none, a
//! threshold past the deprecated member's type, conditions carrying several members at
//! once and criteria carrying several fields at once — on the create path and on the
//! update path.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};

/// The `Condition` member pairs, as `(current, deprecated)`, from
/// `smithy_query(shape="Condition")`: every member of the shape that carries a
/// comparison, apart from `Matches`/`NotMatches`, which have no second spelling.
pub(crate) const CONDITION_SPELLINGS: [(&str, &str); 6] = [
    ("equals", "eq"),
    ("notEquals", "neq"),
    ("greaterThan", "gt"),
    ("greaterThanOrEqual", "gte"),
    ("lessThan", "lt"),
    ("lessThanOrEqual", "lte"),
];

/// One submitted `Condition` as a read reports it: a comparison written under one
/// spelling of a pair is reported under both.
///
/// A filter carrying BOTH members of a pair carries two comparisons, which stay as
/// they were written. A threshold the deprecated member cannot hold is the one other
/// exception: that member targets `Integer` where the current one targets `Long`
/// (`smithy_query(shape="Condition")`), so a value outside the 32-bit range is
/// reported under the current spelling alone.
pub(crate) fn reported_condition(condition: &Value) -> Value {
    let mut reported = condition.clone();
    let Some(members) = reported.as_object_mut() else {
        return reported;
    };
    for (current, deprecated) in CONDITION_SPELLINGS {
        match (
            members.get(current).cloned(),
            members.get(deprecated).cloned(),
        ) {
            (Some(_), Some(_)) | (None, None) => continue,
            (Some(value), None) => {
                if fits_the_deprecated_member(&value) {
                    members.insert(deprecated.to_string(), value);
                }
            }
            (None, Some(value)) => {
                members.insert(current.to_string(), value);
            }
        }
    }
    reported
}

/// Can the deprecated member of a pair hold `value`? Its `Integer` target cannot
/// carry a threshold outside the 32-bit range; a list is not a number and always
/// fits.
pub(crate) fn fits_the_deprecated_member(value: &Value) -> bool {
    value
        .as_i64()
        .is_none_or(|number| i32::try_from(number).is_ok())
}

/// One submitted `findingCriteria` as a read reports it. See [`reported_condition`].
pub(crate) fn reported_criteria(criteria: &Value) -> Value {
    let mut reported = criteria.clone();
    if let Some(criterion) = reported
        .get_mut("criterion")
        .and_then(|criterion| criterion.as_object_mut())
    {
        for condition in criterion.values_mut() {
            *condition = reported_condition(condition);
        }
    }
    reported
}

async fn create_filter(router: &Router, detector_id: &str, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(body.clone()),
    )
    .await
}

async fn update_filter(
    router: &Router,
    detector_id: &str,
    name: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter/{name}"),
        Some(body.clone()),
    )
    .await
}

async fn get_filter(router: &Router, detector_id: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/filter/{name}"),
        None,
    )
    .await
}

/// A criterion the service accepts, as `(why, criterion)`.
///
/// Every field named here is a documented finding field and every value is one that
/// field can hold, so each row is accepted and SAVED: the ordering members go on
/// numeric fields (`severity`, `confidence`, `createdAt`, `service.count`) and the
/// equality members on those and on text fields (`type`, `region`, `accountId`).
fn criterion_rows() -> Vec<(&'static str, Value)> {
    vec![
        // Each pair under its CURRENT spelling.
        (
            "equals on a text field",
            json!({"type": {"equals": ["Recon:EC2/PortProbeUnprotectedPort"]}}),
        ),
        (
            "notEquals with two values",
            json!({"accountId": {"notEquals": ["111122223333", "444455556666"]}}),
        ),
        ("greaterThan", json!({"severity": {"greaterThan": 2}})),
        (
            "greaterThanOrEqual",
            json!({"severity": {"greaterThanOrEqual": 4}}),
        ),
        ("lessThan", json!({"confidence": {"lessThan": 9}})),
        (
            "lessThanOrEqual",
            json!({"service.count": {"lessThanOrEqual": 7}}),
        ),
        // Each pair under its DEPRECATED spelling.
        ("eq on a numeric field", json!({"severity": {"eq": ["8"]}})),
        ("eq on a text field", json!({"region": {"eq": ["us-east-1"]}})),
        (
            "neq with two values",
            json!({"accountId": {"neq": ["111122223333", "444455556666"]}}),
        ),
        ("gt", json!({"severity": {"gt": 2}})),
        ("gte", json!({"severity": {"gte": 4}})),
        ("lt", json!({"confidence": {"lt": 9}})),
        ("lte", json!({"service.count": {"lte": 7}})),
        // The two members with no second spelling: "available only for create-filter
        // and update-filter" (`smithy_query(query="doc",shape="Condition",
        // member="Matches")`).
        (
            "matches, which has no deprecated spelling",
            json!({"type": {"matches": ["Recon*", "UnauthorizedAccess*"]}}),
        ),
        (
            "notMatches, which has no deprecated spelling",
            json!({"region": {"notMatches": ["us-*"]}}),
        ),
        // A threshold past the deprecated member's `Integer` type: an instant in
        // "Unix Epoch millisecond format"
        // (`smithy_query(query="doc",shape="CreateFilterRequest",
        // member="FindingCriteria")`).
        (
            "a greaterThanOrEqual threshold past the 32-bit range",
            json!({"createdAt": {"greaterThanOrEqual": 1486685375000i64}}),
        ),
        (
            "a lessThan threshold one past the 32-bit maximum",
            json!({"createdAt": {"lessThan": 2147483648i64}}),
        ),
        (
            "a gte threshold at the 32-bit maximum",
            json!({"createdAt": {"gte": 2147483647i64}}),
        ),
        // Several members on one field, and several fields at once, in both spellings.
        (
            "two thresholds bounding one field",
            json!({"severity": {"greaterThan": 2, "lessThanOrEqual": 8}}),
        ),
        (
            "two deprecated thresholds bounding one field",
            json!({"severity": {"gt": 2, "lte": 8}}),
        ),
        (
            "an equality and a threshold mixing the two spellings",
            json!({"severity": {"eq": ["8"], "greaterThanOrEqual": 1}}),
        ),
        (
            "both spellings of one pair, carrying a comparison each",
            json!({"severity": {"eq": ["8"], "equals": ["7"]}}),
        ),
        (
            "both spellings of one ordering pair, carrying a threshold each",
            json!({"confidence": {"gte": 2, "greaterThanOrEqual": 3}}),
        ),
        (
            "three fields at once, mixing the two spellings",
            json!({
                "severity": {"gte": 7},
                "type": {"eq": ["Recon:EC2/PortProbeUnprotectedPort"]},
                "region": {"equals": ["us-east-1"]},
            }),
        ),
    ]
}

/// Assert the rule on one read: the criteria are reported as submitted, expanded to
/// both spellings of every pair.
fn assert_reported_in_both_spellings(case: &str, sent: &Value, read: &Value) {
    let expected = reported_criteria(sent);
    assert_eq!(
        read["findingCriteria"], expected,
        "case {case}: expected the criteria reported as {expected}, got {read:?}"
    );

    // The same rule again, stated member by member rather than by comparing whole
    // documents: a comparison written under one spelling of a pair is reported under
    // both, a pair the filter carries twice keeps both comparisons, and a pair it
    // does not carry is reported under neither.
    let criterion = sent["criterion"].as_object().expect("a criterion to send");
    for (field, condition) in criterion {
        let got = &read["findingCriteria"]["criterion"][field];
        for (current, deprecated) in CONDITION_SPELLINGS {
            let written_current = condition.get(current).cloned();
            let written_deprecated = condition.get(deprecated).cloned();
            match (written_current, written_deprecated) {
                (None, None) => assert!(
                    got[current].is_null() && got[deprecated].is_null(),
                    "case {case}: {field} carries no {current}/{deprecated}, so the read must report neither, got {got:?}"
                ),
                (Some(written_current), Some(written_deprecated)) => {
                    assert_eq!(
                        got[current], written_current,
                        "case {case}: expected {field}.{current} {written_current}, got {got:?}"
                    );
                    assert_eq!(
                        got[deprecated], written_deprecated,
                        "case {case}: expected {field}.{deprecated} {written_deprecated}, got {got:?}"
                    );
                }
                (Some(written), None) | (None, Some(written)) => {
                    assert_eq!(
                        got[current], written,
                        "case {case}: expected {field}.{current} {written}, got {got:?}"
                    );
                    if fits_the_deprecated_member(&written) {
                        assert_eq!(
                            got[deprecated], written,
                            "case {case}: expected {field}.{deprecated} {written} beside {current}, got {got:?}"
                        );
                    } else {
                        assert!(
                            got[deprecated].is_null(),
                            "case {case}: {written} is past the 32-bit range the deprecated {deprecated} holds, so only {current} reports it, got {got:?}"
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn a_created_filter_reports_each_condition_in_both_spellings() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for (index, (case, criterion)) in criterion_rows().into_iter().enumerate() {
        let name = format!("created-{index}");
        let criteria = json!({"criterion": criterion});
        let (status, created) = create_filter(
            &router,
            &detector_id,
            &json!({"name": name, "findingCriteria": criteria}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from the create, got {status} {created:?}"
        );

        let (status, read) = get_filter(&router, &detector_id, &name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the saved filter to read back, got {status} {read:?}"
        );
        assert_reported_in_both_spellings(case, &criteria, &read);
    }
}

#[tokio::test]
async fn an_updated_filter_reports_each_condition_in_both_spellings() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let name = "updated.filter";
    let (status, created) = create_filter(
        &router,
        &detector_id,
        &json!({
            "name": name,
            "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 4}}},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the baseline create must be served, got {status} {created:?}"
    );

    for (case, criterion) in criterion_rows() {
        let criteria = json!({"criterion": criterion});
        let (status, updated) = update_filter(
            &router,
            &detector_id,
            name,
            &json!({"findingCriteria": criteria}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from the update, got {status} {updated:?}"
        );

        let (status, read) = get_filter(&router, &detector_id, name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the updated filter to read back, got {status} {read:?}"
        );
        assert_reported_in_both_spellings(case, &criteria, &read);
    }
}

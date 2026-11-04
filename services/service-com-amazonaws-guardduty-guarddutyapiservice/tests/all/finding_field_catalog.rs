// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FIELD NAMES a `FindingCriteria.Criterion` key and a
//! `SortCriteria.AttributeName` may carry, on every operation that takes one:
//! `CreateFilter`, `UpdateFilter`, `ListFindings`, `GetFindingsStatistics` and
//! `GetFindings`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses.
//!
//! The rules under test, each asserted over a SET of field names rather than one
//! recorded call:
//!
//! 1. A criterion key or a sort attribute naming a FIELD OF A FINDING is accepted —
//!    every field the model lists as available for filtering findings, whether or not
//!    a finding this emulator stores carries a value for it.
//! 2. A field no stored finding carries a value for selects NO finding and excludes
//!    none: an absent value equals nothing, so an equality or an ordering on it selects
//!    nothing while a negation of it selects every finding.
//! 3. A field a stored finding DOES carry is selected by the value the finding
//!    reports.
//! 4. A name that is no field of a finding is refused with HTTP 400 under this
//!    service's client-rejection class, by all five operations alike, and the refusal
//!    writes nothing.
//!
//! The field names the rows span come from the model's own list of the fields
//! available for filtering
//! (`smithy_query(query="doc",shape="CreateFilterRequest",member="FindingCriteria")`
//! and `smithy_query(query="doc",shape="ListFindingsRequest",member="FindingCriteria")`)
//! and from the `Finding`, `Resource` and `Service` member names
//! (`smithy_query(shape="Finding,Resource,Service")`), not from any recorded call.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, error_code, new_router, Router};
// The criterion builder and the create call are the ones the `negative_class_rejections`
// suite asserts the refusal side of the same rule with, so both suites build a
// `FindingCriteria` the same way.
use crate::negative_class_rejections::{create_filter, criteria};

/// The client-rejection class every refusal in this slice carries.
const INVALID_INPUT: &str = "InvalidInputException";

/// Finding types whose samples give the store something to select over: two resource
/// types and two severity bands.
const SAMPLE_TYPES: [&str; 3] = [
    "Recon:EC2/PortProbeUnprotectedPort",
    "Backdoor:EC2/C&CActivity.B",
    "Discovery:S3/MaliciousIPCaller",
];

/// Field names the model lists as available for filtering findings, which NO finding
/// this emulator stores carries a value for.
///
/// The rows differ along the FORM the name takes: a field of a resource detail
/// document, a field reached through a list of structures, a deeply nested one, a field
/// of each of the service document's own sub-documents, and a top-level scalar the
/// finding projection leaves absent. Each is quoted from the model's filterable-field
/// list, and none is a field the issue's recorded request named except where noted.
const UNPOPULATED_FIELDS: [(&str, &str); 12] = [
    (
        "service.action.actionType",
        "a field of the action document",
    ),
    (
        "service.action.awsApiCallAction.api",
        "a field of one action kind",
    ),
    (
        "service.action.dnsRequestAction.domain",
        "a field of another action kind",
    ),
    (
        "service.additionalInfo.threatListName",
        "a field of the additional-information document",
    ),
    (
        "service.detection.sequence.actors.id",
        "a field reached through the detection sequence",
    ),
    (
        "resource.instanceDetails.instanceId",
        "a field of a resource detail document",
    ),
    (
        "resource.instanceDetails.tags.key",
        "a field reached through a list of structures",
    ),
    (
        "resource.accessKeyDetails.userIdentity.sessionContext.sessionIssuer.arn",
        "a deeply nested field",
    ),
    (
        "resource.ebsVolumeDetails.scannedVolumeDetails.volumeArn",
        "a field of a scanned-volume document",
    ),
    (
        "resource.bedrockGuardrailDetails.guardrailArn",
        "a field of a recently added detail document",
    ),
    (
        "resource.s3BucketDetails.s3ObjectDetails.eTag",
        "a field whose name carries an interior capital",
    ),
    (
        "associatedAttackSequenceArn",
        "a top-level scalar no stored finding carries",
    ),
];

/// Names that are NO field of a finding, with the way each fails to name one. The rows
/// differ along the failure: an unknown member of a sub-document, a sub-document named
/// with no field under it, a path extending a scalar that has nothing beneath it, a
/// casing variant at each level, an empty segment, and an unknown top-level name.
const NON_FIELDS: [(&str, &str); 10] = [
    ("service.actionType", "no member of the service document"),
    (
        "resource.instanceDetails",
        "a detail document rather than a field in it",
    ),
    ("service", "the service document itself"),
    (
        "service.archived.value",
        "a path extending a scalar that has nothing beneath it",
    ),
    ("Severity", "a top-level field in the wrong case"),
    (
        "resource.InstanceDetails.instanceId",
        "a sub-document member in the wrong case",
    ),
    (
        "resource.instanceDetails.InstanceId",
        "a detail-document field in the wrong case",
    ),
    ("resource.", "a trailing empty segment"),
    ("", "the empty name"),
    ("notAFindingField", "an unknown top-level name"),
];

async fn get_filter(router: &Router, detector_id: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/filter/{name}"),
        None,
    )
    .await
}

async fn update_filter(router: &Router, detector_id: &str, name: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter/{name}"),
        Some(body),
    )
    .await
}

async fn list_findings(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(body),
    )
    .await
}

async fn get_findings(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(body),
    )
    .await
}

async fn findings_statistics(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/statistics"),
        Some(body),
    )
    .await
}

/// Generate the sample findings the selection rows run over and return their ids,
/// newest-first as a bare `ListFindings` reports them.
async fn detector_with_findings(router: &Router) -> (String, Vec<String>) {
    let detector_id = enabled_detector(router).await;
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": SAMPLE_TYPES})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings: expected 200, got {status} {body:?}"
    );
    let (status, body) = list_findings(router, &detector_id, json!({})).await;
    assert_eq!(
        status, 200,
        "list-findings with no criteria: expected 200, got {status} {body:?}"
    );
    let ids = finding_ids(&body);
    assert_eq!(
        ids.len(),
        SAMPLE_TYPES.len(),
        "expected one finding per sample type, got {body:?}"
    );
    (detector_id, ids)
}

/// The `findingIds` a `ListFindings` answer carries.
fn finding_ids(body: &Value) -> Vec<String> {
    body["findingIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: every documented finding field is accepted, by every operation that
// takes a criterion key or a sort attribute.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_documented_finding_field_is_accepted_wherever_a_field_name_is_taken() {
    let router = new_router();
    let (detector_id, ids) = detector_with_findings(&router).await;

    for (index, (field, form)) in UNPOPULATED_FIELDS.into_iter().enumerate() {
        let case = format!("{field:?} ({form})");
        let condition = json!({"equals": ["a-value-no-finding-carries"]});

        // A filter keyed on the field is SAVED, and a read reports back the criterion
        // it was created with rather than a criterion the service rewrote.
        let name = format!("saved-{index}");
        let (status, body) = create_filter(
            &router,
            &detector_id,
            json!({
                "name": name,
                "action": "ARCHIVE",
                "findingCriteria": criteria(field, condition.clone()),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: create-filter expected 200, got {status} {body:?}"
        );
        let (status, saved) = get_filter(&router, &detector_id, &name).await;
        assert_eq!(
            status, 200,
            "case {case}: get-filter expected 200, got {status} {saved:?}"
        );
        assert_eq!(
            saved["findingCriteria"]["criterion"][field],
            crate::filter_condition_spellings::reported_condition(&condition),
            "case {case}: expected the saved criterion to round-trip, got {saved:?}"
        );

        // An update may key a saved filter on it too, so a create and an update agree
        // on what a filter may carry.
        let (status, body) = update_filter(
            &router,
            &detector_id,
            &name,
            json!({"findingCriteria": criteria(field, json!({"notEquals": ["x"]}))}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: update-filter expected 200, got {status} {body:?}"
        );

        // And the three findings reads accept it: as a criterion key, and as a sort
        // attribute.
        let (status, body) = list_findings(
            &router,
            &detector_id,
            json!({"findingCriteria": criteria(field, condition.clone())}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: list-findings expected 200, got {status} {body:?}"
        );
        let (status, body) = findings_statistics(
            &router,
            &detector_id,
            json!({
                "findingStatisticTypes": ["COUNT_BY_SEVERITY"],
                "findingCriteria": criteria(field, condition.clone()),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: get-findings-statistics expected 200, got {status} {body:?}"
        );
        let (status, body) = list_findings(
            &router,
            &detector_id,
            json!({"sortCriteria": {"attributeName": field}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: list-findings sorted on the field expected 200, got {status} {body:?}"
        );
        assert_eq!(
            finding_ids(&body).len(),
            ids.len(),
            "case {case}: sorting must keep the same set, got {body:?}"
        );
        let (status, body) = get_findings(
            &router,
            &detector_id,
            json!({"findingIds": ids.clone(), "sortCriteria": {"attributeName": field}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: get-findings sorted on the field expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body["findings"].as_array().map_or(0, Vec::len),
            ids.len(),
            "case {case}: sorting must keep the same set, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a field with no value on any stored finding selects nothing and
// excludes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_field_no_stored_finding_carries_selects_nothing_and_excludes_nothing() {
    let router = new_router();
    let (detector_id, ids) = detector_with_findings(&router).await;
    let all = ids.len();

    // (condition on the field, how many of the stored findings it selects)
    //
    // The members the model offers only to the filter APIs are not among the rows: a
    // findings read refuses them, and the same rule for a PATTERN on an unpopulated
    // field is asserted on the filter surface by the `filter_only_conditions` suite.
    let conditions: [(Value, usize); 5] = [
        (json!({"equals": ["anything"]}), 0),
        (json!({"eq": ["anything"]}), 0),
        (json!({"greaterThan": 0}), 0),
        (json!({"lessThanOrEqual": 99}), 0),
        (json!({"notEquals": ["anything"]}), all),
    ];

    for (field, form) in UNPOPULATED_FIELDS {
        for (condition, expected) in &conditions {
            let case = format!("{field:?} ({form}) under {condition}");
            let (status, body) = list_findings(
                &router,
                &detector_id,
                json!({"findingCriteria": criteria(field, condition.clone())}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: list-findings expected 200, got {status} {body:?}"
            );
            let selected = finding_ids(&body).len();
            assert_eq!(
                selected, *expected,
                "case {case}: expected {expected} finding(s), got {selected} in {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a field a stored finding DOES carry selects by the stored value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_field_a_stored_finding_carries_selects_by_the_value_it_reports() {
    let router = new_router();
    let (detector_id, ids) = detector_with_findings(&router).await;
    let all = ids.len();

    // The values are read back from the findings themselves, so the expectation is the
    // service's own answer rather than a literal copied into the test.
    let (status, body) = get_findings(&router, &detector_id, json!({"findingIds": ids.clone()})).await;
    assert_eq!(
        status, 200,
        "get-findings: expected 200, got {status} {body:?}"
    );
    let reported = body["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .first()
        .cloned()
        .unwrap_or(Value::Null);
    let resource_role = reported["service"]["resourceRole"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let service_name = reported["service"]["serviceName"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let event_first_seen = reported["service"]["eventFirstSeen"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !resource_role.is_empty() && !service_name.is_empty() && !event_first_seen.is_empty(),
        "the findings must report the service fields the rows select on, got {reported:?}"
    );

    // (field, condition, how many of the stored findings it selects)
    //
    // Each sample finding was created at its own instant, so an equality on ONE
    // finding's activity timestamp selects that finding alone, while the fields every
    // finding shares the same value for select all of them.
    let rows: [(&str, Value, usize); 10] = [
        ("service.resourceRole", json!({"equals": [resource_role.clone()]}), all),
        ("service.resourceRole", json!({"notEquals": [resource_role]}), 0),
        ("service.serviceName", json!({"equals": [service_name.clone()]}), all),
        ("service.serviceName", json!({"notEquals": ["not-guardduty"]}), all),
        ("service.serviceName", json!({"equals": ["not-guardduty"]}), 0),
        ("service.eventFirstSeen", json!({"equals": [event_first_seen]}), 1),
        ("service.eventFirstSeen", json!({"greaterThan": 0}), all),
        ("service.count", json!({"greaterThanOrEqual": 1}), all),
        ("service.count", json!({"greaterThan": 1}), 0),
        ("service.additionalInfo.sample", json!({"equals": ["true"]}), all),
    ];

    for (field, condition, expected) in rows {
        let case = format!("{field:?} under {condition}");
        let (status, body) = list_findings(
            &router,
            &detector_id,
            json!({"findingCriteria": criteria(field, condition.clone())}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: list-findings expected 200, got {status} {body:?}"
        );
        let selected = finding_ids(&body).len();
        assert_eq!(
            selected, expected,
            "case {case}: expected {expected} finding(s), got {selected} in {body:?}"
        );
    }

    // A field the record holds no value for YET behaves as rule 2 says: no feedback has
    // been submitted, so the equality selects nothing and its negation selects all.
    for (condition, expected) in [
        (json!({"equals": ["USEFUL"]}), 0),
        (json!({"notEquals": ["USEFUL"]}), all),
    ] {
        let case = format!("service.userFeedback under {condition}");
        let (status, body) = list_findings(
            &router,
            &detector_id,
            json!({"findingCriteria": criteria("service.userFeedback", condition.clone())}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: list-findings expected 200, got {status} {body:?}"
        );
        let selected = finding_ids(&body).len();
        assert_eq!(
            selected, expected,
            "case {case}: expected {expected} finding(s), got {selected} in {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a name that is no field of a finding is refused everywhere, and the
// refusal writes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_that_is_no_finding_field_is_refused_by_every_operation() {
    let router = new_router();
    let (detector_id, ids) = detector_with_findings(&router).await;

    let baseline = "baseline.filter_1";
    let (status, body) = create_filter(
        &router,
        &detector_id,
        json!({
            "name": baseline,
            "findingCriteria": criteria("severity", json!({"greaterThanOrEqual": 1})),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the baseline filter: expected 200, got {status} {body:?}"
    );

    for (index, (name, why)) in NON_FIELDS.into_iter().enumerate() {
        let condition = json!({"equals": ["x"]});
        let filter_name = format!("refused-{index}");
        let attempts = [
            (
                "create-filter",
                create_filter(
                    &router,
                    &detector_id,
                    json!({
                        "name": filter_name,
                        "findingCriteria": criteria(name, condition.clone()),
                    }),
                )
                .await,
            ),
            (
                "update-filter",
                update_filter(
                    &router,
                    &detector_id,
                    baseline,
                    json!({"findingCriteria": criteria(name, condition.clone())}),
                )
                .await,
            ),
            (
                "list-findings criterion",
                list_findings(
                    &router,
                    &detector_id,
                    json!({"findingCriteria": criteria(name, condition.clone())}),
                )
                .await,
            ),
            (
                "get-findings-statistics criterion",
                findings_statistics(
                    &router,
                    &detector_id,
                    json!({
                        "findingStatisticTypes": ["COUNT_BY_SEVERITY"],
                        "findingCriteria": criteria(name, condition.clone()),
                    }),
                )
                .await,
            ),
            (
                "list-findings sort attribute",
                list_findings(
                    &router,
                    &detector_id,
                    json!({"sortCriteria": {"attributeName": name}}),
                )
                .await,
            ),
            (
                "get-findings sort attribute",
                get_findings(
                    &router,
                    &detector_id,
                    json!({"findingIds": ids.clone(), "sortCriteria": {"attributeName": name}}),
                )
                .await,
            ),
        ];

        for (operation, (status, body)) in attempts {
            let case = format!("{name:?} ({why}) on {operation}");
            // The empty sort attribute is the one name that is NOT a refusal: an
            // absent-or-empty `attributeName` asked for no ordering at all, which the
            // default ordering answers.
            if name.is_empty() && operation.ends_with("sort attribute") {
                assert_eq!(
                    status, 200,
                    "case {case}: an empty sort attribute asks for the default order, got {status} {body:?}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_INPUT,
                "case {case}: expected the client-rejection class, got {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("Criteria") || message.contains("criteri"),
                "case {case}: expected a message naming the member, got {message:?}"
            );
        }

        // Neither the refused create nor the refused update left anything behind.
        let (status, body) = get_filter(&router, &detector_id, &filter_name).await;
        assert_eq!(
            status, 400,
            "case {name:?} ({why}): a refused create must persist no filter, got {status} {body:?}"
        );
        let (status, saved) = get_filter(&router, &detector_id, baseline).await;
        assert_eq!(
            status, 200,
            "case {name:?} ({why}): the baseline filter must survive, got {status} {saved:?}"
        );
        assert_eq!(
            saved["findingCriteria"]["criterion"]["severity"]["greaterThanOrEqual"],
            json!(1),
            "case {name:?} ({why}): a refused update must change nothing, got {saved:?}"
        );
    }
}

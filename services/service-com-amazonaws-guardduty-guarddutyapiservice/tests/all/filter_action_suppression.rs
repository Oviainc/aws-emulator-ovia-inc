// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a saved filter's `Action`: the rule that a filter whose action
//! archives is APPLIED to the findings its criteria select, rather than merely stored
//! and echoed.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, reusing the
//! harness in [`super::detector_lifecycle`], so routing, restJson1 (de)serialization and
//! the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A saved filter whose action archives puts exactly the findings its criteria
//!    select into the archived state, and leaves every other finding where it was. For
//!    criteria built from the `Condition` members a findings read also offers, the set
//!    it selects is the set a `ListFindings` carrying the same criteria reports, so the
//!    suppression and the findings read cannot select different sets; the members the
//!    model offers only to the filter APIs have no findings read to compare against, so
//!    those rows state the selection on their own.
//! 2. The action decides whether a matching finding moves: the archiving member moves
//!    it, the other member and a request naming no action leave it alone.
//! 3. The rule holds whichever order the filter and the findings arrive in — a finding
//!    generated while the filter is saved is archived as it is generated, and another
//!    occurrence of it stays archived.
//! 4. An update applies the same rule to the filter it leaves behind, whether it is the
//!    action or the criteria that the update changes.
//! 5. Several saved filters compose: a finding one of them selects is archived. Saving
//!    one filter applies THAT filter, so the explicit archive write still decides the
//!    state of a finding it does not select — a caller can unarchive a suppressed
//!    finding and it stays unarchived until the filter that selects it is saved over it
//!    again.
//!
//! The value set the rows span — `FilterAction` (`NOOP | ARCHIVE`) plus "names no
//! action", whose documented default is `NOOP` — comes from
//! `smithy_query(shape="FilterAction")` and
//! `smithy_query(query="doc",shape="CreateFilterRequest",member="Action")`, and the
//! `Condition` members the criteria rows use from `smithy_query(shape="Condition")`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::detector_lifecycle::{all_finding_ids, call, enabled_detector, new_router, Router};

/// The finding types each test's detector is populated with, with the severity this
/// service's sample catalogue publishes for each. Source: the GuardDuty finding-types
/// reference, through `CreateSampleFindings`' own catalogue — the rows below select on
/// `type` and `severity`, so the set spans four severities and five resource families
/// and the selections are proper subsets rather than everything or nothing.
const SAMPLES: [(&str, f64); 5] = [
    ("Impact:EC2/SuspiciousDomainRequest.Reputation", 2.0),
    ("Recon:EC2/Portscan", 5.0),
    ("Policy:Kubernetes/ExposedDashboard", 5.0),
    ("Discovery:S3/MaliciousIPCaller", 8.0),
    ("AttackSequence:IAM/CompromisedCredentials", 9.0),
];

/// The `FilterAction` member that archives the findings a filter selects.
const ACTION_ARCHIVE: &str = "ARCHIVE";
/// The other `FilterAction` member, which is also the documented default.
const ACTION_NOOP: &str = "NOOP";

/// One finding type, by index into [`SAMPLES`].
fn sample_type(index: usize) -> &'static str {
    SAMPLES[index].0
}

/// A `FindingCriteria` carrying one condition on one field.
fn criteria(field: &str, condition: Value) -> Value {
    json!({"criterion": {field: condition}})
}

/// Generate one sample finding per entry of `types`.
async fn create_samples(router: &Router, detector_id: &str, types: &[&str]) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": types})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings {types:?}: expected 200, got {status} {body:?}"
    );
}

/// An enabled detector holding one finding per entry of [`SAMPLES`].
async fn detector_with_findings(router: &Router) -> String {
    let detector_id = enabled_detector(router).await;
    let types: Vec<&str> = SAMPLES.iter().map(|(name, _)| *name).collect();
    create_samples(router, &detector_id, &types).await;
    let ids = all_finding_ids(router, &detector_id, 50).await;
    assert_eq!(
        ids.len(),
        SAMPLES.len(),
        "expected one finding per sample type, got {ids:?}"
    );
    // The severity rows below select on a NUMBER, so the severities the findings
    // report are what makes those rows' expectations well founded.
    let reported: BTreeMap<String, f64> = findings(router, &detector_id)
        .await
        .into_iter()
        .map(|finding| {
            (
                finding["type"].as_str().unwrap_or_default().to_string(),
                finding["severity"].as_f64().unwrap_or_default(),
            )
        })
        .collect();
    for (finding_type, severity) in SAMPLES {
        assert_eq!(
            reported.get(finding_type).copied(),
            Some(severity),
            "sample {finding_type:?}: expected severity {severity}, got {:?}",
            reported.get(finding_type)
        );
    }
    detector_id
}

/// Every finding the detector holds, read back through `GetFindings`.
async fn findings(router: &Router, detector_id: &str) -> Vec<Value> {
    let ids = all_finding_ids(router, detector_id, 50).await;
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(json!({"findingIds": ids})),
    )
    .await;
    assert_eq!(
        status, 200,
        "get-findings: expected 200, got {status} {body:?}"
    );
    body["findings"].as_array().cloned().unwrap_or_default()
}

/// Each finding's `service.archived`, keyed by its finding TYPE (one finding per type
/// here, and the type is what the rows select on).
async fn archived_by_type(router: &Router, detector_id: &str) -> BTreeMap<String, bool> {
    findings(router, detector_id)
        .await
        .into_iter()
        .map(|finding| {
            let finding_type = finding["type"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a finding type, got {finding:?}"))
                .to_string();
            let archived = finding["service"]["archived"].as_bool().unwrap_or_else(|| {
                panic!("finding {finding_type}: expected a boolean service.archived, got {finding:?}")
            });
            (finding_type, archived)
        })
        .collect()
}

/// The finding types whose findings are archived.
async fn archived_types(router: &Router, detector_id: &str) -> BTreeSet<String> {
    archived_by_type(router, detector_id)
        .await
        .into_iter()
        .filter(|(_, archived)| *archived)
        .map(|(finding_type, _)| finding_type)
        .collect()
}

/// The finding ids a `ListFindings` carrying `body` reports.
async fn listed_ids(router: &Router, detector_id: &str, body: Value) -> BTreeSet<String> {
    let (status, listed) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "list-findings: expected 200, got {status} {listed:?}"
    );
    listed["findingIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// The finding TYPES a `ListFindings` carrying `criteria` selects — the reference set a
/// filter carrying the same criteria must suppress.
async fn selected_types(router: &Router, detector_id: &str, criteria: &Value) -> BTreeSet<String> {
    let type_by_id: BTreeMap<String, String> = findings(router, detector_id)
        .await
        .into_iter()
        .map(|finding| {
            (
                finding["id"].as_str().unwrap_or_default().to_string(),
                finding["type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    listed_ids(
        router,
        detector_id,
        json!({"findingCriteria": criteria.clone()}),
    )
    .await
    .into_iter()
    .map(|id| {
        type_by_id
            .get(&id)
            .cloned()
            .unwrap_or_else(|| panic!("list-findings reported an id no finding carries: {id}"))
    })
    .collect()
}

/// Save a filter, failing the test unless the create succeeded.
async fn create_filter_ok(router: &Router, detector_id: &str, body: Value) {
    let (status, answer) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-filter {body}: expected 200, got {status} {answer:?}"
    );
}

/// Update a filter, failing the test unless the update succeeded.
async fn update_filter_ok(router: &Router, detector_id: &str, name: &str, body: Value) {
    let (status, answer) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter/{name}"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "update-filter {name} with {body}: expected 200, got {status} {answer:?}"
    );
}

/// Archive or unarchive the named findings explicitly.
async fn set_archived_ok(router: &Router, detector_id: &str, ids: &[String], archived: bool) {
    let path = if archived { "archive" } else { "unarchive" };
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/{path}"),
        Some(json!({"findingIds": ids})),
    )
    .await;
    assert_eq!(
        status, 200,
        "set archived={archived} over {ids:?}: expected 200, got {status} {body:?}"
    );
}

/// The ids of the findings whose type is in `types`.
async fn ids_of_types(router: &Router, detector_id: &str, types: &[&str]) -> Vec<String> {
    findings(router, detector_id)
        .await
        .into_iter()
        .filter(|finding| types.contains(&finding["type"].as_str().unwrap_or_default()))
        .filter_map(|finding| finding["id"].as_str().map(str::to_string))
        .collect()
}

/// Does `criteria` apply a `Condition` member the model offers only to the filter APIs?
///
/// `Matches` is "available only for create-filter and update-filter APIs"
/// (`smithy_query(query="doc",shape="Condition",member="Matches")`) and `NotMatches`
/// says the same of itself, so a findings read refuses criteria carrying either and
/// cannot serve as the reference set for such a row.
fn uses_filter_only_condition(criteria: &Value) -> bool {
    criteria["criterion"]
        .as_object()
        .into_iter()
        .flatten()
        .any(|(_, condition)| {
            ["matches", "notMatches"]
                .iter()
                .any(|member| condition.get(member).is_some())
        })
}

// ---------------------------------------------------------------------------
// Rule 1: an archiving filter archives exactly the findings its criteria select.
// ---------------------------------------------------------------------------

/// Each row is a different `FindingCriteria`, so the rows differ along the field, the
/// `Condition` member, the value and the SIZE of the selection: everything, one
/// finding, two, the complement of one, the two ends of a numeric range, a wildcard, an
/// intersection of two criteria, and a selection that is empty.
///
/// The expectation is stated twice over wherever it can be: the findings the row names,
/// and the set the service's own `ListFindings` reports for the same criteria — so a
/// filter can neither suppress more than it selects nor less. A row built from a
/// `Condition` member the model offers only to the filter APIs has no findings read to
/// compare against ([`uses_filter_only_condition`]), so it stands on the set it names.
#[tokio::test]
async fn an_archiving_filter_archives_exactly_the_findings_its_criteria_select() {
    // (label, criteria, the indices of SAMPLES the criteria select)
    let rows: [(&str, Value, &[usize]); 9] = [
        (
            "every finding (no criterion at all)",
            json!({"criterion": {}}),
            &[0, 1, 2, 3, 4],
        ),
        (
            "one type",
            criteria("type", json!({"equals": [sample_type(1)]})),
            &[1],
        ),
        (
            "two types",
            criteria(
                "type",
                json!({"equals": [sample_type(0), sample_type(3)]}),
            ),
            &[0, 3],
        ),
        (
            "the complement of one type",
            criteria("type", json!({"notEquals": [sample_type(1)]})),
            &[0, 2, 3, 4],
        ),
        (
            "the high end of the severity range",
            criteria("severity", json!({"greaterThanOrEqual": 8})),
            &[3, 4],
        ),
        (
            "the low end of the severity range",
            criteria("severity", json!({"lessThan": 4})),
            &[0],
        ),
        (
            "a wildcard on the type",
            criteria("type", json!({"matches": ["Recon:EC2/*"]})),
            &[1],
        ),
        (
            "two criteria, intersected",
            json!({"criterion": {
                "severity": {"greaterThanOrEqual": 5},
                "type": {"matches": ["*:S3/*"]},
            }}),
            &[3],
        ),
        (
            "a type no finding carries",
            criteria("type", json!({"equals": ["Recon:EC2/NoSuchFindingType"]})),
            &[],
        ),
    ];

    for (label, criteria, selected) in rows {
        let router = new_router();
        let detector_id = detector_with_findings(&router).await;

        let expected: BTreeSet<String> = selected
            .iter()
            .map(|index| sample_type(*index).to_string())
            .collect();
        if !uses_filter_only_condition(&criteria) {
            let listed = selected_types(&router, &detector_id, &criteria).await;
            assert_eq!(
                listed, expected,
                "case {label:?}: expected the criteria to select {expected:?}, got {listed:?}"
            );
        }

        create_filter_ok(
            &router,
            &detector_id,
            json!({
                "name": "suppress-the-selection",
                "action": ACTION_ARCHIVE,
                "findingCriteria": criteria,
            }),
        )
        .await;

        let archived = archived_types(&router, &detector_id).await;
        assert_eq!(
            archived, expected,
            "case {label:?}: expected exactly the selected findings to be archived, got {archived:?}"
        );

        // The stored state is what a findings read selects on, so the set a criterion
        // on `service.archived` reports is the suppressed set itself, id for id.
        let suppressed_ids = listed_ids(
            &router,
            &detector_id,
            json!({"findingCriteria": {"criterion": {"service.archived": {"equals": ["true"]}}}}),
        )
        .await;
        let suppressed_types: BTreeSet<String> = findings(&router, &detector_id)
            .await
            .into_iter()
            .filter(|finding| {
                suppressed_ids.contains(finding["id"].as_str().unwrap_or_default())
            })
            .map(|finding| finding["type"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            suppressed_types, expected,
            "case {label:?}: expected the filtered list to report exactly the suppressed findings, got {suppressed_ids:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the action decides whether a matching finding moves.
// ---------------------------------------------------------------------------

/// The rows are the whole `FilterAction` value set plus "names no action", over criteria
/// that select one finding: only the archiving member moves it, and the other two leave
/// every finding in the state the generation put it in.
#[tokio::test]
async fn the_filter_action_decides_whether_a_matching_finding_is_archived() {
    let rows: [(&str, Option<&str>, bool); 3] = [
        ("ARCHIVE", Some(ACTION_ARCHIVE), true),
        ("NOOP", Some(ACTION_NOOP), false),
        ("names no action", None, false),
    ];

    for (label, action, moves) in rows {
        let router = new_router();
        let detector_id = detector_with_findings(&router).await;

        let selected = sample_type(2);
        let mut body = json!({
            "name": "action-under-test",
            "findingCriteria": criteria("type", json!({"equals": [selected]})),
        });
        if let Some(action) = action {
            body["action"] = json!(action);
        }
        create_filter_ok(&router, &detector_id, body).await;

        let archived = archived_by_type(&router, &detector_id).await;
        for (finding_type, _) in SAMPLES {
            let expected = moves && finding_type == selected;
            assert_eq!(
                archived.get(finding_type).copied(),
                Some(expected),
                "case {label:?}: finding {finding_type:?} expected archived={expected}, got {:?}",
                archived.get(finding_type)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the order the filter and the findings arrive in does not matter.
// ---------------------------------------------------------------------------

/// A suppression filter is a standing rule, so it covers the findings already stored and
/// the findings generated after it alike. The rows differ along that order, along how
/// many calls the generation takes, and along a repeat generation of a type already
/// stored — which GuardDuty aggregates into the finding it already has.
#[tokio::test]
async fn a_filter_covers_the_findings_generated_after_it_as_well_as_before() {
    let selected = sample_type(1);
    let other = sample_type(3);
    let filter = json!({
        "name": "standing-suppression",
        "action": ACTION_ARCHIVE,
        "findingCriteria": criteria("type", json!({"equals": [selected]})),
    });

    let rows: [&str; 3] = [
        "the filter after the findings",
        "the filter before the findings",
        "the filter between two generations of the same type",
    ];

    for label in rows {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        match label {
            "the filter after the findings" => {
                create_samples(&router, &detector_id, &[selected, other]).await;
                create_filter_ok(&router, &detector_id, filter.clone()).await;
            }
            "the filter before the findings" => {
                create_filter_ok(&router, &detector_id, filter.clone()).await;
                create_samples(&router, &detector_id, &[selected]).await;
                create_samples(&router, &detector_id, &[other]).await;
            }
            _ => {
                create_samples(&router, &detector_id, &[selected, other]).await;
                create_filter_ok(&router, &detector_id, filter.clone()).await;
                // Another occurrence of the same activity: GuardDuty aggregates it
                // into the finding it already holds, which the standing filter still
                // suppresses.
                create_samples(&router, &detector_id, &[selected]).await;
            }
        }

        let archived = archived_by_type(&router, &detector_id).await;
        assert_eq!(
            archived.get(selected).copied(),
            Some(true),
            "case {label:?}: expected the selected finding to be archived, got {archived:?}"
        );
        assert_eq!(
            archived.get(other).copied(),
            Some(false),
            "case {label:?}: expected the unselected finding to be left alone, got {archived:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an update applies the same rule to the filter it leaves behind.
// ---------------------------------------------------------------------------

/// `UpdateFilter` writes the members the request names and leaves the rest as they were,
/// so what the updated filter suppresses follows from the filter the update leaves
/// behind. The rows differ along which member the update names — the action, the
/// criteria, both, neither — and along the action the create stored.
#[tokio::test]
async fn an_update_applies_the_filter_it_leaves_behind() {
    // (label, created action, update body, the types expected archived afterwards)
    let rows: [(&str, &str, Value, &[usize]); 5] = [
        (
            "an update into the archiving action",
            ACTION_NOOP,
            json!({"action": ACTION_ARCHIVE}),
            &[1],
        ),
        (
            "an update of the criteria under a stored archiving action",
            ACTION_ARCHIVE,
            json!({"findingCriteria": {"criterion": {"type": {"equals": [sample_type(3)]}}}}),
            // The finding the create already archived stays archived: nothing
            // unarchives it, and the new criteria select another one.
            &[1, 3],
        ),
        (
            "an update of the criteria under a stored NOOP action",
            ACTION_NOOP,
            json!({"findingCriteria": {"criterion": {"type": {"equals": [sample_type(3)]}}}}),
            &[],
        ),
        (
            "an update naming both members",
            ACTION_NOOP,
            json!({
                "action": ACTION_ARCHIVE,
                "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 8}}},
            }),
            &[3, 4],
        ),
        (
            "an update naming neither member",
            ACTION_ARCHIVE,
            json!({"description": "a description and nothing else"}),
            &[1],
        ),
    ];

    for (label, created_action, update, expected) in rows {
        let router = new_router();
        let detector_id = detector_with_findings(&router).await;

        create_filter_ok(
            &router,
            &detector_id,
            json!({
                "name": "updated-filter",
                "action": created_action,
                "findingCriteria": criteria("type", json!({"equals": [sample_type(1)]})),
            }),
        )
        .await;
        update_filter_ok(&router, &detector_id, "updated-filter", update).await;

        let archived = archived_types(&router, &detector_id).await;
        let expected: BTreeSet<String> = expected
            .iter()
            .map(|index| sample_type(*index).to_string())
            .collect();
        assert_eq!(
            archived, expected,
            "case {label:?}: expected exactly {expected:?} archived, got {archived:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: filters compose, and the explicit archive write still decides.
// ---------------------------------------------------------------------------

/// A detector may hold up to 100 saved filters, so "is this finding suppressed" is a
/// question about the whole set: a finding ANY archiving filter selects is archived, and
/// a filter that archives nothing takes nothing away from the others.
#[tokio::test]
async fn a_finding_any_archiving_filter_selects_is_archived() {
    let router = new_router();
    let detector_id = detector_with_findings(&router).await;

    create_filter_ok(
        &router,
        &detector_id,
        json!({
            "name": "suppress-one",
            "action": ACTION_ARCHIVE,
            "findingCriteria": criteria("type", json!({"equals": [sample_type(0)]})),
        }),
    )
    .await;
    create_filter_ok(
        &router,
        &detector_id,
        json!({
            "name": "suppress-another",
            "action": ACTION_ARCHIVE,
            "findingCriteria": criteria("severity", json!({"greaterThanOrEqual": 9})),
        }),
    )
    .await;
    create_filter_ok(
        &router,
        &detector_id,
        json!({
            "name": "suppress-nothing",
            "action": ACTION_NOOP,
            "findingCriteria": criteria("type", json!({"equals": [sample_type(2)]})),
        }),
    )
    .await;

    let archived = archived_types(&router, &detector_id).await;
    let expected: BTreeSet<String> = [sample_type(0).to_string(), sample_type(4).to_string()]
        .into_iter()
        .collect();
    assert_eq!(
        archived, expected,
        "expected the union of what the archiving filters select, got {archived:?}"
    );
}

/// `ArchiveFindings`/`UnarchiveFindings` are the explicit write over the same state, so
/// the last call to land decides it: a caller may unarchive a suppressed finding, and it
/// stays unarchived until the filter that selects it is saved over it again — saving
/// another filter applies that other filter and nothing else.
#[tokio::test]
async fn the_last_write_decides_a_suppressed_findings_archived_state() {
    let router = new_router();
    let detector_id = detector_with_findings(&router).await;
    let selected = sample_type(1);

    create_filter_ok(
        &router,
        &detector_id,
        json!({
            "name": "suppress-then-unarchive",
            "action": ACTION_ARCHIVE,
            "findingCriteria": criteria("type", json!({"equals": [selected]})),
        }),
    )
    .await;
    let ids = ids_of_types(&router, &detector_id, &[selected]).await;
    assert_eq!(
        ids.len(),
        1,
        "expected one finding of the selected type, got {ids:?}"
    );
    assert_eq!(
        archived_by_type(&router, &detector_id).await.get(selected).copied(),
        Some(true),
        "expected the saved filter to archive the finding it selects"
    );

    set_archived_ok(&router, &detector_id, &ids, false).await;
    assert_eq!(
        archived_by_type(&router, &detector_id).await.get(selected).copied(),
        Some(false),
        "expected the explicit unarchive to stand"
    );

    // Saving an unrelated filter applies THAT filter, so a finding it does not select
    // keeps the state the explicit write left it in.
    create_filter_ok(
        &router,
        &detector_id,
        json!({
            "name": "an-unrelated-filter",
            "action": ACTION_ARCHIVE,
            "findingCriteria": criteria("type", json!({"equals": [sample_type(3)]})),
        }),
    )
    .await;
    let archived = archived_by_type(&router, &detector_id).await;
    assert_eq!(
        archived.get(selected).copied(),
        Some(false),
        "expected an unrelated filter to leave the unarchived finding alone, got {archived:?}"
    );
    assert_eq!(
        archived.get(sample_type(3)).copied(),
        Some(true),
        "expected the unrelated filter to archive what it does select, got {archived:?}"
    );

    // Saving the filter that selects it again applies it again, as the create did.
    update_filter_ok(
        &router,
        &detector_id,
        "suppress-then-unarchive",
        json!({"rank": 1}),
    )
    .await;
    assert_eq!(
        archived_by_type(&router, &detector_id).await.get(selected).copied(),
        Some(true),
        "expected saving the filter again to apply it again"
    );
}

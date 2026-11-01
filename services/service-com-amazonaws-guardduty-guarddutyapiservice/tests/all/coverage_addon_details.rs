// SPDX-License-Identifier: CC0-1.0

//! What an Amazon EKS coverage row reports about the GuardDuty security agent add-on.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//! the two members of `AddonDetails` carry two facts the owning service answers for
//! independently of one another, and neither is required
//! (`smithy_query(shape="AddonDetails")`), so a coverage row reports EACH of them from
//! its own fact — the structure carries every member this service holds a fact for and
//! is absent only when it holds neither. A member is therefore never dropped because a
//! sibling member is absent, which would leave the row stating a `coverageStatus` and
//! an `issue` computed from the add-on's status while reporting no add-on detail at
//! all.
//!
//! The rows a coverage report carries are the account's own Amazon EKS clusters, which
//! belong to the service that owns them, so a router carrying only this service has
//! nobody to ask: the projection is driven here over the whole domain of facts a row
//! can hold, and the same rule is asserted on the WIRE, over rows computed from a real
//! inventory, in `core/router/tests/all/guardduty_runtime_coverage.rs`. The operation
//! itself is driven end to end below, so the report a client reads and the projection
//! under test are the same code path.

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::coverage;

use super::detector_lifecycle::{call, create_detector, new_router, Router};

/// Every `AddonStatus` member the owning service can report for an installed add-on
/// (`smithy_query(shape="AddonStatus")` on the Amazon EKS model): the status fact a
/// coverage row can hold, whatever the add-on's version fact is.
const ADDON_STATUSES: [&str; 8] = [
    "CREATING",
    "ACTIVE",
    "CREATE_FAILED",
    "UPDATING",
    "DELETING",
    "DELETE_FAILED",
    "DEGRADED",
    "UPDATE_FAILED",
];

/// Add-on versions spanning the forms `AddonDetails.AddonVersion` carries — the build
/// of the agent add-on EKS add-on management reports — including the oldest build the
/// Amazon EKS add-on catalog publishes for the agent and a two-digit minor.
const ADDON_VERSIONS: [&str; 3] = [
    "v1.6.1-eksbuild.1",
    "v1.10.0-eksbuild.3",
    "v2.0.0-eksbuild.11",
];

/// The `addonDetails` of one EKS row, as the wire carries it.
fn addon_details(row: &Value) -> Option<&Value> {
    row.get("resourceDetails")
        .and_then(|details| details.get("eksClusterDetails"))
        .and_then(|details| details.get("addonDetails"))
        .filter(|details| !details.is_null())
}

// ---------------------------------------------------------------------------
// The rule: each member is reported from its own fact.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_agent_addon_fact_a_row_holds_is_reported_on_its_own() {
    // The domain is both facts crossed with their own absence: a row holding both, one
    // holding only the status (the combination the owning service's own answer admits,
    // its version and its status being separate optional members), one holding only the
    // version, and one holding neither.
    let mut cases: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    for status in ADDON_STATUSES {
        cases.push((
            format!("status {status} with no version"),
            None,
            Some(status.to_string()),
        ));
        for version in ADDON_VERSIONS {
            cases.push((
                format!("version {version} with status {status}"),
                Some(version.to_string()),
                Some(status.to_string()),
            ));
        }
    }
    for version in ADDON_VERSIONS {
        cases.push((
            format!("version {version} with no status"),
            Some(version.to_string()),
            None,
        ));
    }
    cases.push(("neither fact".to_string(), None, None));

    for (case, version, status) in cases {
        let details = coverage::agent_addon_details(version.as_deref(), status.as_deref());
        if version.is_none() && status.is_none() {
            assert!(
                details.is_none(),
                "case {case:?}: a row holding no fact about the add-on reports no \
                 addonDetails, got {details:?}"
            );
            continue;
        }
        let reported = details.unwrap_or_else(|| {
            panic!("case {case:?}: expected addonDetails for a row that holds a fact about the add-on, got none")
        });
        assert_eq!(
            reported.addon_version, version,
            "case {case:?}: expected addonVersion {version:?}, got {:?}",
            reported.addon_version
        );
        assert_eq!(
            reported.addon_status, status,
            "case {case:?}: expected addonStatus {status:?}, got {:?}",
            reported.addon_status
        );
    }
}

/// The structure is absent ONLY where no fact is held: for every status, a row holding
/// just that status reports it, and reports no version it does not hold.
#[tokio::test]
async fn a_status_without_a_version_is_still_reported() {
    for status in ADDON_STATUSES {
        let case = format!("status {status} with no version");
        let details = coverage::agent_addon_details(None, Some(status))
            .unwrap_or_else(|| panic!("case {case:?}: expected addonDetails, got none"));
        assert_eq!(
            details.addon_status.as_deref(),
            Some(status),
            "case {case:?}: expected addonStatus {status:?}, got {:?}",
            details.addon_status
        );
        assert!(
            details.addon_version.is_none(),
            "case {case:?}: a row holding no version reports none, got {:?}",
            details.addon_version
        );
    }
}

// ---------------------------------------------------------------------------
// The same rule on the wire: whatever rows the report carries, an `addonDetails`
// it carries is one with a fact in it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_report_never_carries_an_addon_details_with_no_fact_in_it() {
    let router: Router = new_router();
    // Runtime Monitoring enabled, which is the configuration under which the Amazon EKS
    // clusters are covered at all and so the one under which a row could carry add-on
    // details.
    let (status, created) = create_detector(
        &router,
        json!({"enable": true, "features": [
            {"name": "RUNTIME_MONITORING", "status": "ENABLED"},
        ]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-detector expected 200, got {status} {created:?}"
    );
    let detector_id = created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a detectorId, got {created:?}"))
        .to_string();

    // The paging, criterion and sort shapes a client reads the report through: each
    // answer's rows are checked against the rule.
    let bodies: Vec<(&str, Value)> = vec![
        ("the whole report", json!({})),
        (
            "narrowed to the Amazon EKS rows",
            json!({"filterCriteria": {"filterCriterion": [
                {"criterionKey": "RESOURCE_TYPE", "filterCondition": {"equals": ["EKS"]}},
            ]}}),
        ),
        (
            "ordered by the add-on version",
            json!({"sortCriteria": {"attributeName": "ADDON_VERSION", "orderBy": "DESC"}}),
        ),
        ("one row at a time", json!({"maxResults": 1})),
    ];

    for (case, body) in bodies {
        let (status, response) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/coverage"),
            Some(body),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: list-coverage expected 200, got {status} {response:?}"
        );
        let rows = response
            .get("resources")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for row in &rows {
            let Some(details) = addon_details(row) else {
                continue;
            };
            let members = details
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: expected an addonDetails object, got {details:?}"))
                .iter()
                .filter(|(_, value)| !value.is_null())
                .count();
            assert!(
                members > 0,
                "case {case:?}: addonDetails is reported only for a fact the row holds, \
                 got {details:?} in {row:?}"
            );
        }
    }
}

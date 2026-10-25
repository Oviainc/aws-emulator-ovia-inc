// SPDX-License-Identifier: CC0-1.0

//! The benefit application's workflow `Stage`, end to end over the wire.
//!
//! Every assertion goes through the real request path — `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — and every effect is read back through
//! `GetBenefitApplication` and `ListBenefitApplications`, so what these tests pin is the
//! emulator's HTTP behaviour on the persisted record.
//!
//! The rule under test: an application's `Stage` is the position in AWS's processing
//! workflow that its CURRENT `BenefitApplicationStatus` puts it at, reported on every read
//! that models the member and selectable through the `Stages` filter; and the members that
//! explain the current status (`StatusReason`, `StatusReasonCodes`) describe the status the
//! application holds now, so a transition that carries no reason leaves none behind.
//!
//! The tables' rows differ along what that rule generalises over: which of the six
//! `BenefitApplicationStatus` values the application holds (the generated
//! `BENEFIT_APPLICATION_STATUS_VALUES`, so the row set cannot drift from the model), which
//! seeded catalog it lives in, which identifier form names it, and which operation moved
//! it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::BENEFIT_APPLICATION_STATUS_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    stage_for_status, BENEFIT_APPLICATION_STAGE_VALUES, SEEDED_CATALOGS, STAGE_CLOSED,
    STAGE_DECISION, STAGE_DRAFT, STAGE_PARTNER_ACTION, STAGE_REVIEW, STATUS_ACTION_REQUIRED,
    STATUS_APPROVED, STATUS_CANCELED, STATUS_IN_REVIEW, STATUS_PENDING_SUBMISSION,
    STATUS_REJECTED,
};

use crate::benefit_application_lifecycle::{instant, text, Harness, CATALOG};

/// The stage each `BenefitApplicationStatus` stands at, written out here rather than read
/// off the implementation so this table pins the pairing instead of restating it.
///
/// The status column is the model's own `BenefitApplicationStatus` enum
/// (`smithy_query(query="shape",shape="BenefitApplicationStatus")`, also generated as
/// `BENEFIT_APPLICATION_STATUS_VALUES`); the stage column is this service's stage
/// vocabulary, which the model leaves an unconstrained string.
const STAGE_OF_STATUS: &[(&str, &str)] = &[
    (STATUS_PENDING_SUBMISSION, STAGE_DRAFT),
    (STATUS_IN_REVIEW, STAGE_REVIEW),
    (STATUS_ACTION_REQUIRED, STAGE_PARTNER_ACTION),
    (STATUS_APPROVED, STAGE_DECISION),
    (STATUS_REJECTED, STAGE_DECISION),
    (STATUS_CANCELED, STAGE_CLOSED),
];

/// The stage the table above pairs with `status`.
fn expected_stage(status: &str) -> &'static str {
    STAGE_OF_STATUS
        .iter()
        .find(|(name, _)| *name == status)
        .map(|(_, stage)| *stage)
        .unwrap_or_else(|| panic!("the stage table covers every status, but not {status:?}"))
}

/// Create one application in `catalog` and return `(id, arn)`.
async fn seed(harness: &Harness, case: &str, catalog: &str, token: &str) -> (String, String) {
    let benefit = harness.active_benefit_of(catalog).await;
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": catalog,
                "ClientToken": token,
                "BenefitIdentifier": text(case, &benefit, "Id"),
                "Name": "stage-subject",
            }),
        )
        .await;
    (text(case, &created, "Id"), text(case, &created, "Arn"))
}

/// Put the application into `status`, through the operations that reach it where this
/// service has one and directly in the store where only AWS's own review does.
async fn reach_status(harness: &Harness, case: &str, catalog: &str, identifier: &str, status: &str) {
    match status {
        STATUS_PENDING_SUBMISSION => {}
        STATUS_IN_REVIEW => {
            harness
                .ok(
                    case,
                    "SubmitBenefitApplication",
                    &json!({ "Catalog": catalog, "Identifier": identifier }),
                )
                .await;
        }
        STATUS_CANCELED => {
            harness
                .ok(
                    case,
                    "CancelBenefitApplication",
                    &json!({
                        "Catalog": catalog,
                        "Identifier": identifier,
                        "ClientToken": format!("tok-cancel-{identifier}"),
                    }),
                )
                .await;
        }
        // `ACTION_REQUIRED`, `APPROVED` and `REJECTED` are set by AWS's review workflow,
        // which no operation of this slice drives.
        other => harness.place_status(case, catalog, identifier, other),
    }
}

/// Every summary of one catalog under `filter`, by id.
async fn listed_ids(harness: &Harness, case: &str, catalog: &str, filter: &Value) -> BTreeSet<String> {
    let mut body = filter.clone();
    body["Catalog"] = json!(catalog);
    body["MaxResults"] = json!(100);
    let out = harness.ok(case, "ListBenefitApplications", &body).await;
    out["BenefitApplicationSummaries"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| text(case, summary, "Id"))
        .collect()
}

/// One application's summary out of an unfiltered listing of its catalog.
async fn summary_of(harness: &Harness, case: &str, catalog: &str, id: &str) -> Value {
    let out = harness
        .ok(
            case,
            "ListBenefitApplications",
            &json!({ "Catalog": catalog, "MaxResults": 100 }),
        )
        .await;
    out["BenefitApplicationSummaries"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|summary| summary["Id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the listing of {catalog:?} omits {id:?}: {out:?}"))
}

/// Every status reports its workflow stage on `GetBenefitApplication` and on the
/// `ListBenefitApplications` summary, and the `Stages` filter selects exactly the
/// applications standing at the named stage.
///
/// The rows are every `BenefitApplicationStatus` the model declares, crossed with every
/// seeded catalog, each read back by both identifier forms the modelled
/// `BenefitApplicationIdentifier` pattern admits.
#[tokio::test]
async fn every_status_reports_its_workflow_stage() {
    let harness = Harness::new();

    for catalog in SEEDED_CATALOGS {
        for (row, status) in BENEFIT_APPLICATION_STATUS_VALUES.iter().enumerate() {
            let case = format!("{catalog}, status {status}");
            let (id, arn) = seed(&harness, &case, catalog, &format!("tok-{catalog}-{row}")).await;
            reach_status(&harness, &case, catalog, &id, status).await;
            let stage = expected_stage(status);

            for identifier in [id.clone(), arn.clone()] {
                let read = harness.get_in(&case, catalog, &identifier).await;
                assert_eq!(
                    read["Status"],
                    json!(status),
                    "case {case:?}: expected the application to hold {status}, got {read:?}"
                );
                assert_eq!(
                    read["Stage"],
                    json!(stage),
                    "case {case:?}: expected Stage {stage:?} for status {status}, got {read:?}"
                );
            }

            let summary = summary_of(&harness, &case, catalog, &id).await;
            assert_eq!(
                summary["Stage"],
                json!(stage),
                "case {case:?}: expected the summary Stage {stage:?}, got {summary:?}"
            );

            // The filter selects by that stage, and by no other stage this service reports.
            for candidate in BENEFIT_APPLICATION_STAGE_VALUES {
                let selected =
                    listed_ids(&harness, &case, catalog, &json!({ "Stages": [candidate] })).await;
                assert_eq!(
                    selected.contains(&id),
                    *candidate == stage,
                    "case {case:?}: the Stages filter {candidate:?} must {} {id:?} (its stage is {stage:?}), got {selected:?}",
                    if *candidate == stage { "select" } else { "not select" }
                );
            }

            // A stage no application stands at selects nothing of this catalog.
            let none = listed_ids(
                &harness,
                &case,
                catalog,
                &json!({ "Stages": ["not-a-stage-this-service-reports"] }),
            )
            .await;
            assert!(
                none.is_empty(),
                "case {case:?}: an unknown stage must select nothing, got {none:?}"
            );
        }
    }
}

/// The stage follows the status through the lifecycle this service's own operations drive:
/// a created application is at the draft stage, submitting it moves it to the review stage,
/// recalling it takes it back to draft, and cancelling it closes it — each change visible
/// through both the read and the listing's `Stages` filter.
///
/// The rows are every seeded catalog crossed with both identifier forms, with a differently
/// shaped recall `Reason` per row.
#[tokio::test]
async fn the_stage_follows_the_status_through_the_lifecycle() {
    let harness = Harness::new();

    // `Reason` is an unconstrained `String`, so the rows span its shapes.
    let reasons: [&str; 4] = [
        "x",
        "the partner is reworking the figures",
        "montant incorrect — données à revoir ✓",
        "needs rework: the attached plan is the wrong revision and the requested amount has \
         to be recalculated before AWS reviews it again",
    ];

    let mut row = 0usize;
    for catalog in SEEDED_CATALOGS {
        for by_arn in [false, true] {
            let case = format!("{catalog}, by_arn {by_arn}");
            let reason = reasons[row % reasons.len()];
            row += 1;

            let (id, arn) = seed(&harness, &case, catalog, &format!("tok-cycle-{row}")).await;
            let identifier = if by_arn { arn.clone() } else { id.clone() };

            let drafted = harness.get_in(&case, catalog, &identifier).await;
            assert_eq!(
                drafted["Stage"],
                json!(STAGE_DRAFT),
                "case {case:?}: a created application must stand at {STAGE_DRAFT}, got {drafted:?}"
            );

            harness
                .ok(
                    &case,
                    "SubmitBenefitApplication",
                    &json!({ "Catalog": catalog, "Identifier": identifier }),
                )
                .await;
            let submitted = harness.get_in(&case, catalog, &identifier).await;
            assert_eq!(
                submitted["Stage"],
                json!(STAGE_REVIEW),
                "case {case:?}: a submitted application must stand at {STAGE_REVIEW}, got {submitted:?}"
            );
            assert!(
                instant(&case, &submitted, "UpdatedAt") >= instant(&case, &drafted, "UpdatedAt"),
                "case {case:?}: submitting must not move UpdatedAt backwards, got {submitted:?}"
            );
            let in_review =
                listed_ids(&harness, &case, catalog, &json!({ "Stages": [STAGE_REVIEW] })).await;
            assert!(
                in_review.contains(&id),
                "case {case:?}: the {STAGE_REVIEW} filter must report {id:?}, got {in_review:?}"
            );

            harness
                .ok(
                    &case,
                    "RecallBenefitApplication",
                    &json!({ "Catalog": catalog, "Identifier": identifier, "Reason": reason }),
                )
                .await;
            let recalled = harness.get_in(&case, catalog, &identifier).await;
            assert_eq!(
                recalled["Stage"],
                json!(STAGE_DRAFT),
                "case {case:?}: a recalled application must stand at {STAGE_DRAFT}, got {recalled:?}"
            );
            assert_eq!(
                recalled["StatusReason"],
                json!(reason),
                "case {case:?}: the recall reason must explain the draft status, got {recalled:?}"
            );

            // Re-submitting leaves no explanation of the status the application has left:
            // the recall's reason explained the draft, not the review.
            harness
                .ok(
                    &case,
                    "SubmitBenefitApplication",
                    &json!({ "Catalog": catalog, "Identifier": identifier }),
                )
                .await;
            let resubmitted = harness.get_in(&case, catalog, &identifier).await;
            assert_eq!(
                resubmitted["Stage"],
                json!(STAGE_REVIEW),
                "case {case:?}: a re-submitted application must stand at {STAGE_REVIEW}, got {resubmitted:?}"
            );
            assert!(
                resubmitted
                    .get("StatusReason")
                    .is_none_or(Value::is_null),
                "case {case:?}: a submission carries no reason, so none must survive it, got {resubmitted:?}"
            );
            assert!(
                resubmitted
                    .get("StatusReasonCodes")
                    .is_none_or(Value::is_null),
                "case {case:?}: no StatusReasonCodes must survive a submission, got {resubmitted:?}"
            );

            harness
                .ok(
                    &case,
                    "CancelBenefitApplication",
                    &json!({
                        "Catalog": catalog,
                        "Identifier": identifier,
                        "ClientToken": format!("tok-cancel-{row}"),
                    }),
                )
                .await;
            let canceled = harness.get_in(&case, catalog, &identifier).await;
            assert_eq!(
                canceled["Stage"],
                json!(STAGE_CLOSED),
                "case {case:?}: a cancelled application must stand at {STAGE_CLOSED}, got {canceled:?}"
            );
            assert!(
                canceled.get("StatusReason").is_none_or(Value::is_null),
                "case {case:?}: a cancel with no Reason must leave no StatusReason, got {canceled:?}"
            );
            let closed =
                listed_ids(&harness, &case, catalog, &json!({ "Stages": [STAGE_CLOSED] })).await;
            assert!(
                closed.contains(&id),
                "case {case:?}: the {STAGE_CLOSED} filter must report {id:?}, got {closed:?}"
            );
            let still_in_review =
                listed_ids(&harness, &case, catalog, &json!({ "Stages": [STAGE_REVIEW] })).await;
            assert!(
                !still_in_review.contains(&id),
                "case {case:?}: {id:?} has left {STAGE_REVIEW}, got {still_in_review:?}"
            );
        }
    }
}

/// Every status the model declares stands at a stage this service publishes, and nothing
/// else does: the projection is total over `BenefitApplicationStatus`, so no stored
/// application can report an absent `Stage`.
#[tokio::test]
async fn the_stage_projection_covers_every_modelled_status() {
    for status in BENEFIT_APPLICATION_STATUS_VALUES {
        let stage = stage_for_status(status).unwrap_or_else(|| {
            panic!("expected a stage for the modelled status {status:?}, got none")
        });
        assert_eq!(
            stage,
            expected_stage(status),
            "case {status:?}: expected stage {:?}, got {stage:?}",
            expected_stage(status)
        );
        assert!(
            BENEFIT_APPLICATION_STAGE_VALUES.contains(&stage),
            "case {status:?}: {stage:?} is not one of the published stages {BENEFIT_APPLICATION_STAGE_VALUES:?}"
        );
    }

    for unmodelled in ["", "DRAFT", "in_review", "QUALIFY"] {
        assert!(
            stage_for_status(unmodelled).is_none(),
            "case {unmodelled:?}: a value that is no BenefitApplicationStatus has no stage, got {:?}",
            stage_for_status(unmodelled)
        );
    }
}

/// The `Stages` filter is a filter, not a page: an application standing at a stage is
/// reported for that stage and omitted from every other, whichever catalog it lives in.
///
/// The row set is every published stage, each asserted against the set of applications
/// placed at it — one per status, so two statuses sharing a stage are both selected by it.
#[tokio::test]
async fn the_stages_filter_selects_every_application_at_that_stage() {
    let harness = Harness::new();
    let mut expected: Vec<(&str, String)> = Vec::new();

    for (row, status) in BENEFIT_APPLICATION_STATUS_VALUES.iter().enumerate() {
        let case = format!("status {status}");
        let (id, _) = seed(&harness, &case, CATALOG, &format!("tok-filter-{row}")).await;
        reach_status(&harness, &case, CATALOG, &id, status).await;
        expected.push((expected_stage(status), id));
    }

    for stage in BENEFIT_APPLICATION_STAGE_VALUES {
        let case = format!("stage {stage}");
        let selected = listed_ids(&harness, &case, CATALOG, &json!({ "Stages": [stage] })).await;
        let wanted: BTreeSet<String> = expected
            .iter()
            .filter(|(at, _)| at == stage)
            .map(|(_, id)| id.clone())
            .collect();
        assert_eq!(
            selected, wanted,
            "case {case:?}: expected the applications at {stage:?} to be {wanted:?}, got {selected:?}"
        );
    }
}

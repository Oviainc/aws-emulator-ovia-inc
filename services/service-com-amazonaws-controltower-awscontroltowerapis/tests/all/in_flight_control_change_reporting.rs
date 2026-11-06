// SPDX-License-Identifier: CC0-1.0

//! What EVERY reporting call answers about an enabled control while a configuration change
//! it accepted is still being deployed, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes and JSON
//! bodies — not the handlers' Rust signatures.
//!
//! The RULE under test: the transient state an enabled control is in while a configuration
//! change is in flight is ONE state, reported the same way by every call that reports it —
//! `UNDER_CHANGE` with no drift assessment (`UNKNOWN` in all three dimensions, the value
//! for which the service "is not able to check the drift status"), whichever call started
//! the change, because drift is a property of a DEPLOYED configuration. So
//! `GetEnabledControl` and `ListEnabledControls` cannot answer "the configuration is
//! changing" and "the control matches the configuration we deployed" about the same
//! control, and the listing's drift filters select that control under `UNKNOWN` rather than
//! under the assessment its previous configuration earned. A control nobody changed keeps
//! the assessment it has.
//!
//! The sibling suite `in_flight_configuration_change_drift` pins the same rule through
//! `GetEnabledControl`; this one pins that the LISTING — the body and the three drift
//! members of its `filter` — agrees with it, which is the surface on which a stale
//! assessment would also decide which controls a caller is shown.
//!
//! Asserted over the SAME table as that suite (`control_rows`), whose rows are what the
//! rule generalises over — which call starts deploying a configuration, and which
//! configuration it deploys — so no control, target or parameter key is written out here:
//! the rows are built off `state::CONTROL_CATALOG`, the catalog these operations resolve an
//! identifier against. Each row is read back straight after ITS OWN change, so every
//! in-flight assertion falls inside that row's own window rather than depending on how long
//! the rest of the table took; the exact-set assertions are made once everything has
//! settled, where the set is stable.
//!
//! The enum values and filter-list bounds written out by hand come from
//! `smithy_query(query="shape", shape="EnablementStatus,DriftStatus,EnabledControlFilter")`
//! on this crate's `smithy-model/`: `SUCCEEDED` | `FAILED` | `UNDER_CHANGE`, `DRIFTED` |
//! `IN_SYNC` | `NOT_CHECKING` | `UNKNOWN`, and every member of `EnabledControlFilter` is a
//! list of length 1..1 — so a case sends exactly one value per member.

use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{
    details, drift_dimensions, enable_with, listed_arns, listed_entry, ou,
    status_summary_status as control_status,
};
use crate::in_flight_configuration_change_drift::{control_rows, start_control_change, ControlRow};
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// The three `EnabledControlFilter` members that narrow the listing by one of the drift
/// dimensions a control reports.
pub(crate) const DRIFT_FILTER_MEMBERS: &[&str] = &[
    "driftStatuses",
    "inheritanceDriftStatuses",
    "resourceDriftStatuses",
];

/// `ListEnabledControls` over the whole store, narrowed by `filter` (`Value::Null` asks for
/// no narrowing at all). `maxResults` is inside the modelled range 1..200 and above the
/// number of controls this suite enables, so every answer is one page.
pub(crate) async fn listing(router: &MultiServiceRouter, filter: serde_json::Value) -> Answer {
    let mut body = serde_json::json!({ "maxResults": 100 });
    if !filter.is_null() {
        body["filter"] = filter;
    }
    let listed = call(router, "/list-enabled-controls", body.clone()).await;
    assert_eq!(
        listed.status, 200,
        "list-enabled-controls {body}: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    listed
}

/// The `filter` that narrows the listing by `member` to the controls reporting `status` for
/// the drift dimension it narrows by. One value, the modelled maximum length of the list.
pub(crate) fn drift_filter(member: &str, status: &str) -> serde_json::Value {
    let mut filter = serde_json::Map::new();
    filter.insert(member.to_string(), serde_json::json!([status]));
    serde_json::Value::Object(filter)
}

/// The state every reporting call reports about the control `arn` names, asserting the two
/// agree — they read one stored record, so a difference between them is the defect.
pub(crate) async fn reported_everywhere(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> (String, (String, String, String)) {
    let detail = details(router, arn).await;
    let listed = listing(router, serde_json::Value::Null).await;
    let entry = listed_entry(&listed, arn);

    assert_eq!(
        (
            entry.get("statusSummary"),
            entry.get("driftStatusSummary")
        ),
        (
            detail.get("statusSummary"),
            detail.get("driftStatusSummary")
        ),
        "case {case:?}: the listing and the get must report one state for {arn}, got \
         {entry:?} and {detail:?}"
    );

    // Both reporting operations carry the same two summary members, so one reading applies
    // to a `GetEnabledControl` detail and to a listed entry.
    let (drift, inheritance, resource) = drift_dimensions(&detail);
    (
        control_status(&detail).unwrap_or_default().to_string(),
        (
            drift.unwrap_or_default().to_string(),
            inheritance.unwrap_or_default().to_string(),
            resource.unwrap_or_default().to_string(),
        ),
    )
}

/// The state `status` with all three drift dimensions at `drift`, as
/// [`reported_everywhere`] hands one back.
pub(crate) fn state(status: &str, drift: &str) -> (String, (String, String, String)) {
    (
        status.to_string(),
        (drift.to_string(), drift.to_string(), drift.to_string()),
    )
}

/// RULE: while a configuration change is in flight an enabled control is `UNDER_CHANGE`
/// with `UNKNOWN` in all three drift dimensions — whichever call started the change and
/// whichever configuration it deploys — in `GetEnabledControl`, in `ListEnabledControls`
/// and in which controls that listing's drift filters select; a control nobody changed
/// keeps its assessment, and the changed ones are assessed `IN_SYNC` again once the clock
/// makes the change due.
#[tokio::test]
async fn every_reporting_call_agrees_on_a_control_change_in_flight() {
    let router = set_up_emulator().await;

    let rows = control_rows();
    // A control nobody changes, so each case also pins that a change withdraws the
    // assessment of the control it changes and of no other. Its control is a row's, so no
    // catalog control is written out here, on a target of its own: a control is enabled at
    // most once per target.
    let untouched_target = ou("ifr0");
    let untouched_control = rows
        .first()
        .expect("the control table must carry at least one row")
        .control_identifier
        .clone();

    let mut enabled: Vec<(ControlRow, String)> = Vec::new();
    for row in rows {
        let (arn, _) = enable_with(
            &router,
            &row.control_identifier,
            &row.target_identifier,
            row.initial.clone(),
        )
        .await;
        enabled.push((row, arn));
    }
    let (untouched, _) = enable_with(&router, &untouched_control, &untouched_target, None).await;

    // The enables are asynchronous too, so wait for them: a control still being deployed
    // refuses a further change as a conflict, and the assessment a change withdraws has to
    // exist first.
    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        assert_eq!(
            reported_everywhere(&router, &row.case, arn).await,
            state("SUCCEEDED", "IN_SYNC"),
            "case {:?}: expected a deployed control assessed IN_SYNC before the change",
            row.case
        );
    }
    assert_eq!(
        reported_everywhere(&router, "the control nobody changes", &untouched).await,
        state("SUCCEEDED", "IN_SYNC"),
        "the control nobody changes must start out deployed and assessed IN_SYNC"
    );

    // Each row is read back straight after ITS OWN change, so every assertion below falls
    // inside the window that row's operation is in flight — the window is one settle span
    // wide, so a row read after the whole table had been started would depend on how long
    // the other rows took.
    for (row, arn) in &enabled {
        let answer = start_control_change(&router, row, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the change expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );

        assert_eq!(
            reported_everywhere(&router, &row.case, arn).await,
            state("UNDER_CHANGE", "UNKNOWN"),
            "case {:?}: expected UNDER_CHANGE with no drift assessment while the \
             configuration is still being deployed",
            row.case
        );

        // The same state decides which controls the listing SHOWS. The two controls whose
        // state is known at this instant are the one whose change just started and the one
        // nobody ever changes, so each drift member of `filter` is asserted on both: a row
        // whose own window may already have closed is not asserted about here, which is
        // what keeps the narrowing assertion independent of how long the calls took.
        for &member in DRIFT_FILTER_MEMBERS {
            let under_change = listing(&router, drift_filter(member, "UNKNOWN")).await;
            let unassessed = listed_arns(&under_change);
            assert!(
                unassessed.contains(arn),
                "case {:?}/{member:?}: expected the control being changed among the \
                 unassessed ones, got {:?}",
                row.case,
                under_change.body
            );
            assert!(
                !unassessed.contains(&untouched),
                "case {:?}/{member:?}: the control nobody changed must not be reported \
                 unassessed, got {:?}",
                row.case,
                under_change.body
            );

            let assessed = listing(&router, drift_filter(member, "IN_SYNC")).await;
            let in_sync = listed_arns(&assessed);
            assert!(
                in_sync.contains(&untouched),
                "case {:?}/{member:?}: expected the control nobody changed among the ones \
                 assessed IN_SYNC, got {:?}",
                row.case,
                assessed.body
            );
            assert!(
                !in_sync.contains(arn),
                "case {:?}/{member:?}: the control being changed must not be reported \
                 IN_SYNC, got {:?}",
                row.case,
                assessed.body
            );
        }
    }

    assert_eq!(
        reported_everywhere(&router, "the control nobody changes", &untouched).await,
        state("SUCCEEDED", "IN_SYNC"),
        "a change to other controls must not withdraw the assessment of {untouched}"
    );

    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        assert_eq!(
            reported_everywhere(&router, &row.case, arn).await,
            state("SUCCEEDED", "IN_SYNC"),
            "case {:?}: expected IN_SYNC in every dimension once the configuration is \
             deployed",
            row.case
        );
    }

    // Settled, the whole set is stable, so the narrowing is asserted as an exact set: every
    // control is assessed again and none is left unassessed.
    let mut everything: Vec<String> = enabled.iter().map(|(_, arn)| arn.clone()).collect();
    everything.push(untouched.clone());
    everything.sort();
    for &member in DRIFT_FILTER_MEMBERS {
        let under_change = listing(&router, drift_filter(member, "UNKNOWN")).await;
        assert!(
            listed_arns(&under_change).is_empty(),
            "case {member:?}: expected no control left unassessed once every change is \
             deployed, got {:?}",
            under_change.body
        );

        let assessed = listing(&router, drift_filter(member, "IN_SYNC")).await;
        assert_eq!(
            listed_arns(&assessed),
            everything,
            "case {member:?}: expected every control assessed IN_SYNC once every change is \
             deployed, got {:?}",
            assessed.body
        );
    }
}

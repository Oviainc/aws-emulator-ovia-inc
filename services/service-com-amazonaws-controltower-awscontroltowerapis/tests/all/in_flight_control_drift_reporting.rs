// SPDX-License-Identifier: CC0-1.0

//! What EVERY reporting call answers about an enabled control while the TEARDOWN it accepted
//! is still removing it, end to end over HTTP.
//!
//! Driven through the same `MultiServiceRouter` the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The RULE under test is the one `in_flight_control_change_reporting` asserts for the calls
//! that DEPLOY a configuration, here for the call that REMOVES one: the transient state an
//! enabled control is in while a change to what it has deployed is in flight is ONE state,
//! reported the same way by every call that reports it — `UNDER_CHANGE` with no drift
//! assessment (`UNKNOWN` in all three dimensions, the value for which the service "is not
//! able to check the drift status"), because drift is a property of a DEPLOYED configuration
//! and a teardown is removing the one that was deployed. So `GetEnabledControl` and
//! `ListEnabledControls` cannot answer "this control is being changed" and "the control
//! matches the configuration we deployed" about one control, and the listing's drift filters
//! select it under `UNKNOWN` rather than under the assessment its removed configuration
//! earned. A control nobody tore down keeps the assessment it has, and once the clock makes
//! the teardown due the enablement is gone from every reporting call.
//!
//! The sibling suite `in_flight_teardown_drift` pins this window through `GetEnabledControl`;
//! what this one adds is that the LISTING — the body and the three drift members of its
//! `filter` — agrees with it, which is the surface on which a stale assessment would also
//! decide which controls a caller is shown. Both halves of the rule therefore share one set
//! of helpers rather than each carrying its own.
//!
//! Asserted over the SAME table as that suite (`control_rows`), whose rows are what the rule
//! generalises over — WHICH configuration the control had deployed when its teardown started
//! (every parameter key the control declares, an empty configuration, no `parameters`
//! member) and WHICH control implementation the catalog publishes it with — so no control,
//! target or parameter key is written out here: the rows are built off
//! `state::CONTROL_CATALOG`, the catalog these operations resolve an identifier against.
//! Each row is read back straight after ITS OWN teardown, so every in-flight assertion falls
//! inside that row's own window rather than depending on how long the rest of the table took.
//!
//! The enum values and filter-list bounds written out by hand come from
//! `smithy_query(query="shape", shape="EnablementStatus,DriftStatus,EnabledControlFilter")`
//! on this crate's `smithy-model/`: `SUCCEEDED` | `FAILED` | `UNDER_CHANGE`, `DRIFTED` |
//! `IN_SYNC` | `NOT_CHECKING` | `UNKNOWN`, and every member of `EnabledControlFilter` is a
//! list of length 1..1 — so a case sends exactly one value per member. The refusal a
//! torn-down identifier earns comes from `smithy_query(query="operation",
//! operation="GetEnabledControl")`, which models `ResourceNotFoundException` (404).

use crate::enabled_control_lifecycle::{enable_with, listed_arns, ou};
use crate::in_flight_control_change_reporting::{
    drift_filter, listing, reported_everywhere, state as reported_state, DRIFT_FILTER_MEMBERS,
};
use crate::in_flight_teardown_drift::{
    control_rows, disable_control, get_enabled_control, ControlRow,
};
use crate::landing_zone_lifecycle::{past_the_settle_span, set_up_emulator};

/// RULE: while a teardown is in flight an enabled control is `UNDER_CHANGE` with `UNKNOWN`
/// in all three drift dimensions — whatever configuration it had deployed — in
/// `GetEnabledControl`, in `ListEnabledControls` and in which controls that listing's drift
/// filters select; a control nobody tore down keeps its assessment, and once the clock makes
/// the teardown due the enablement is gone from every reporting call.
#[tokio::test]
async fn every_reporting_call_agrees_on_a_control_teardown_in_flight() {
    let router = set_up_emulator().await;

    let rows = control_rows();
    // A control nobody tears down, so each case also pins that a teardown withdraws the
    // assessment of the control it removes and of no other. Its control is a row's, so no
    // catalog control is written out here, on a target of its own: a control is enabled at
    // most once per target.
    let untouched_target = ou("tdwn0");
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
    // refuses a teardown as a conflict, and the assessment a teardown withdraws has to exist
    // first. Nothing sleeps server-side; the wait is the test's.
    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        assert_eq!(
            reported_everywhere(&router, &row.case, arn).await,
            reported_state("SUCCEEDED", "IN_SYNC"),
            "case {:?}: expected a deployed control assessed IN_SYNC before the teardown",
            row.case
        );
    }
    assert_eq!(
        reported_everywhere(&router, "the control nobody tears down", &untouched).await,
        reported_state("SUCCEEDED", "IN_SYNC"),
        "the control nobody tears down must start out deployed and assessed IN_SYNC"
    );

    // Each row is read back straight after ITS OWN teardown, so every assertion below falls
    // inside the window that row's operation is in flight — the window is one settle span
    // wide, so a row read after the whole table had been started would depend on how long
    // the other rows took.
    for (row, arn) in &enabled {
        let answer = disable_control(&router, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: disable-control expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );

        assert_eq!(
            reported_everywhere(&router, &row.case, arn).await,
            reported_state("UNDER_CHANGE", "UNKNOWN"),
            "case {:?}: expected UNDER_CHANGE with no drift assessment while the teardown is \
             still running",
            row.case
        );

        // The same state decides which controls the listing SHOWS. The two controls whose
        // state is known at this instant are the one whose teardown just started and the one
        // nobody ever tears down, so each drift member of `filter` is asserted on both: a row
        // whose own window may already have closed is not asserted about here, which is what
        // keeps the narrowing assertion independent of how long the calls took.
        for &member in DRIFT_FILTER_MEMBERS {
            let torn_down = listing(&router, drift_filter(member, "UNKNOWN")).await;
            let unassessed = listed_arns(&torn_down);
            assert!(
                unassessed.contains(arn),
                "case {:?}/{member:?}: expected the control being torn down among the \
                 unassessed ones, got {:?}",
                row.case,
                torn_down.body
            );
            assert!(
                !unassessed.contains(&untouched),
                "case {:?}/{member:?}: the control nobody tore down must not be reported \
                 unassessed, got {:?}",
                row.case,
                torn_down.body
            );

            let assessed = listing(&router, drift_filter(member, "IN_SYNC")).await;
            let in_sync = listed_arns(&assessed);
            assert!(
                in_sync.contains(&untouched),
                "case {:?}/{member:?}: expected the control nobody tore down among the ones \
                 assessed IN_SYNC, got {:?}",
                row.case,
                assessed.body
            );
            assert!(
                !in_sync.contains(arn),
                "case {:?}/{member:?}: the control being torn down must not be reported \
                 IN_SYNC, got {:?}",
                row.case,
                assessed.body
            );
        }
    }

    assert_eq!(
        reported_everywhere(&router, "the control nobody tears down", &untouched).await,
        reported_state("SUCCEEDED", "IN_SYNC"),
        "a teardown of other controls must not withdraw the assessment of {untouched}"
    );

    tokio::time::sleep(past_the_settle_span()).await;

    // Once every teardown is due the enablements it removed are reported by nothing: not by
    // the detail call, not by the listing, and not by a listing narrowed to either reading.
    let settled = listing(&router, serde_json::Value::Null).await;
    let listed = listed_arns(&settled);
    for (row, arn) in &enabled {
        let gone = get_enabled_control(&router, arn).await;
        assert_eq!(
            (gone.status, gone.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {:?}: expected the enablement gone once its teardown is due, got {:?} body \
             {:?}",
            row.case,
            (gone.status, gone.error_code.as_deref()),
            gone.body
        );
        assert!(
            !listed.contains(arn),
            "case {:?}: expected the listing to hold no entry for a torn-down enablement, got \
             {:?}",
            row.case,
            settled.body
        );
    }

    // The one enablement left is the control nobody tore down, still assessed, and it is the
    // whole of what each drift filter now selects: a settled set is stable, so the narrowing
    // is asserted as an exact set here.
    assert_eq!(
        reported_everywhere(&router, "the control nobody tears down", &untouched).await,
        reported_state("SUCCEEDED", "IN_SYNC"),
        "the control nobody tore down must still be deployed and assessed IN_SYNC"
    );
    for &member in DRIFT_FILTER_MEMBERS {
        let unassessed = listing(&router, drift_filter(member, "UNKNOWN")).await;
        assert!(
            listed_arns(&unassessed).is_empty(),
            "case {member:?}: expected no control left unassessed once every teardown is due, \
             got {:?}",
            unassessed.body
        );

        let assessed = listing(&router, drift_filter(member, "IN_SYNC")).await;
        assert_eq!(
            listed_arns(&assessed),
            vec![untouched.clone()],
            "case {member:?}: expected the control nobody tore down to be the whole of what \
             IN_SYNC selects once every teardown is due, got {:?}",
            assessed.body
        );
    }
}

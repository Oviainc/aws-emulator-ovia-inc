// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ASYNCHRONOUS teardown `DeleteWorkspaceInstance`
//! performs, driven over the wire this service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`).
//!
//! The rule under test: deleting a resource this service tears down asynchronously
//! ACCEPTS the request and starts the teardown — it does not make the resource
//! disappear inside the call. The instance keeps being reported, by
//! `GetWorkspaceInstance` and by `ListWorkspaceInstances` (including under a
//! `ProvisionStates` filter naming the state it is in), while it moves through the
//! transient `DEALLOCATING` to the terminal `DEALLOCATED`; only once the teardown and
//! its retention have both fallen due does the id stop naming a stored record and get
//! answered like one that never existed.
//!
//! Every row below creates a DIFFERENT instance — differing in the state it is in
//! when the delete arrives, in whether it carries a `ClientToken` or tags, and in the
//! shape of its configuration — because the rule is about the delete path, not about
//! one recorded request. Nothing here can pass against a canned answer: every
//! assertion reads the instance back out of the store through `Get` or `List`.
//!
//! The wire helpers come from the sibling `workspace_instance_lifecycle` suite.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{
    call, create, error_code, harness, list_all_ids, minimal_create, ok, provision_state,
};

/// Every value `ProvisionStateEnum` declares, as the generated constant the model
/// drives (`smithy_query(query="shape",shape="ProvisionStateEnum")`), so this suite
/// cannot drift from the model's value list.
const EVERY_PROVISION_STATE: &[&str] = wsinst::ops::types::PROVISION_STATE_ENUM_VALUES;

/// A slack added to a wait that only has to wake PAST the deadline it waited for.
/// `tokio::time::sleep` never wakes early, so this only has to cover the resolution
/// of the timer, not the whole span.
const SLACK: Duration = Duration::from_millis(50);

/// [`SLACK`] for a wait with no window after it to stay inside.
const GENEROUS_SLACK: Duration = Duration::from_millis(250);

/// One instance to delete. The rows differ along what the rule generalises over: the
/// `ProvisionState` the instance is in when the delete arrives, and the stored
/// record's shape (its idempotency token, its tags, its configuration).
struct DeleteRow {
    case: &'static str,
    body: Value,
    /// Whether to let the `ALLOCATING -> ALLOCATED` move fall due before deleting, so
    /// the table spans a delete of a transient instance and of a settled one.
    settle_first: bool,
}

fn delete_rows() -> Vec<DeleteRow> {
    vec![
        DeleteRow {
            case: "deleted while still ALLOCATING",
            body: minimal_create(),
            settle_first: false,
        },
        DeleteRow {
            case: "deleted once it reached ALLOCATED",
            body: minimal_create(),
            settle_first: true,
        },
        DeleteRow {
            case: "an instance created with an idempotency token",
            body: json!({ "ClientToken": "tok-teardown", "ManagedInstance": {} }),
            settle_first: true,
        },
        DeleteRow {
            case: "an instance carrying tags",
            body: json!({
                "ManagedInstance": { "InstanceType": "t3.medium" },
                "Tags": [{ "Key": "env", "Value": "dev" }, { "Key": "owner", "Value": "alice" }]
            }),
            settle_first: false,
        },
        DeleteRow {
            case: "an instance with a nested configuration and monthly billing",
            body: json!({
                "ManagedInstance": {
                    "InstanceType": "m5.large",
                    "Placement": { "AvailabilityZone": "us-east-1a", "Tenancy": "default" },
                    "CpuOptions": { "CoreCount": 2, "ThreadsPerCore": 2 }
                },
                "BillingConfiguration": { "BillingMode": "MONTHLY" }
            }),
            settle_first: true,
        },
    ]
}

/// The instant the `DEALLOCATING -> DEALLOCATED` move falls due for `id`, read
/// straight off the stored record: the wire cannot show a deadline, and a test that
/// asserted the transient state without one would be racing the clock.
fn deallocation_due(engine: &engine::InMemoryEngine, id: &str) -> Instant {
    let store = wsinst::state::service_state(engine).expect("the service's own store");
    let due = store
        .lock()
        .instances
        .get(id)
        .unwrap_or_else(|| panic!("the delete must have left the record for {id:?} stored"))
        .deallocation_due;
    due.unwrap_or_else(|| panic!("the delete must have stored a teardown deadline for {id:?}"))
}

/// `GetWorkspaceInstance`, as the (HTTP status, body) pair.
async fn get(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await
}

/// Sleep until `deadline` has passed, with [`SLACK`] to spare.
async fn sleep_past(deadline: Instant) {
    tokio::time::sleep(deadline.saturating_duration_since(Instant::now()) + SLACK).await;
}

/// Fail unless the reads just made all finished before `deadline`, so the assertions
/// about the state inside that window really were made inside it.
///
/// The windows here are the chosen `DEALLOCATION_SPAN` / `DEALLOCATED_RETENTION`
/// (500 ms each) and the reads are in-process calls, so this holds with a vast margin;
/// a machine stalled long enough to miss it FAILS the test rather than silently
/// skipping the assertions and passing against a synchronous delete.
fn still_inside(case: &str, phase: &str, deadline: Instant) {
    let now = Instant::now();
    assert!(
        now < deadline,
        "case {case:?}: the test's own reads for {phase} took {:?} longer than the window they had to land in, so nothing about that state could be asserted — rerun on a less loaded machine",
        now.saturating_duration_since(deadline)
    );
}

#[tokio::test]
async fn delete_starts_a_teardown_the_clock_carries_through_to_not_found() {
    for DeleteRow {
        case,
        body,
        settle_first,
    } in delete_rows()
    {
        let (router, engine) = harness();
        let id = create(&router, body).await;
        // A second instance nobody deletes: every "no longer reported" assertion below
        // has to be about the deleted one, not about an empty store.
        let survivor = create(&router, minimal_create()).await;
        if settle_first {
            tokio::time::sleep(wsinst::state::ALLOCATION_SPAN + SLACK).await;
            let state = provision_state(&router, &id).await;
            assert_eq!(
                state, "ALLOCATED",
                "case {case:?}: expected the instance to be ALLOCATED before the delete, got {state:?}"
            );
        }

        // 1. The delete is ACCEPTED and the teardown it starts is never instantaneous.
        let before_delete = Instant::now();
        ok(
            &router,
            "DeleteWorkspaceInstance",
            json!({ "WorkspaceInstanceId": id }),
        )
        .await;
        let due = deallocation_due(&engine, &id);
        assert!(
            due >= before_delete + wsinst::state::DEALLOCATION_SPAN,
            "case {case:?}: expected the DEALLOCATED move to fall due no sooner than one span after the delete, got a deadline {:?} earlier",
            (before_delete + wsinst::state::DEALLOCATION_SPAN).saturating_duration_since(due)
        );

        // 2. While the teardown runs the instance is still reported, in the transient
        //    DEALLOCATING, and the `ProvisionStates` filter selects it on that state.
        //    `still_inside` proves the reads landed inside the teardown window, so
        //    these assertions are made unconditionally.
        let (status, payload) = get(&router, &id).await;
        let reported = payload["ProvisionState"].as_str().unwrap_or_default().to_string();
        let listed = list_all_ids(&router, json!({})).await;
        let filtered = list_all_ids(&router, json!({ "ProvisionStates": ["DEALLOCATING"] })).await;
        let allocated_filter =
            list_all_ids(&router, json!({ "ProvisionStates": ["ALLOCATING", "ALLOCATED"] })).await;
        still_inside(case, "the teardown window", due);
        assert_eq!(
            status, 200,
            "case {case:?}: expected the instance to still be reported while it tears down, got {status} with {payload}"
        );
        assert_eq!(
            reported, "DEALLOCATING",
            "case {case:?}: expected the transient DEALLOCATING right after the delete, got {reported:?}"
        );
        assert!(
            listed.contains(&id),
            "case {case:?}: expected the unfiltered list to still carry {id:?}, got {listed:?}"
        );
        assert_eq!(
            filtered,
            vec![id.clone()],
            "case {case:?}: expected ProvisionStates=[DEALLOCATING] to select only {id:?}, got {filtered:?}"
        );
        assert_eq!(
            allocated_filter,
            vec![survivor.clone()],
            "case {case:?}: expected the allocation-state filter to select only the instance nobody deleted, got {allocated_filter:?}"
        );

        // 3. Once the teardown has fallen due the instance is reported in the terminal
        //    DEALLOCATED, and the filter follows it there.
        sleep_past(due).await;
        let (status, payload) = get(&router, &id).await;
        let reported = payload["ProvisionState"].as_str().unwrap_or_default().to_string();
        let filtered = list_all_ids(&router, json!({ "ProvisionStates": ["DEALLOCATED"] })).await;
        let still_deallocating =
            list_all_ids(&router, json!({ "ProvisionStates": ["DEALLOCATING"] })).await;
        still_inside(
            case,
            "the retention window",
            due + wsinst::state::DEALLOCATED_RETENTION,
        );
        assert_eq!(
            status, 200,
            "case {case:?}: expected the torn-down instance to still be reported inside its retention, got {status} with {payload}"
        );
        assert_eq!(
            reported, "DEALLOCATED",
            "case {case:?}: expected the terminal DEALLOCATED once the teardown fell due, got {reported:?}"
        );
        assert_eq!(
            filtered,
            vec![id.clone()],
            "case {case:?}: expected ProvisionStates=[DEALLOCATED] to select only {id:?}, got {filtered:?}"
        );
        assert!(
            !still_deallocating.contains(&id),
            "case {case:?}: expected {id:?} to have left DEALLOCATING, got {still_deallocating:?}"
        );

        // 4. Past the retention the instance stops being reported at all: every
        //    reader answers the modelled not-found, no filter can select it, and the
        //    instance nobody deleted is untouched.
        tokio::time::sleep(
            (due + wsinst::state::DEALLOCATED_RETENTION)
                .saturating_duration_since(Instant::now())
                + GENEROUS_SLACK,
        )
        .await;
        for operation in [
            "GetWorkspaceInstance",
            "ListTagsForResource",
            "DeleteWorkspaceInstance",
        ] {
            let (status, payload) =
                call(&router, operation, json!({ "WorkspaceInstanceId": id })).await;
            // This service answers `ResourceNotFoundException` with HTTP 400, not the
            // 404 its shape declares — see `state::RESOURCE_NOT_FOUND_STATUS`.
            assert_eq!(
                status,
                wsinst::state::RESOURCE_NOT_FOUND_STATUS,
                "case {case:?}: expected {operation} to answer HTTP {} past the retention, got {status} with {payload}",
                wsinst::state::RESOURCE_NOT_FOUND_STATUS
            );
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {case:?}: expected {operation} to answer ResourceNotFoundException past the retention, got {payload}"
            );
        }
        let listed = list_all_ids(&router, json!({})).await;
        assert_eq!(
            listed,
            vec![survivor.clone()],
            "case {case:?}: expected only the instance nobody deleted, got {listed:?}"
        );
        for state in EVERY_PROVISION_STATE {
            let selected = list_all_ids(&router, json!({ "ProvisionStates": [state] })).await;
            assert!(
                !selected.contains(&id),
                "case {case:?}: expected no ProvisionStates filter to select the forgotten {id:?}, but [{state}] did: {selected:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_repeated_delete_is_accepted_and_does_not_push_the_teardown_out() {
    // Delete is idempotent in effect: a repeat call names the end state the instance
    // is already heading to, so it is accepted and leaves the teardown deadline where
    // the FIRST delete put it — repeated calls cannot keep a resource alive.
    let (router, engine) = harness();
    let id = create(&router, minimal_create()).await;

    ok(
        &router,
        "DeleteWorkspaceInstance",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    let first_due = deallocation_due(&engine, &id);

    for repeat in 2..=4 {
        let (status, payload) = call(
            &router,
            "DeleteWorkspaceInstance",
            json!({ "WorkspaceInstanceId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case delete number {repeat}: expected HTTP 200 for an instance already tearing down, got {status} with {payload}"
        );
        let due = deallocation_due(&engine, &id);
        assert_eq!(
            due, first_due,
            "case delete number {repeat}: expected the teardown deadline to stay where the first delete put it, got a deadline moved by {:?}",
            due.saturating_duration_since(first_due)
        );
    }

    // A repeat once the teardown has FINISHED does not move the record back to the
    // transient state either: the instance is already where the delete asks for.
    sleep_past(first_due).await;
    let (status, payload) = call(
        &router,
        "DeleteWorkspaceInstance",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete a torn-down instance inside its retention: expected HTTP 200, got {status} with {payload}"
    );
    let (status, payload) = get(&router, &id).await;
    let reported = payload["ProvisionState"].as_str().unwrap_or_default().to_string();
    still_inside(
        "a repeat inside the retention",
        "the retention window",
        first_due + wsinst::state::DEALLOCATED_RETENTION,
    );
    assert_eq!(
        (status, reported.as_str()),
        (200, "DEALLOCATED"),
        "case delete a torn-down instance inside its retention: expected it to stay DEALLOCATED, got {status} with {payload}"
    );

    // And the clock still finishes the teardown it started.
    tokio::time::sleep(
        (first_due + wsinst::state::DEALLOCATED_RETENTION)
            .saturating_duration_since(Instant::now())
            + GENEROUS_SLACK,
    )
    .await;
    let (status, payload) = get(&router, &id).await;
    assert_eq!(
        status,
        wsinst::state::RESOURCE_NOT_FOUND_STATUS,
        "case read past the retention: expected HTTP {}, got {status} with {payload}",
        wsinst::state::RESOURCE_NOT_FOUND_STATUS
    );
}

#[tokio::test]
async fn an_instance_tearing_down_no_longer_holds_a_running_instance_slot() {
    // The quota this emulator enforces counts RUNNING instances (a chosen semantic,
    // documented on `state::MAX_INSTANCES`): a delete frees the slot as soon as it is
    // accepted, while the record it leaves behind is still reported.
    let (router, engine) = harness();
    let id = create(&router, minimal_create()).await;

    ok(
        &router,
        "DeleteWorkspaceInstance",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;

    let store = wsinst::state::service_state(&*engine).expect("the service's own store");
    let (held, running) = {
        let state = store.lock();
        (state.instances.len(), state.running_instance_count())
    };
    assert_eq!(
        (held, running),
        (1, 0),
        "case right after the delete: expected the record to be held but not counted as running, got {held} held and {running} running"
    );

    // The record is still reported for the caller, which is the point: the slot is
    // free and the instance is visible.
    let state = provision_state(&router, &id).await;
    assert_eq!(
        state, "DEALLOCATING",
        "case right after the delete: expected the record to still report DEALLOCATING, got {state:?}"
    );
}

#[tokio::test]
async fn a_tag_write_and_a_tag_read_both_still_reach_an_instance_tearing_down() {
    // A resource that is tearing down still EXISTS, so the operations that resolve an
    // instance id keep resolving it — they do not answer not-found early.
    let (router, _engine) = harness();
    let id = create(
        &router,
        json!({ "ManagedInstance": {}, "Tags": [{ "Key": "env", "Value": "dev" }] }),
    )
    .await;

    ok(
        &router,
        "DeleteWorkspaceInstance",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;

    let tags = ok(
        &router,
        "ListTagsForResource",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    assert_eq!(
        tags["Tags"][0]["Key"].as_str(),
        Some("env"),
        "case read the tags of an instance tearing down: expected the stored tag, got {tags}"
    );

    let (status, payload) = call(
        &router,
        "TagResource",
        json!({ "WorkspaceInstanceId": id, "Tags": [{ "Key": "owner", "Value": "alice" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case tag an instance tearing down: expected HTTP 200, got {status} with {payload}"
    );
    let tags = ok(
        &router,
        "ListTagsForResource",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    let keys: Vec<&str> = tags["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("ListTagsForResource returned no Tags: {tags}"))
        .iter()
        .map(|tag| tag["Key"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        keys,
        vec!["env", "owner"],
        "case tag an instance tearing down: expected the merged tag set, got {keys:?}"
    );
}

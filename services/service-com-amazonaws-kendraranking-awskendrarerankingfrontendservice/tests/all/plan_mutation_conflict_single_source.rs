// SPDX-License-Identifier: CC0-1.0

//! The "a plan may only be mutated while ACTIVE" refusal, asserted to be ONE rule rather
//! than one per operation.
//!
//! Every operation that MUTATES a rescore execution plan settles the status the clock
//! already owes it and, when that status is not `ACTIVE`, refuses the call with the
//! modelled `ConflictException` carrying the same sentence — naming the plan, the status
//! that refuses the call, and the only status in which it would have been served. The
//! sentence differs between operations in exactly one word: what the caller asked to do
//! to the plan.
//!
//! This suite drives that shared path through each of the two mutating operations, in
//! every non-terminal status a plan can be caught in, and asserts their refusals agree
//! with each other and with `state::plan_mutation_conflict_message`, so a change made in
//! one place can no longer leave the other stating something else. It also drives the
//! served side of the same path, so centralising the decision did not change a call that
//! is allowed.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use super::harness::{
    call, create_plan, emulator, error_code, error_message, sleep_past_the_transition,
};

/// Every operation that mutates a plan addressed by its `Id`, with a request body
/// against `id` that reaches the status decision, and the word the refusal names that
/// mutation with: one row per handler that used to make this decision itself.
fn mutating_calls(id: &str) -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "DeleteRescoreExecutionPlan",
            json!({ "Id": id }),
            state::MUTATION_DELETED,
        ),
        (
            "UpdateRescoreExecutionPlan",
            json!({ "Id": id, "Name": "the-rename-that-is-refused" }),
            state::MUTATION_UPDATED,
        ),
    ]
}

/// The status `DescribeRescoreExecutionPlan` reports for `id`.
async fn reported_status(router: &MultiServiceRouter, id: &str) -> String {
    let (status, described) = call(router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 200, "describe {id}: got {status} {described}");
    described["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("describe {id}: no Status in {described}"))
        .to_string()
}

#[tokio::test]
async fn every_mutating_operation_states_the_same_mid_transition_refusal() {
    // The non-terminal statuses a plan can be caught in, each reached through the call
    // that puts it there: the rule generalises over the STATUS that refuses the
    // mutation, so the rows differ in that status rather than in a name. The status
    // values themselves are the ones `RescoreExecutionPlanStatus` models (see
    // `state::STATUS_CREATING` / `STATUS_UPDATING` / `STATUS_DELETING`).
    let rows: Vec<(&str, &str)> = vec![
        ("a plan the create is still provisioning", state::STATUS_CREATING),
        ("a plan an update is still re-provisioning", state::STATUS_UPDATING),
        ("a plan a delete is already tearing down", state::STATUS_DELETING),
    ];

    for (case, expected_status) in rows {
        // One emulator per row, so a row's plan is the only one in the store and the
        // transient window it opens is not shared with another row's timing.
        let router = emulator();
        let id = create_plan(&router, "mutation-conflict").await;
        if expected_status != state::STATUS_CREATING {
            // `CREATING` is the status a create leaves behind; the other two are opened
            // by a mutating call on a plan that has settled into `ACTIVE`.
            sleep_past_the_transition();
            let opener = if expected_status == state::STATUS_UPDATING {
                (
                    "UpdateRescoreExecutionPlan",
                    json!({ "Id": id, "Description": "the update that opens the window" }),
                )
            } else {
                ("DeleteRescoreExecutionPlan", json!({ "Id": id }))
            };
            let (status, answer) = call(&router, opener.0, opener.1).await;
            assert_eq!(
                status, 200,
                "case {case:?}: opening the window with {} got {status} {answer}",
                opener.0
            );
        }

        // The status the service itself reports for the plan right now, so the sentence
        // the refusal must carry is built from the service's state rather than assumed.
        let observed = reported_status(&router, &id).await;
        assert_eq!(
            observed, expected_status,
            "case {case:?}: expected the plan to report {expected_status:?}, got {observed:?}"
        );

        let mut refusals: Vec<(&'static str, String)> = Vec::new();
        for (operation, body, verb) in mutating_calls(&id) {
            // What the crate's single construction site says for this plan and status.
            // Every mutating operation is held to it, so the handlers cannot drift
            // apart — and the sentence that site builds is itself held to the service's
            // wording by `the_conflict_sentence_is_the_service_s_own_wording` below, so
            // a rewording cannot pass by changing both sides of this comparison.
            let expected = state::plan_mutation_conflict_message(&id, expected_status, verb);
            let (status, answer) = call(&router, operation, body).await;
            // This RPC service answers every client-side failure with the protocol's
            // client-error status; the error TYPE says which failure it was.
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {answer}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ConflictException",
                "case {case:?} on {operation}: expected ConflictException, got {code:?} in {answer}"
            );
            let message = error_message(&answer);
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {message:?}"
            );
            refusals.push((operation, message));
        }

        // The two operations' sentences are the SAME rule: replacing each one's own word
        // for the mutation with the other's makes them identical.
        let [(first_op, first), (second_op, second)] = refusals.as_slice() else {
            panic!("case {case:?}: expected one refusal per mutating operation, got {refusals:?}");
        };
        let aligned = first.replace(state::MUTATION_DELETED, state::MUTATION_UPDATED);
        assert_eq!(
            &aligned, second,
            "case {case:?}: {first_op} and {second_op} must state one rule, got {first:?} \
             and {second:?}"
        );

        // A refused mutation wrote nothing: the plan keeps the status it was caught in
        // and the name it was created with, so neither handler half-applied its change
        // before the shared decision refused it.
        let after = reported_status(&router, &id).await;
        assert_eq!(
            after, expected_status,
            "case {case:?}: a refused mutation must leave the status alone, got {after:?}"
        );
        let (status, described) =
            call(&router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
        assert_eq!(status, 200, "case {case:?}: describe got {status} {described}");
        assert_eq!(
            described["Name"].as_str().unwrap_or_default(),
            "mutation-conflict",
            "case {case:?}: a refused update must not rename the plan, got {described}"
        );
    }
}

#[tokio::test]
async fn the_conflict_sentence_is_the_service_s_own_wording() {
    let router = emulator();

    // The wording itself, over the wire, independent of the constructor the handlers
    // call: the sentence names the plan, the status that refuses the call and the status
    // the call needs. Spelled out literally here so a rewording of the single
    // construction site fails this suite instead of silently changing both sides of the
    // agreement assertion above.
    let id = create_plan(&router, "conflict-wording").await;
    for (operation, body, verb) in mutating_calls(&id) {
        let (_, answer) = call(&router, operation, body).await;
        let message = error_message(&answer);
        let expected = format!(
            "Rescore execution plan {id} is CREATING and can only be {verb} while it is ACTIVE."
        );
        assert_eq!(
            message, expected,
            "{operation} on a CREATING plan: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_active_plan_is_served_by_every_mutating_operation() {
    // The other side of the shared decision: a plan that IS `ACTIVE` reaches each
    // handler's own work, so centralising the refusal did not change a served call.
    for (case, operation, mutate) in [
        (
            "the update of an ACTIVE plan",
            "UpdateRescoreExecutionPlan",
            true,
        ),
        (
            "the delete of an ACTIVE plan",
            "DeleteRescoreExecutionPlan",
            false,
        ),
    ] {
        let router = emulator();
        let id = create_plan(&router, "mutation-served").await;
        sleep_past_the_transition();
        let body = if mutate {
            json!({ "Id": id, "Name": "served-rename" })
        } else {
            json!({ "Id": id })
        };
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {answer}"
        );

        // The call did its own work: the update renamed the plan and moved it into the
        // transient re-provisioning status, the delete moved it into teardown.
        let reported = reported_status(&router, &id).await;
        let expected_status = if mutate {
            state::STATUS_UPDATING
        } else {
            state::STATUS_DELETING
        };
        assert_eq!(
            reported, expected_status,
            "case {case:?}: expected the plan to report {expected_status:?}, got {reported:?}"
        );
        if mutate {
            let (status, described) =
                call(&router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
            assert_eq!(status, 200, "case {case:?}: describe got {status} {described}");
            assert_eq!(
                described["Name"].as_str().unwrap_or_default(),
                "served-rename",
                "case {case:?}: the served update must have renamed the plan, got {described}"
            );
        }
    }
}

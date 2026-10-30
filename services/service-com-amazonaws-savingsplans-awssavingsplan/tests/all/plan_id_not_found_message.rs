// SPDX-License-Identifier: CC0-1.0

//! The sentence this service answers with when an operation is pointed at a
//! Savings Plan id this store does not hold.
//!
//! The rule: every operation whose input names a plan by `savingsPlanId`
//! answers an id the store does not hold with a 404 `ResourceNotFoundException`
//! whose message is `SavingsPlan can't be found. Savings Plan Id: <id>.` — the
//! fixed text, the `savingsPlanId` taken verbatim from the request, and a
//! terminating period. The id's spelling does not enter the sentence's form and
//! does not change the answer: not being in the store is what makes it a 404.
//!
//! The table below spans both axes the rule generalises over — WHICH operation
//! is pointed at the plan, and WHAT the id looks like — so each row is one
//! (operation, id) pair rather than the one recorded request.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots,
//! reusing the harness in [`crate::savings_plans_lifecycle`].

use serde_json::json;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// Now, in whole seconds since the epoch: what a `purchaseTime` carries.
fn now_seconds() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after 1970")
            .as_secs(),
    )
    .expect("a clock this century")
}

/// The whole message the service owes for an id it does not hold, spelled out
/// here rather than taken from the crate so the test pins the contract instead
/// of whatever the implementation currently formats.
fn expected_message(savings_plan_id: &str) -> String {
    format!("SavingsPlan can't be found. Savings Plan Id: {savings_plan_id}.")
}

/// Every operation pointed at a plan by `savingsPlanId` refuses an id this
/// store does not hold with the same status, the same code and the same
/// sentence, built from the id the caller sent.
#[tokio::test]
async fn a_plan_id_the_store_does_not_hold_is_refused_with_aws_s_sentence() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    // One plan this store really does hold, so every row below is refused for
    // the id it names and not because the store is empty.
    let mine = emulator
        .purchase(json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.1",
            "purchaseTime": now_seconds() + 86_400,
        }))
        .await;
    let arn = text(&emulator.plan(&mine).await, "savingsPlanArn");

    // Every operation whose input names a plan by `savingsPlanId`
    // (OPERATIONS.md: DeleteQueuedSavingsPlan, DescribeSavingsPlanRates,
    // ReturnSavingsPlan). `DescribeSavingsPlans` takes a LIST of ids and
    // filters by it, so an unknown id there is an empty page, not a refusal.
    let operations = [
        "DeleteQueuedSavingsPlan",
        "DescribeSavingsPlanRates",
        "ReturnSavingsPlan",
    ];

    // (case, the id to point the operation at)
    let ids: [(&str, String); 6] = [
        (
            "a UUID minted by another account",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("a near miss on one of ours", format!("{mine}-not-mine")),
        ("the same id upper-cased", mine.to_uppercase()),
        ("an ARN where an id belongs", arn.clone()),
        ("a long opaque value", "z".repeat(64)),
        ("not an id at all", "not a savings plan".to_string()),
    ];

    for operation in operations {
        for (case, savings_plan_id) in &ids {
            // A row only pins the refusal for an id this store does not hold.
            assert_ne!(
                savings_plan_id, &mine,
                "case {case:?} on {operation}: the row must name an id this store does not hold"
            );
            let answer = emulator
                .call(operation, json!({ "savingsPlanId": savings_plan_id }))
                .await;
            assert_eq!(
                (answer.status, answer.code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} on {operation}: expected a 404 ResourceNotFoundException, got {} \
                 {:?}",
                answer.status,
                answer.body
            );
            let expected = expected_message(savings_plan_id);
            assert_eq!(
                text(&answer.body, "message"),
                expected,
                "case {case:?} on {operation}: expected {expected:?}, got {:?}",
                text(&answer.body, "message")
            );
        }
    }

    // None of those refusals touched the plan that IS in the store: its queued
    // purchase is still there to cancel, and the cancellation reads back.
    let answer = emulator
        .call("DeleteQueuedSavingsPlan", json!({ "savingsPlanId": mine }))
        .await;
    assert_eq!(
        answer.status, 200,
        "the stored plan must still be cancellable, got {} {:?}",
        answer.status, answer.body
    );
    let state_after = text(&emulator.plan(&mine).await, "state");
    assert_eq!(
        state_after,
        service_com_amazonaws_savingsplans_awssavingsplan::state::STATE_QUEUED_DELETED,
        "an accepted cancellation must read back as queued-deleted, got {state_after:?}"
    );
}

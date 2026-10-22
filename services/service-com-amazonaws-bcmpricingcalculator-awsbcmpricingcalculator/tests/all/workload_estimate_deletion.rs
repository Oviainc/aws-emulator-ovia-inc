// SPDX-License-Identifier: CC0-1.0

//! `DeleteWorkloadEstimate`, driven end-to-end over this service's awsJson1_0 wire
//! (`POST /`, `X-Amz-Target: AWSBCMPricingCalculator.<Operation>`) through the same
//! `MultiServiceRouter` the production front door uses.
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded request:
//! the rows differ along what the rule generalises over (the number of estimates in the
//! store, which one of them is deleted, how many times the delete is repeated, the kind
//! of identifier it names, and the age of the estimate).
//!
//! The wire harness is the one `bill_scenario_lifecycle` defines; a second copy could
//! drift from the front door that one exercises.

use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::{
    state, ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState as ServiceState,
};

use crate::bill_scenario_lifecycle::{error_code, Emulator};

/// Create `count` estimates, named `e0..e<count>`, and return their ids in that order.
async fn seed(emulator: &Emulator, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let (status, body) = emulator
            .call(
                "CreateWorkloadEstimate",
                json!({ "name": format!("e{index}") }),
            )
            .await;
        assert_eq!(
            status, 200,
            "seeding e{index}: expected 200, got {status} with {body}"
        );
        ids.push(
            body["id"]
                .as_str()
                .unwrap_or_else(|| panic!("seeding e{index}: no id in {body}"))
                .to_string(),
        );
    }
    ids
}

/// The ids `ListWorkloadEstimates` reports, over as many pages as it takes.
async fn listed_ids(emulator: &Emulator, case: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(token) = next.clone() {
            request["nextToken"] = json!(token);
        }
        let (status, body) = emulator.call("ListWorkloadEstimates", request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from ListWorkloadEstimates, got {status} with {body}"
        );
        for item in body["items"].as_array().cloned().unwrap_or_default() {
            ids.push(item["id"].as_str().unwrap_or_default().to_string());
        }
        next = body["nextToken"].as_str().map(str::to_string);
        if next.is_none() {
            break;
        }
    }
    ids
}

/// Whether an awsJson1_0 response body carries no members: `DeleteWorkloadEstimateResponse`
/// models none (smithy_query: `query="operation",operation="DeleteWorkloadEstimate"` has an
/// empty `output`).
fn is_empty_body(body: &Value) -> bool {
    body.as_object().is_none_or(serde_json::Map::is_empty)
}

/// RULE: a delete removes exactly the resource its `identifier` names and nothing else —
/// the reader then answers `ResourceNotFoundException` for it, the list no longer reports
/// it, and every other stored resource is untouched.
///
/// The table spans the size of the store and which position in it is deleted (first,
/// middle, last, and the only one).
#[tokio::test]
async fn deleting_an_estimate_removes_only_that_estimate() {
    // (how many estimates exist, which ones are deleted)
    let cases: [(usize, &[usize]); 6] = [
        (1, &[0]),
        (3, &[0]),
        (3, &[1]),
        (3, &[2]),
        (5, &[0, 2, 4]),
        (4, &[0, 1, 2, 3]),
    ];

    for (count, doomed) in cases {
        let case = format!("count={count} deleting={doomed:?}");
        let emulator = Emulator::new();
        let ids = seed(&emulator, count).await;

        for &index in doomed {
            let (status, body) = emulator
                .call(
                    "DeleteWorkloadEstimate",
                    json!({ "identifier": ids[index] }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case} index={index}: expected 200 from DeleteWorkloadEstimate, got {status} with {body}"
            );
            assert!(
                is_empty_body(&body),
                "case {case} index={index}: expected an empty response body, got {body}"
            );
        }

        for (index, id) in ids.iter().enumerate() {
            let deleted = doomed.contains(&index);
            let (status, body) = emulator
                .call("GetWorkloadEstimate", json!({ "identifier": id }))
                .await;
            let expected = if deleted { 404 } else { 200 };
            assert_eq!(
                status, expected,
                "case {case} index={index}: expected GetWorkloadEstimate {expected}, got {status} with {body}"
            );
            if deleted {
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case} index={index}: expected ResourceNotFoundException, got {body}"
                );
                assert_eq!(
                    body["resourceId"].as_str(),
                    Some(id.as_str()),
                    "case {case} index={index}: expected the error to name what was looked for, got {body}"
                );
                assert_eq!(
                    body["resourceType"].as_str(),
                    Some(state::WORKLOAD_ESTIMATE_RESOURCE_TYPE),
                    "case {case} index={index}: expected the error to name the resource type, got {body}"
                );
            } else {
                assert_eq!(
                    body["name"].as_str(),
                    Some(format!("e{index}").as_str()),
                    "case {case} index={index}: expected the survivor to keep its name, got {body}"
                );
            }
        }

        let survivors: Vec<String> = ids
            .iter()
            .enumerate()
            .filter(|(index, _)| !doomed.contains(index))
            .map(|(_, id)| id.clone())
            .collect();
        let listed = listed_ids(&emulator, &case).await;
        assert_eq!(
            listed, survivors,
            "case {case}: expected the list to report exactly the survivors {survivors:?}, got {listed:?}"
        );

        let stored = emulator
            .engine
            .state::<ServiceState>()
            .lock()
            .workload_estimates
            .len();
        assert_eq!(
            stored,
            survivors.len(),
            "case {case}: expected {} estimate(s) left in the store, got {stored}",
            survivors.len()
        );
    }
}

/// RULE: the delete is `@idempotent` and models no not-found error, so naming an
/// identifier the store does not hold is an empty success, not a failure — whether it was
/// deleted a moment ago, never existed, or belongs to a different kind of resource — and
/// such a call changes nothing.
///
/// The table spans how many times the delete is repeated and which kind of identifier it
/// names.
#[tokio::test]
async fn deleting_an_absent_estimate_is_an_empty_success() {
    for repeats in [1_usize, 2, 5] {
        let emulator = Emulator::new();
        let ids = seed(&emulator, 2).await;

        // A neighbouring resource of the OTHER root kind: a workload-estimate delete must
        // not resolve a bill scenario's id.
        let (status, body) = emulator
            .call("CreateBillScenario", json!({ "name": "neighbour" }))
            .await;
        assert_eq!(
            status, 200,
            "repeats={repeats}: expected 200 creating the neighbouring scenario, got {status} with {body}"
        );
        let scenario = body["id"].as_str().unwrap_or_default().to_string();

        // Delete one estimate, so one of the rows below is an identifier that existed.
        let (status, body) = emulator
            .call("DeleteWorkloadEstimate", json!({ "identifier": ids[0] }))
            .await;
        assert_eq!(
            status, 200,
            "repeats={repeats}: expected 200 from the first delete, got {status} with {body}"
        );

        let absent: [(&str, &str); 5] = [
            ("already-deleted", ids[0].as_str()),
            ("all-zero", "00000000-0000-0000-0000-000000000000"),
            ("all-f", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
            ("mixed-hex", "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d"),
            ("other-resource-kind", scenario.as_str()),
        ];

        for (label, identifier) in absent {
            for attempt in 0..repeats {
                let case = format!("repeats={repeats} id={label} attempt={attempt}");
                let (status, body) = emulator
                    .call(
                        "DeleteWorkloadEstimate",
                        json!({ "identifier": identifier }),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case}: expected an idempotent delete to answer 200, got {status} with {body}"
                );
                assert!(
                    is_empty_body(&body),
                    "case {case}: expected an empty response body, got {body}"
                );
            }
        }

        // The surviving estimate and the neighbouring scenario are both untouched.
        let (status, body) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": ids[1] }))
            .await;
        assert_eq!(
            status, 200,
            "repeats={repeats}: expected the untouched estimate to survive, got {status} with {body}"
        );
        let (status, body) = emulator
            .call("GetBillScenario", json!({ "identifier": scenario }))
            .await;
        assert_eq!(
            status, 200,
            "repeats={repeats}: expected the neighbouring scenario to survive a workload-estimate delete, got {status} with {body}"
        );
    }
}

/// RULE: the idempotency record a create made does not outlive the resource it names, so
/// once an estimate is deleted its `clientToken` no longer replays and the same token
/// mints a fresh estimate instead of answering with the vanished one.
///
/// The table spans the token's shape (`ClientToken` is `len 1..64`, pattern
/// `^[\u{21}-\u{7e}]+$`).
#[tokio::test]
async fn deleting_an_estimate_retires_the_token_that_made_it() {
    let tokens = [
        "t",
        "plain-token-1",
        "~!@#$%^&*()_+punctuation",
        "t234567890123456789012345678901234567890123456789012345678901234",
    ];

    for token in tokens {
        let emulator = Emulator::new();
        let request = json!({ "name": "retired", "clientToken": token });

        let (status, first) = emulator
            .call("CreateWorkloadEstimate", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case token={token}: expected 200 from the create, got {status} with {first}"
        );
        let first_id = first["id"].as_str().unwrap_or_default().to_string();

        let (status, body) = emulator
            .call("DeleteWorkloadEstimate", json!({ "identifier": first_id }))
            .await;
        assert_eq!(
            status, 200,
            "case token={token}: expected 200 from the delete, got {status} with {body}"
        );

        let tracked = emulator
            .engine
            .state::<ServiceState>()
            .lock()
            .idempotency
            .len();
        assert_eq!(
            tracked, 0,
            "case token={token}: expected the delete to retire the idempotency record, got {tracked}"
        );

        let (status, second) = emulator.call("CreateWorkloadEstimate", request).await;
        assert_eq!(
            status, 200,
            "case token={token}: expected the re-used token to create afresh, got {status} with {second}"
        );
        let second_id = second["id"].as_str().unwrap_or_default().to_string();
        assert_ne!(
            second_id, first_id,
            "case token={token}: expected a fresh estimate rather than the deleted {first_id}, got {second_id}"
        );

        let (status, body) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": first_id }))
            .await;
        assert_eq!(
            status, 404,
            "case token={token}: expected the deleted estimate to stay deleted, got {status} with {body}"
        );
        let (status, body) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": second_id }))
            .await;
        assert_eq!(
            status, 200,
            "case token={token}: expected the fresh estimate to be readable, got {status} with {body}"
        );
    }
}

/// RULE: a delete resolves its identifier against the state the CLOCK has already made
/// due, so an estimate past the `expiresAt` it advertised is gone either way — the delete
/// is still an empty success and never resurrects it — while an estimate still inside its
/// window is removed by the delete and only by the delete.
///
/// The table spans the age of the estimate on each side of the boundary instant. The store
/// is aged directly, the way the clock would: the handlers store the due instant and never
/// sleep for it.
#[tokio::test]
async fn deleting_an_expired_estimate_is_still_a_success() {
    // (seconds the advertised expiry sits from now, whether it was live at delete time)
    let cases: [(i64, bool); 5] = [
        (-1, false),
        (0, false),
        (-7 * 24 * 60 * 60, false),
        (60, true),
        (state::WORKLOAD_ESTIMATE_RETENTION_SECS, true),
    ];

    for (offset, expected_live) in cases {
        let case = format!("expiresAt = now{offset:+}");
        let emulator = Emulator::new();
        let ids = seed(&emulator, 2).await;
        let aged = ids[0].clone();
        let untouched = ids[1].clone();

        {
            let store = emulator.engine.state::<ServiceState>();
            let mut guard = store.lock();
            let estimate = guard
                .workload_estimate_mut(&aged)
                .unwrap_or_else(|| panic!("case {case}: {aged} was not stored"));
            estimate.expires_at = aws_smithy_types::DateTime::from_secs(state::now_secs() + offset);
        }

        // Whether the clock already removed it or the delete does, the answer is the same
        // empty success.
        let (status, body) = emulator
            .call("DeleteWorkloadEstimate", json!({ "identifier": aged }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the delete, got {status} with {body}"
        );
        assert!(
            is_empty_body(&body),
            "case {case}: expected an empty response body, got {body}"
        );

        let (status, body) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": aged }))
            .await;
        assert_eq!(
            status, 404,
            "case {case}: expected the deleted estimate to be gone (it was live={expected_live}), got {status} with {body}"
        );

        let (status, body) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": untouched }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the in-window estimate to survive, got {status} with {body}"
        );

        let listed = listed_ids(&emulator, &case).await;
        assert_eq!(
            listed,
            vec![untouched.clone()],
            "case {case}: expected the list to report only the survivor, got {listed:?}"
        );
    }
}

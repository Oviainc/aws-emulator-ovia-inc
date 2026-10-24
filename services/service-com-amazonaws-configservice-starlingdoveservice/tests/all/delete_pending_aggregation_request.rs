// SPDX-License-Identifier: CC0-1.0

//! What `DeletePendingAggregationRequest` does to state, end to end over the wire.
//!
//! The rule under test: the (requester account, requester Region) pair the request
//! names identifies ONE pending aggregation request, so the delete removes exactly that
//! entry and leaves every other one reported; the operation models no not-found error
//! (`smithy_query(query="operation", operation="DeletePendingAggregationRequest")`
//! declares only `InvalidParameterValueException`), so a pair holding no pending
//! request succeeds and changes nothing, which makes the delete idempotent; a pair
//! outside the modelled constraints on `RequesterAccountId` (`^\d{12}$`) and
//! `RequesterAwsRegion` (length 1..64) is refused and persists nothing.
//!
//! No operation of this API mints a pending aggregation request — the requester is
//! another account's aggregator, whose `PutConfigurationAggregator` reaches that
//! account's Config — so each case seeds the inventory in the shared engine the router
//! hands the handlers (the `seed` helper `pending_aggregation_requests` already uses)
//! and then drives the service the way a client does: an `awsJson1_1` `POST /` through
//! the same `MultiServiceRouter` the emulator binary mounts. What the store holds is
//! read back through `DescribePendingAggregationRequests`.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::Emulator;
use crate::pending_aggregation_requests::seed;

/// Twelve-digit account ids standing for aggregator accounts, deliberately NOT in
/// ascending order so a reported ordering can only come from the store
/// (`RequesterAccountId`, pattern `^\d{12}$`).
const ACCOUNT_A: &str = "210987654321";
/// See [`ACCOUNT_A`].
const ACCOUNT_B: &str = "333344445555";
/// See [`ACCOUNT_A`].
const ACCOUNT_C: &str = "999988887777";

/// The maximum length `RequesterAwsRegion` allows
/// (`smithy_query(query="operation", operation="DeletePendingAggregationRequest")`:
/// length 1..64).
const AWS_REGION_MAX_LENGTH: usize = 64;

/// One pending aggregation request, as a (account, Region) pair.
fn pair(account: &str, region: &str) -> (String, String) {
    (account.to_string(), region.to_string())
}

/// The pending requests the seeded store holds before any delete: several accounts in
/// several Regions, inserted out of ascending order, one account asking from two
/// Regions, one Region asked for by two accounts, and the longest Region the model
/// allows.
fn seeded() -> Vec<(String, String)> {
    vec![
        pair(ACCOUNT_C, "us-east-2"),
        pair(ACCOUNT_A, "us-east-1"),
        pair(ACCOUNT_B, "eu-west-3"),
        pair(ACCOUNT_A, "ap-southeast-2"),
        pair(ACCOUNT_B, "us-east-1"),
        pair(ACCOUNT_C, &"r".repeat(AWS_REGION_MAX_LENGTH)),
    ]
}

/// The pending requests the service reports, as (account, Region) pairs in the order
/// reported. `Limit` is left out, so one page carries the operation's maximum of 20 —
/// more than any case here seeds.
async fn reported(case: &str, emulator: &Emulator) -> Vec<(String, String)> {
    let body = emulator
        .ok("DescribePendingAggregationRequests", &json!({}))
        .await;
    assert!(
        body.get("NextToken").is_none(),
        "case {case:?}: these cases fit one page, got {body}"
    );
    body["PendingAggregationRequests"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a PendingAggregationRequests list, got {body}")
        })
        .iter()
        .map(|request| {
            let account = request["RequesterAccountId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an account, got {request}"))
                .to_string();
            let region = request["RequesterAwsRegion"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Region, got {request}"))
                .to_string();
            (account, region)
        })
        .collect()
}

/// `pairs` in the (account, Region) ordering the store keys them by, which is the order
/// they are reported in.
fn in_key_order(pairs: &[(String, String)]) -> Vec<(String, String)> {
    let mut sorted = pairs.to_vec();
    sorted.sort();
    sorted
}

/// The request body naming `account` and `region` as the requesting pair.
fn delete_request(account: &str, region: &str) -> Value {
    json!({ "RequesterAccountId": account, "RequesterAwsRegion": region })
}

#[tokio::test]
async fn a_delete_removes_exactly_the_pair_it_names() {
    // Rows differ along WHICH pair of the seeded store is named: the first and the last
    // in key order, one of an account that asks from two Regions, one of a Region two
    // accounts ask for, the longest Region the model allows, and — the row the rule
    // generalises to — a pair the store holds no request for, both an account it knows
    // from another Region and an account it has never seen.
    let targets: Vec<(&str, (String, String))> = vec![
        ("the first pair in key order", pair(ACCOUNT_A, "ap-southeast-2")),
        (
            "the last pair in key order",
            pair(ACCOUNT_C, &"r".repeat(AWS_REGION_MAX_LENGTH)),
        ),
        (
            "one Region of an account that asks from two",
            pair(ACCOUNT_A, "us-east-1"),
        ),
        (
            "one account of a Region two accounts ask for",
            pair(ACCOUNT_B, "us-east-1"),
        ),
        ("a middle pair", pair(ACCOUNT_B, "eu-west-3")),
        (
            "a known account in a Region it did not ask from",
            pair(ACCOUNT_A, "eu-central-1"),
        ),
        ("an account the store has never seen", pair("000011112222", "us-west-2")),
    ];

    for (case, target) in targets {
        let emulator = Emulator::new();
        let requests = seeded();
        seed(&emulator, &requests);

        let body = emulator
            .ok("DeletePendingAggregationRequest", &delete_request(&target.0, &target.1))
            .await;
        assert!(
            body.is_null() || body.as_object().is_some_and(|fields| fields.is_empty()),
            "case {case:?}: the delete models no output members, got {body}"
        );

        let want: Vec<(String, String)> = in_key_order(
            &requests
                .iter()
                .filter(|held| **held != target)
                .cloned()
                .collect::<Vec<_>>(),
        );
        let got = reported(case, &emulator).await;
        assert_eq!(
            got, want,
            "case {case:?}: expected exactly {target:?} removed and the other {} \
             requests still reported, got {got:?}",
            want.len()
        );

        // Naming the same pair again changes nothing: the model declares no not-found
        // error, so the second delete succeeds and the store stays as it is.
        emulator
            .ok("DeletePendingAggregationRequest", &delete_request(&target.0, &target.1))
            .await;
        let again = reported(case, &emulator).await;
        assert_eq!(
            again, want,
            "case {case:?}: a repeated delete of {target:?} must change nothing, got {again:?}"
        );
    }
}

#[tokio::test]
async fn deleting_every_pair_empties_the_store() {
    // Rows differ along the ORDER the seeded pairs are deleted in: key order, its
    // reverse, and the insertion order — the set left behind after each step must be
    // the same whichever order the deletes come in.
    let requests = seeded();
    let orders: [(&str, Vec<(String, String)>); 3] = [
        ("in key order", in_key_order(&requests)),
        (
            "in reverse key order",
            in_key_order(&requests).into_iter().rev().collect(),
        ),
        ("in insertion order", requests.clone()),
    ];

    for (case, order) in orders {
        let emulator = Emulator::new();
        seed(&emulator, &requests);

        let mut remaining = requests.clone();
        for target in &order {
            emulator
                .ok("DeletePendingAggregationRequest", &delete_request(&target.0, &target.1))
                .await;
            remaining.retain(|held| held != target);
            let got = reported(case, &emulator).await;
            assert_eq!(
                got,
                in_key_order(&remaining),
                "case {case:?}: after deleting {target:?} expected {} requests, got {got:?}",
                remaining.len()
            );
        }

        let got = reported(case, &emulator).await;
        assert!(
            got.is_empty(),
            "case {case:?}: deleting every pair must leave nothing reported, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_outside_the_modelled_constraints_is_refused_and_persists_nothing() {
    let too_long_region = "r".repeat(AWS_REGION_MAX_LENGTH + 1);
    // Rows differ along WHICH member is outside its modelled constraint and HOW:
    // `RequesterAccountId`'s `^\d{12}$` pattern (too short, too long, non-digits, a
    // separator, empty) and `RequesterAwsRegion`'s 1..64 length, each side of the
    // published bound, plus a required member left out entirely.
    let cases: Vec<(&str, Value)> = vec![
        (
            "an account id of eleven digits",
            delete_request("21098765432", "us-east-1"),
        ),
        (
            "an account id of thirteen digits",
            delete_request("2109876543210", "us-east-1"),
        ),
        (
            "an account id that is not digits at all",
            delete_request("not-an-account", "us-east-1"),
        ),
        (
            "an account id with separators",
            delete_request("2109-8765-4321", "us-east-1"),
        ),
        ("an empty account id", delete_request("", "us-east-1")),
        ("an empty Region", delete_request(ACCOUNT_A, "")),
        (
            "a Region one character past the modelled maximum",
            delete_request(ACCOUNT_A, &too_long_region),
        ),
        (
            "no requester account id at all",
            json!({ "RequesterAwsRegion": "us-east-1" }),
        ),
        (
            "no requester Region at all",
            json!({ "RequesterAccountId": ACCOUNT_A }),
        ),
    ];

    let requests = seeded();
    for (case, request) in cases {
        let emulator = Emulator::new();
        seed(&emulator, &requests);

        let (status, code) = emulator
            .refused("DeletePendingAggregationRequest", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a pair outside the modelled constraints must be refused \
             400 ValidationException, got {status} {code}"
        );

        let got = reported(case, &emulator).await;
        assert_eq!(
            got,
            in_key_order(&requests),
            "case {case:?}: a refused delete must leave every seeded request reported, \
             got {got:?}"
        );
    }
}

#[tokio::test]
async fn the_published_region_length_bounds_are_accepted() {
    // The published bound itself, on both members: an account id of exactly twelve
    // digits with a Region of exactly one character and one of exactly 64.
    for region in ["r".to_string(), "r".repeat(AWS_REGION_MAX_LENGTH)] {
        let case = format!("a Region of {} characters", region.len());
        let emulator = Emulator::new();
        let requests = vec![pair(ACCOUNT_A, &region), pair(ACCOUNT_B, "us-east-1")];
        seed(&emulator, &requests);

        emulator
            .ok(
                "DeletePendingAggregationRequest",
                &delete_request(ACCOUNT_A, &region),
            )
            .await;
        let got = reported(&case, &emulator).await;
        assert_eq!(
            got,
            vec![pair(ACCOUNT_B, "us-east-1")],
            "case {case:?}: expected only the named pair removed, got {got:?}"
        );
    }
}

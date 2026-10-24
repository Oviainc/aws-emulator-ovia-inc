// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the account's storage tier policy: `PutStorageTierPolicy` and the
//! policy `GetStorageTierPolicy` reports.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: the storage tier policy is ONE per account — a put replaces the
//! tier the account was at and stamps `lastUpdatedTime` with the instant of that put, and
//! a get reports what the last put stored without changing it; before any put the account
//! is at the default tier (`STANDARD`) with no `lastUpdatedTime`, which is not a missing
//! resource.
//!
//! The tables below span the class the rule generalises over: the whole documented
//! `StorageTier` catalogue, every ordered pair of tiers (so a case is never only one
//! direction), a repeated put of the tier already set, and the gets interleaved between
//! puts. Any single tier an issue happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// Every storage tier the account can be set to. Source: the model's `StorageTier` enum,
/// as `smithy_query(query="shape",shape="StorageTier")` lists it — `INTELLIGENT_TIERING`
/// and `STANDARD`.
const TIERS: [&str; 2] = ["INTELLIGENT_TIERING", "STANDARD"];

/// The tier an account that no put has touched is at. Source: `PutStorageTierPolicy`'s own
/// documentation — `INTELLIGENT_TIERING` is the tier you "set" to have log data moved
/// automatically, so the other documented tier is where an untouched account sits.
const DEFAULT_TIER: &str = "STANDARD";

/// The code this operation pair declares for a parameter it will not accept. Source: the
/// modelled error set of both operations (`InvalidParameterException`, HTTP 400); neither
/// declares `ValidationException`.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The `storageTier` a response reports, which must always be present.
fn tier_of(case: &str, body: &Value) -> String {
    body.get("storageTier")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a response carries storageTier, got {body}"))
        .to_string()
}

/// The `lastUpdatedTime` a response reports, absent until a put has set the policy.
fn last_updated_of(body: &Value) -> Option<i64> {
    body.get("lastUpdatedTime").and_then(Value::as_i64)
}

/// `GetStorageTierPolicy`, which takes no input.
async fn get_policy(logs: &Logs) -> Value {
    logs.ok("GetStorageTierPolicy", &json!({})).await
}

/// Before any put, the account is at the default tier with no `lastUpdatedTime`, and
/// reading it is never a missing resource however many times it is read — a get stores
/// nothing, so the first put afterwards is still the first update.
#[tokio::test]
async fn an_account_with_no_policy_reports_the_default_tier() {
    let logs = Logs::new();

    for read in 0..5 {
        let body = get_policy(&logs).await;
        let case = format!("read {read} before any put");
        assert_eq!(
            tier_of(&case, &body),
            DEFAULT_TIER,
            "case {case:?}: an account no put has touched is at the default tier, got {body}"
        );
        assert_eq!(
            last_updated_of(&body),
            None,
            "case {case:?}: nothing has updated the policy, so it carries no lastUpdatedTime, got {body}"
        );
    }

    // The repeated reads did not quietly materialise a policy: the first put is still what
    // puts a `lastUpdatedTime` on it.
    let before = now_ms();
    let put = logs
        .ok(
            "PutStorageTierPolicy",
            &json!({"storageTier": TIERS[0]}),
        )
        .await;
    let after = now_ms();
    let stamped = last_updated_of(&put).unwrap_or_else(|| {
        panic!("case the first put: a put reports the lastUpdatedTime it stamped, got {put}")
    });
    assert!(
        (before..=after).contains(&stamped),
        "case the first put: lastUpdatedTime is the instant of the put in epoch millis, \
         expected {before}..={after}, got {stamped}"
    );
}

/// Every documented tier round-trips: a put is answered with the tier it stored, and every
/// later get reports the same tier and the same `lastUpdatedTime` until another put. The
/// table walks every ordered pair of tiers, plus a repeat of the tier already set.
#[tokio::test]
async fn every_documented_tier_round_trips() {
    let logs = Logs::new();

    // Every ordered pair (including each tier after itself), so neither a particular tier
    // nor a particular direction of change can be what makes a case pass.
    let sequence: Vec<&str> = TIERS
        .iter()
        .flat_map(|from| TIERS.iter().flat_map(move |to| [*from, *to]))
        .collect();

    let mut previous_stamp = 0_i64;
    for (index, tier) in sequence.iter().enumerate() {
        let case = format!("put {index} of {tier}");
        let before = now_ms();
        let put = logs
            .ok("PutStorageTierPolicy", &json!({"storageTier": tier}))
            .await;
        let after = now_ms();

        assert_eq!(
            tier_of(&case, &put),
            *tier,
            "case {case:?}: a put is answered with the tier it stored, got {put}"
        );
        let stamped = last_updated_of(&put).unwrap_or_else(|| {
            panic!("case {case:?}: a put reports a lastUpdatedTime, got {put}")
        });
        assert!(
            (before..=after).contains(&stamped),
            "case {case:?}: lastUpdatedTime is the instant of the put in epoch millis, \
             expected {before}..={after}, got {stamped}"
        );
        assert!(
            stamped >= previous_stamp,
            "case {case:?}: a later put is never stamped before an earlier one, \
             expected at least {previous_stamp}, got {stamped}"
        );
        previous_stamp = stamped;

        // Repeated gets report exactly what the put stored — the same tier and the same
        // stamp — so a get neither re-stamps the policy nor changes the tier.
        for read in 0..3 {
            let got = get_policy(&logs).await;
            assert_eq!(
                tier_of(&case, &got),
                *tier,
                "case {case:?} read {read}: a get reports the tier the last put stored, got {got}"
            );
            assert_eq!(
                last_updated_of(&got),
                Some(stamped),
                "case {case:?} read {read}: a get reports the stamp of the last put, got {got}"
            );
        }
    }
}

/// A put that cannot be stored is refused with the code this operation declares, and
/// stores nothing: the account stays at the tier the last accepted put set — including
/// when nothing has been put at all, where the refusal leaves the account at the default.
#[tokio::test]
async fn requests_that_cannot_be_stored_are_refused_and_store_nothing() {
    let logs = Logs::new();

    // Values outside the documented catalogue, in the forms a client gets them wrong in:
    // a plausible other tier name, a case variant of a legal value, one with surrounding
    // whitespace, the empty string, and the required member absent altogether.
    let refused = [
        ("a tier nobody declares", json!({"storageTier": "GLACIER"})),
        ("a case variant", json!({"storageTier": "Standard"})),
        (
            "a legal tier with whitespace",
            json!({"storageTier": " STANDARD "}),
        ),
        ("the empty string", json!({"storageTier": ""})),
        ("an absent required member", json!({})),
    ];

    for (case, body) in &refused {
        assert_eq!(
            logs.err("PutStorageTierPolicy", body).await,
            INVALID_PARAMETER,
            "case {case:?}: only a tier the model declares can be stored"
        );
    }

    // Nothing was stored, so the account is still at the default and still carries no
    // stamp.
    let untouched = get_policy(&logs).await;
    assert_eq!(
        tier_of("after the refusals", &untouched),
        DEFAULT_TIER,
        "case after the refusals: a refused put persists nothing, got {untouched}"
    );
    assert_eq!(
        last_updated_of(&untouched),
        None,
        "case after the refusals: a refused put stamps nothing, got {untouched}"
    );

    // With a policy in place, a refused put still changes neither the tier nor the stamp.
    let stored = logs
        .ok("PutStorageTierPolicy", &json!({"storageTier": TIERS[0]}))
        .await;
    for (case, body) in &refused {
        assert_eq!(
            logs.err("PutStorageTierPolicy", body).await,
            INVALID_PARAMETER,
            "case {case:?} with a policy in place: only a declared tier can be stored"
        );
        let after = get_policy(&logs).await;
        assert_eq!(
            after.get("storageTier"),
            stored.get("storageTier"),
            "case {case:?}: a refused put leaves the stored tier alone, got {after}"
        );
        assert_eq!(
            last_updated_of(&after),
            last_updated_of(&stored),
            "case {case:?}: a refused put leaves the stamp alone, got {after}"
        );
    }
}

/// The policy is the ACCOUNT's, not a log group's: creating and deleting log groups never
/// changes it, and it survives every log group going away.
#[tokio::test]
async fn the_policy_belongs_to_the_account_and_not_to_a_log_group() {
    let logs = Logs::new();
    let stored = logs
        .ok("PutStorageTierPolicy", &json!({"storageTier": TIERS[0]}))
        .await;

    let groups = ["tiered-a", "tiered.b", "tiered/c"];
    for group in groups {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        let after = get_policy(&logs).await;
        assert_eq!(
            after, stored,
            "case created {group}: a log group does not own the account's policy, got {after}"
        );
    }
    for group in groups {
        logs.ok("DeleteLogGroup", &json!({"logGroupName": group}))
            .await;
        let after = get_policy(&logs).await;
        assert_eq!(
            after, stored,
            "case deleted {group}: deleting a log group leaves the account's policy, got {after}"
        );
    }
}

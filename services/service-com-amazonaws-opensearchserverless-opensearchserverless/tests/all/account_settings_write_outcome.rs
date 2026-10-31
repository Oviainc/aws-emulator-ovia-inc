// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateAccountSettings` call's audit record says it DID, end to end over the
//! wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body) through the
//! harness this crate's `collection_lifecycle` suite owns, and reads the records back from
//! the one `tracing` capture `audit_capture` installs.
//!
//! The rule under test: an audit record names the outcome the call really reached, so the
//! write outcome is recorded only by a call that changed the resource — a call that left
//! the account's stored limits exactly as it found them reported them instead, whether
//! because it named no `capacityLimits`, named one with neither member, or restated only
//! limits the account already holds.
//!
//! The table spans what the rule generalises over: which `CapacityLimits` members the
//! request names (neither, each alone, both, the member itself absent), whether the value
//! each carries differs from the stored one, values from the published minimum to the
//! account's own OCU quota ceiling, and a refusal past that ceiling, which writes nothing
//! and keeps its own outcome.

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_ACCOUNT_CAPACITY_IN_OCU;

use crate::audit_capture::{audit_capture, CapturedRecords};
use crate::collection_lifecycle::{call_signed_as, emulator, CLIENT_ERROR};

/// The access key id THIS suite signs with: its own, used by no sibling suite, because the
/// capture is this test binary's single global subscriber and suites run in parallel — the
/// identity the request presented is what tells one suite's records from another's.
const AUDIT_ACCESS_KEY_ID: &str = "AKIDSETTINGSWRITE";

/// The Region every row is signed for. The settings belong to the account rather than to a
/// Region (the `account_settings_audit` suite spans the Regions); this suite varies the
/// request's limits instead.
const REGION: &str = "us-east-1";

/// The outcome token a call that CHANGED the account's stored limits records
/// (`crate::state::AuditOutcome::SettingsUpdated`).
const WROTE: &str = "settings-updated";

/// The outcome token a call that left the stored limits as they stand records
/// (`crate::state::AuditOutcome::SettingsUnchanged`).
const UNCHANGED: &str = "settings-unchanged";

/// The outcome token a request past the account's OCU quota records.
const REFUSED: &str = "quota-exceeded";

/// The outcome token a report of a stored resource records, which is what
/// `GetAccountSettings` leaves (`crate::state::AuditOutcome::PolicyRead`).
const READ: &str = "policy-read";

/// The capacity limits an account carries before anything configures it, from AWS's
/// "Managing capacity limits for Amazon OpenSearch Serverless" (the page `CapacityLimits`'
/// own documentation links): 10 OCUs for indexing and 10 for search.
const DEFAULTS: (i64, i64) = (10, 10);

/// The least capacity either limit may be set to, from the model's own `@range(min: 2)` on
/// `IndexingCapacityValue` and `SearchCapacityValue`
/// (`smithy_query(query="shape", shape="IndexingCapacityValue,SearchCapacityValue")`).
const MIN_CAPACITY: i64 = 2;

/// One row of the table below: the request to drive, the status it must answer, the outcome
/// token its audit record must carry, and the limits the account holds afterwards.
struct Row {
    case: &'static str,
    request: Value,
    status: u16,
    outcome: &'static str,
    after: (i64, i64),
}

/// A [`Row`] the service accepts, which answers 200.
fn accepted(case: &'static str, request: Value, outcome: &'static str, after: (i64, i64)) -> Row {
    Row {
        case,
        request,
        status: 200,
        outcome,
        after,
    }
}

/// A [`Row`] the service refuses, which writes nothing and leaves `after` standing.
fn refused(case: &'static str, request: Value, outcome: &'static str, after: (i64, i64)) -> Row {
    Row {
        case,
        request,
        status: CLIENT_ERROR,
        outcome,
        after,
    }
}

/// The records THIS suite's calls wrote, taken from the shared capture.
fn records(captured: &CapturedRecords) -> Vec<String> {
    captured.drain_for(AUDIT_ACCESS_KEY_ID)
}

/// The one audit record `case`'s call left, asserting the call left EXACTLY one.
fn one_record(captured: &CapturedRecords, case: &str) -> String {
    let written = records(captured);
    assert_eq!(
        written.len(),
        1,
        "case {case:?}: exactly one audit record per call, got {written:?}"
    );
    written.into_iter().next().unwrap_or_default()
}

/// The `capacityLimits` pair `answer` reports, as `(indexing, search)`.
fn reported(answer: &Value) -> (Option<i64>, Option<i64>) {
    let limits = &answer["accountSettingsDetail"]["capacityLimits"];
    (
        limits["maxIndexingCapacityInOCU"].as_i64(),
        limits["maxSearchCapacityInOCU"].as_i64(),
    )
}

/// RULE: the outcome an `UpdateAccountSettings` record carries is the one the call reached —
/// the write outcome only when the account's stored limits changed.
#[tokio::test]
async fn update_account_settings_records_a_write_only_when_the_limits_changed() {
    let captured = audit_capture();
    let router = emulator();
    let ceiling = i64::from(MAX_ACCOUNT_CAPACITY_IN_OCU);

    // Each row's "before" is the previous row's stored pair, so a row naming a value the
    // account already holds is a genuine no-op rather than a fresh store.
    let rows: Vec<Row> = vec![
        accepted(
            "both limits moved off the defaults",
            json!({ "capacityLimits": {
                "maxIndexingCapacityInOCU": 12,
                "maxSearchCapacityInOCU": 14,
            }}),
            WROTE,
            (12, 14),
        ),
        accepted(
            "the same pair restated",
            json!({ "capacityLimits": {
                "maxIndexingCapacityInOCU": 12,
                "maxSearchCapacityInOCU": 14,
            }}),
            UNCHANGED,
            (12, 14),
        ),
        accepted(
            "no capacityLimits member at all",
            json!({}),
            UNCHANGED,
            (12, 14),
        ),
        accepted(
            "a capacityLimits naming neither member",
            json!({ "capacityLimits": {} }),
            UNCHANGED,
            (12, 14),
        ),
        accepted(
            "only the indexing limit, at the published minimum",
            json!({ "capacityLimits": { "maxIndexingCapacityInOCU": MIN_CAPACITY }}),
            WROTE,
            (MIN_CAPACITY, 14),
        ),
        accepted(
            "only the indexing limit, at the value it already holds",
            json!({ "capacityLimits": { "maxIndexingCapacityInOCU": MIN_CAPACITY }}),
            UNCHANGED,
            (MIN_CAPACITY, 14),
        ),
        accepted(
            "only the search limit, at the account's OCU quota ceiling",
            json!({ "capacityLimits": { "maxSearchCapacityInOCU": ceiling }}),
            WROTE,
            (MIN_CAPACITY, ceiling),
        ),
        accepted(
            "both limits restated at the values the account holds",
            json!({ "capacityLimits": {
                "maxIndexingCapacityInOCU": MIN_CAPACITY,
                "maxSearchCapacityInOCU": ceiling,
            }}),
            UNCHANGED,
            (MIN_CAPACITY, ceiling),
        ),
        accepted(
            "one limit restated and the other lowered",
            json!({ "capacityLimits": {
                "maxIndexingCapacityInOCU": MIN_CAPACITY,
                "maxSearchCapacityInOCU": 7,
            }}),
            WROTE,
            (MIN_CAPACITY, 7),
        ),
        refused(
            "one OCU past the account's quota, so nothing is written",
            json!({ "capacityLimits": { "maxIndexingCapacityInOCU": ceiling + 1 }}),
            REFUSED,
            (MIN_CAPACITY, 7),
        ),
        accepted(
            "back to the documented defaults",
            json!({ "capacityLimits": {
                "maxIndexingCapacityInOCU": DEFAULTS.0,
                "maxSearchCapacityInOCU": DEFAULTS.1,
            }}),
            WROTE,
            DEFAULTS,
        ),
    ];

    let mut before = DEFAULTS;
    for Row {
        case,
        request,
        status: expected_status,
        outcome,
        after,
    } in rows
    {
        let (status, code, answer) = call_signed_as(
            &router,
            REGION,
            AUDIT_ACCESS_KEY_ID,
            "UpdateAccountSettings",
            request,
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {code} with {answer}"
        );

        let record = one_record(captured, case);
        assert!(
            record.contains(&format!("outcome={outcome}")),
            "case {case:?}: expected outcome={outcome}, got {record:?}"
        );
        assert!(
            record.contains("operation=UpdateAccountSettings")
                && record.contains("resource=account-settings"),
            "case {case:?}: the record must name the operation and the settings resource, \
             got {record:?}"
        );
        // The rule itself, stated over this row's own before/after rather than read off the
        // table: the write outcome appears exactly when the stored limits moved.
        if expected_status == 200 {
            assert_eq!(
                record.contains(&format!("outcome={WROTE}")),
                after != before,
                "case {case:?}: a record may claim a write only when the stored limits \
                 changed ({before:?} -> {after:?}), got {record:?}"
            );
        }

        // The store the outcome is about: read it back through `GetAccountSettings`, which
        // leaves its own record of a read, so the write outcome above is pinned against
        // what the account really holds.
        let (status, code, answer) = call_signed_as(
            &router,
            REGION,
            AUDIT_ACCESS_KEY_ID,
            "GetAccountSettings",
            json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAccountSettings expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            reported(&answer),
            (Some(after.0), Some(after.1)),
            "case {case:?}: expected the stored limits {after:?}, got {answer}"
        );
        let read_record = one_record(captured, case);
        assert!(
            read_record.contains("operation=GetAccountSettings")
                && read_record.contains(&format!("outcome={READ}")),
            "case {case:?}: reading the settings must be recorded as outcome={READ}, got \
             {read_record:?}"
        );

        before = after;
    }
}

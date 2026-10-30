// SPDX-License-Identifier: CC0-1.0

//! The audit record every **account settings** call leaves, end to end over the wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body) through the
//! harness this crate's `collection_lifecycle` suite owns, so the boundary validator, the
//! protocol codec and the handler all take part exactly as they do for an SDK client. The
//! records are read back from the one `tracing` capture `audit_capture` installs.
//!
//! The rule under test: a call addressing a resource this crate audits leaves exactly ONE
//! structured record naming the service, the operation, the identity the request presented,
//! the request target, that resource and the outcome — the READ of the resource as much as
//! the write of it, and whatever the call answered.
//!
//! The table spans what the rule generalises over for the account's own settings: both
//! operations on the resource, reporting and state-changing, accepted and refused, the
//! outcome each exit records, the Region the call is signed for, and a read repeated after
//! a write and after a refusal.

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_ACCOUNT_CAPACITY_IN_OCU;

use crate::audit_capture::{audit_capture, CapturedRecords};
use crate::collection_lifecycle::{call_signed_as, emulator, CLIENT_ERROR};

/// This service's SigV4 signing name, which every audit record names as its service
/// (`smithy_query(query="service_info")` -> `sigv4Name`).
const SERVICE: &str = "aoss";

/// The access key id THIS suite signs with, presented in the `Authorization` header's
/// credential scope and reported by `emulator_server::extract_akid`.
///
/// Its own, used by no sibling suite: the capture is this test binary's single global
/// subscriber, so the records of a suite running in parallel over the same routes are told
/// apart from this suite's by the identity the request presented.
const AUDIT_ACCESS_KEY_ID: &str = "AKIDSETTINGSAUDIT";

/// The `resource` field the records of this resource carry
/// (`crate::state::AUDIT_ACCOUNT_SETTINGS_RESOURCE`): one resource per account, so nothing
/// request-derived identifies it.
const SETTINGS_RESOURCE: &str = "account-settings";

/// Regions a row is signed for, so the record is proven to be about the ACCOUNT's settings
/// whichever Region asked.
const REGIONS: [&str; 3] = ["us-east-1", "eu-central-1", "ap-southeast-2"];

/// The records THIS suite's calls wrote, taken from the shared capture.
fn records(captured: &CapturedRecords) -> Vec<String> {
    captured.drain_for(AUDIT_ACCESS_KEY_ID)
}

/// RULE: every account-settings call — the read as much as the write, accepted or refused —
/// leaves exactly ONE structured audit record naming the service, the operation, the
/// identity the request presented, the request target, the settings resource and the
/// outcome.
#[tokio::test]
async fn every_account_settings_call_emits_one_structured_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a missing record below can never be mistaken for a capture that
    // receives nothing: the capture must receive an event emitted right here.
    tracing::info!(
        target: "emulator::audit",
        access_key_id = AUDIT_ACCESS_KEY_ID,
        "probe"
    );
    let probe = records(captured);
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let router = emulator();
    let ceiling = i64::from(MAX_ACCOUNT_CAPACITY_IN_OCU);

    // (case, region, operation, request, expected status, expected outcome token). The rows
    // differ along what the rule generalises over: which operation, whether the call
    // reports or writes, whether it was accepted or refused, and the Region it is signed
    // for. A read is driven before any write, after a write and after a refusal, because a
    // record is owed by the read itself and not by what the account happens to hold.
    let rows: Vec<(&str, &str, &str, Value, u16, &str)> = vec![
        (
            "the settings of an account nobody configured were read",
            REGIONS[0],
            "GetAccountSettings",
            json!({}),
            200,
            "policy-read",
        ),
        (
            "the settings were written",
            REGIONS[0],
            "UpdateAccountSettings",
            json!({ "capacityLimits": {
                "maxIndexingCapacityInOCU": 12,
                "maxSearchCapacityInOCU": 14,
            }}),
            200,
            "settings-updated",
        ),
        (
            "the written settings were read",
            REGIONS[0],
            "GetAccountSettings",
            json!({}),
            200,
            "policy-read",
        ),
        (
            "a write naming no limits",
            REGIONS[1],
            "UpdateAccountSettings",
            json!({}),
            200,
            "settings-updated",
        ),
        (
            "the write asks for more than the account's OCU quota",
            REGIONS[1],
            "UpdateAccountSettings",
            json!({ "capacityLimits": { "maxIndexingCapacityInOCU": ceiling + 1 }}),
            CLIENT_ERROR,
            "quota-exceeded",
        ),
        (
            "the settings were read after the refusal",
            REGIONS[2],
            "GetAccountSettings",
            json!({}),
            200,
            "policy-read",
        ),
    ];

    for (case, region, operation, request, expected_status, outcome) in rows {
        audited(
            &router, captured, case, region, operation, request, expected_status, outcome,
        )
        .await;
    }

    // The read is owed its record on every call, not only the first: a repeat in each
    // Region leaves one record of its own, and reports the limits the write above stored,
    // so the record is a `tracing` line and not a state change.
    for region in REGIONS {
        let case = format!("the settings were read again in {region}");
        let answer = audited(
            &router,
            captured,
            &case,
            region,
            "GetAccountSettings",
            json!({}),
            200,
            "policy-read",
        )
        .await;
        let limits = &answer["accountSettingsDetail"]["capacityLimits"];
        assert_eq!(
            (
                limits["maxIndexingCapacityInOCU"].as_i64(),
                limits["maxSearchCapacityInOCU"].as_i64()
            ),
            (Some(12), Some(14)),
            "case {case}: the audited read must still report the stored limits, got {answer}"
        );
    }
}

/// Drive `operation` with `request` in `region`, assert it answered `expected_status`, and
/// assert the call left exactly ONE structured audit record for the account's settings,
/// carrying `outcome` and none of the request's signature. Answers the response body.
#[allow(clippy::too_many_arguments)]
async fn audited(
    router: &smithy_server_runtime::MultiServiceRouter,
    captured: &CapturedRecords,
    case: &str,
    region: &str,
    operation: &str,
    request: Value,
    expected_status: u16,
    outcome: &str,
) -> Value {
    let (status, code, answer) =
        call_signed_as(router, region, AUDIT_ACCESS_KEY_ID, operation, request).await;
    assert_eq!(
        status, expected_status,
        "case {case}: expected {expected_status}, got {status} {code} with {answer}"
    );

    let written = records(captured);
    assert_eq!(
        written.len(),
        1,
        "case {case}: exactly one audit record per call, got {written:?}"
    );
    let record = &written[0];
    for expected in [
        format!("service={SERVICE}"),
        format!("operation={operation}"),
        format!("resource={SETTINGS_RESOURCE}"),
        format!("outcome={outcome}"),
        "request_target=POST /".to_string(),
        // The identity the request presented: this suite signs with this access key id, so
        // the field's CONTENT is checked, not merely its presence.
        format!("access_key_id={AUDIT_ACCESS_KEY_ID}"),
    ] {
        assert!(
            record.contains(&expected),
            "case {case}: the audit record must carry {expected:?}, got {record:?}"
        );
    }
    // Metadata only: the request's signature never reaches the log.
    assert!(
        !record.contains("deadbeef"),
        "case {case}: the audit record must not carry the request signature, got {record:?}"
    );
    answer
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MONOTONICITY of the last-usage record `GetKeyLastUsage`
//! reports, driven over the wire through the shared [`super::harness`].
//!
//! The rule under test: once KMS's usage tracking has produced a last-usage record for a
//! key, `GetKeyLastUsage` keeps reporting a record for that key — "if the KeyLastUsage
//! response element is present, the key has been used for a successful cryptographic
//! operation since the TrackingStartDate"
//! (`smithy_query(query="doc",shape="GetKeyLastUsage")`) is a monotone fact about the
//! key, so a later use REPLACES the reported record once the tracking produces the newer
//! one and never takes the key back to the "not used since tracking began" state its
//! EMPTY record reports. The reported `Timestamp` therefore only advances: it is held at
//! the value a reader has already been told while the newer use is still in the pipeline
//! ([`state::USAGE_TRACKING_LAG`]), and then moves strictly forward.
//!
//! What a read taken straight after the FIRST use of a key is answered — the empty record
//! — is the subject of [`super::key_last_usage`]; this suite is about every use after
//! that one.
//!
//! The rows are the tracked operations
//! (`smithy_query(query="shape",shape="KeyLastUsageTrackingOperation")`) that use the key
//! on their own input, so they differ in the operation, in the kind of key it needs
//! (symmetric encryption, HMAC, elliptic-curve signing) and in what the call produces.
//! Each row is driven through THREE successive uses, so the advance is asserted on a
//! record that replaced a record that had itself replaced one.
//!
//! The rows are driven in PHASES — every row's call first, then one wait for the tracking
//! — so the suite waits out the lag once per round rather than once per row.

use serde_json::{json, Value};

use super::harness::{b64, create_key, ok, router, text};

use service_com_amazonaws_kms_trentservice::state::USAGE_TRACKING_LAG;

/// The message every row seals, MACs or signs.
const MESSAGE: &[u8] = b"the advancing last usage record";

/// How many successive uses of one key each row performs. Three, so the last record
/// asserted replaced a record that had itself replaced one.
const USES_PER_ROW: usize = 3;

/// One row of the table: a tracked operation, the key it needs and the request that uses
/// that key.
struct Row {
    /// The operation, as `KeyLastUsageData.Operation` names it.
    operation: &'static str,
    /// The `CreateKey` request for the key this operation needs: its `KeySpec` and
    /// `KeyUsage`
    /// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")`).
    key: Value,
    /// The request that performs `operation` on the key `key_id` names.
    request: fn(&str) -> Value,
}

/// The table: tracked operations spanning the kinds of key a cryptographic operation
/// needs.
fn table() -> Vec<Row> {
    vec![
        Row {
            operation: "Encrypt",
            key: json!({"Description": "Encrypt"}),
            request: |key_id| json!({"KeyId": key_id, "Plaintext": b64(MESSAGE)}),
        },
        Row {
            operation: "GenerateDataKey",
            key: json!({"Description": "GenerateDataKey"}),
            request: |key_id| json!({"KeyId": key_id, "KeySpec": "AES_256"}),
        },
        Row {
            operation: "GenerateDataKeyWithoutPlaintext",
            key: json!({"Description": "GenerateDataKeyWithoutPlaintext"}),
            request: |key_id| json!({"KeyId": key_id, "NumberOfBytes": 32}),
        },
        Row {
            operation: "GenerateMac",
            key: json!({
                "Description": "GenerateMac",
                "KeySpec": "HMAC_256",
                "KeyUsage": "GENERATE_VERIFY_MAC"
            }),
            request: |key_id| json!({
                "KeyId": key_id,
                "Message": b64(MESSAGE),
                "MacAlgorithm": "HMAC_SHA_256"
            }),
        },
        Row {
            operation: "Sign",
            key: json!({
                "Description": "Sign",
                "KeySpec": "ECC_NIST_P256",
                "KeyUsage": "SIGN_VERIFY"
            }),
            request: |key_id| json!({
                "KeyId": key_id,
                "Message": b64(MESSAGE),
                "MessageType": "RAW",
                "SigningAlgorithm": "ECDSA_SHA_256"
            }),
        },
    ]
}

/// The `GetKeyLastUsage` answer for `key_id`, taken as it is — without waiting for the
/// tracking.
async fn reported(
    router: &smithy_server_runtime::MultiServiceRouter,
    key_id: &str,
) -> Value {
    ok(router, "GetKeyLastUsage", &json!({"KeyId": key_id})).await
}

/// The `Timestamp` of the record `answer` carries, in milliseconds since the epoch, or
/// `None` when the record is empty.
fn timestamp(answer: &Value) -> Option<f64> {
    answer["KeyLastUsage"]["Timestamp"].as_f64()
}

/// Assert `answer` reports a record of `operation` that is at or after `floor`, which is
/// what "the element stays present" means on the wire, and report its timestamp.
fn assert_present(case: &str, operation: &str, answer: &Value, floor: f64) -> f64 {
    let Some(reported_at) = timestamp(answer) else {
        panic!(
            "case {case}: the key has been used, so the record must stay present — \
             expected a Timestamp at or after {floor}, got {answer}"
        );
    };
    assert_eq!(
        answer["KeyLastUsage"]["Operation"].as_str().unwrap_or_default(),
        operation,
        "case {case}: expected the record to name {operation}, got {answer}"
    );
    assert!(
        reported_at >= floor,
        "case {case}: the reported Timestamp must not move backwards from {floor}, got \
         {reported_at}"
    );
    reported_at
}

/// Wait until the tracking has produced the FIRST record of a key — until then the
/// element is empty, which is what a key that has not been used since tracking began
/// reports.
async fn poll_for_first_record(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    key_id: &str,
    floor: f64,
) -> Value {
    let deadline = std::time::Instant::now() + USAGE_TRACKING_LAG * 20;
    loop {
        let answer = reported(router, key_id).await;
        if timestamp(&answer).is_some() {
            assert_present(case, operation, &answer, floor);
            return answer;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: the tracking must produce the record of the {operation} of \
             {key_id}, got {answer}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Wait until the tracking has produced the record of the use just made — a record whose
/// `Timestamp` is strictly after `previous` — asserting on EVERY read in between that the
/// record stays present and never regresses, which is what a client polling for the
/// advance observes.
async fn poll_for_advance(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    key_id: &str,
    previous: f64,
) -> Value {
    let deadline = std::time::Instant::now() + USAGE_TRACKING_LAG * 20;
    loop {
        let answer = reported(router, key_id).await;
        let reported_at = assert_present(case, operation, &answer, previous);
        if reported_at > previous {
            return answer;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: the tracking must produce the newer record, so the reported \
             Timestamp must advance past {previous}, got {answer}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// The rule: a use the tracking has not caught up with never erases the record already
/// reported, and the reported `Timestamp` advances strictly once it has.
#[tokio::test]
async fn the_reported_last_usage_record_is_held_over_and_then_advances() {
    let router = router();
    let rows = table();

    // One key per row, and the record every row's first use produced.
    let mut key_ids: Vec<String> = Vec::new();
    for row in &rows {
        let metadata = create_key(&router, &row.key).await;
        key_ids.push(text(row.operation, &metadata, "KeyId"));
    }

    // Round 1: the first use of every key, then one wait for the tracking.
    for (row, key_id) in rows.iter().zip(&key_ids) {
        ok(&router, row.operation, &(row.request)(key_id)).await;
    }
    let mut records: Vec<Value> = Vec::new();
    for (row, key_id) in rows.iter().zip(&key_ids) {
        let case = format!("{} use 1", row.operation);
        // The key's creation date is the floor of its first record: tracking starts when
        // the key is minted.
        let floor = reported(&router, key_id).await["KeyCreationDate"]
            .as_f64()
            .unwrap_or_default();
        records.push(poll_for_first_record(&router, &case, row.operation, key_id, floor).await);
    }

    // Rounds 2..: each later use is held over while the tracking produces it, and then
    // advances the reported record.
    for use_number in 2..=USES_PER_ROW {
        for (index, (row, key_id)) in rows.iter().zip(&key_ids).enumerate() {
            let case = format!("{} use {use_number} straight after the call", row.operation);
            let before = records[index].clone();
            ok(&router, row.operation, &(row.request)(key_id)).await;

            // The read a caller takes straight after this use: the record the tracking
            // has already produced is still the answer — the newer use replaces nothing
            // until the tracking produces it, and it never takes the key back to the
            // "not used since tracking began" state an empty record reports.
            let straight_after = reported(&router, key_id).await;
            assert_eq!(
                straight_after, before,
                "case {case}: expected the record already reported, got {straight_after}"
            );
            records[index] = straight_after;
        }

        for (index, (row, key_id)) in rows.iter().zip(&key_ids).enumerate() {
            let case = format!("{} use {use_number}", row.operation);
            let before = records[index].clone();
            let previous = timestamp(&before).unwrap_or_default();
            let advanced =
                poll_for_advance(&router, &case, row.operation, key_id, previous).await;

            // The record that replaced it is this call's own: its CloudTrail event id and
            // its KMS request id belong to the call being recorded, so neither is the one
            // the replaced record carried.
            for member in ["CloudTrailEventId", "KmsRequestId"] {
                let value = advanced["KeyLastUsage"][member].as_str().unwrap_or_default();
                assert!(
                    !value.is_empty(),
                    "case {case}: expected a {member} on the advanced record, got {advanced}"
                );
                assert_ne!(
                    value,
                    before["KeyLastUsage"][member].as_str().unwrap_or_default(),
                    "case {case}: expected a {member} of this call's own, got the replaced \
                     record's"
                );
            }
            records[index] = advanced;
        }
    }

    // Every row advanced, so no row sat on the record its first use produced.
    for (row, key_id) in rows.iter().zip(&key_ids) {
        let answer = reported(&router, key_id).await;
        assert_eq!(
            answer["KeyId"].as_str().unwrap_or_default(),
            key_id,
            "case {} final: this answer names the key by its key ID, got {answer}",
            row.operation
        );
        assert!(
            timestamp(&answer).is_some(),
            "case {} final: the key has been used {USES_PER_ROW} times, so the record must \
             be present, got {answer}",
            row.operation
        );
    }
}

/// The rule, on one key beside another: the record held over belongs to the key that was
/// used, and a key the tracking has produced no record for holds nothing — neither its
/// own first use, until the tracking produces it, nor another key's.
#[tokio::test]
async fn a_held_over_record_belongs_to_the_key_that_was_used() {
    let router = router();
    let used = create_key(&router, &json!({"Description": "used twice"})).await;
    let used_id = text("setup", &used, "KeyId");
    let untouched = create_key(&router, &json!({"Description": "never used"})).await;
    let untouched_id = text("setup", &untouched, "KeyId");

    let encrypt = json!({"KeyId": used_id, "Plaintext": b64(MESSAGE)});
    ok(&router, "Encrypt", &encrypt).await;
    // Nothing is held over for a FIRST use the tracking has not produced yet: the record
    // is reported empty, which is what the recorded answer of a read taken straight after
    // a cryptographic operation carries.
    let before_any_record = reported(&router, &used_id).await;
    assert_eq!(
        before_any_record["KeyLastUsage"],
        json!({}),
        "case first use straight after the call: the tracking has produced no record yet, \
         so the element is empty, got {before_any_record}"
    );
    let first = poll_for_first_record(
        &router,
        "first use",
        "Encrypt",
        &used_id,
        used["CreationDate"].as_f64().unwrap_or_default(),
    )
    .await;

    ok(&router, "Encrypt", &encrypt).await;
    // Held over for the key that was used …
    let straight_after = reported(&router, &used_id).await;
    assert_eq!(
        straight_after, first,
        "case held over: expected the record already reported, got {straight_after}"
    );
    // … and for the key that never was, nothing is held at all: its record stays empty,
    // because the element is empty exactly while the key has not been used since
    // tracking began.
    let other = reported(&router, &untouched_id).await;
    assert_eq!(
        other["KeyLastUsage"],
        json!({}),
        "case untouched: a key that has had no cryptographic operation reports the empty \
         record, got {other}"
    );
    assert_eq!(
        other["KeyId"].as_str().unwrap_or_default(),
        untouched_id,
        "case untouched: expected the answer to name its own key, got {other}"
    );

    let advanced = poll_for_advance(
        &router,
        "second use",
        "Encrypt",
        &used_id,
        timestamp(&first).unwrap_or_default(),
    )
    .await;
    assert!(
        timestamp(&advanced) > timestamp(&first),
        "case second use: expected the Timestamp to advance past {:?}, got {advanced}",
        timestamp(&first)
    );
    // The untouched key is still untouched: the advance belongs to one key.
    let other_again = reported(&router, &untouched_id).await;
    assert_eq!(
        other_again, other,
        "case untouched again: a use of another key must not fill this key's record"
    );
    // And the reads above, which are the only calls this key has had besides the two
    // uses, left the record where it was: asking again answers the same.
    let again = reported(&router, &used_id).await;
    assert_eq!(
        again, advanced,
        "case reread: a second read must answer the same record, got {again}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MONOTONICITY of the last-usage record `GetKeyLastUsage`
//! reports, driven over the wire through the shared [`super::harness`].
//!
//! The rule under test: KMS's usage tracking begins recording a key with the first
//! cryptographic operation that uses it and takes a span to produce that key's first
//! record ([`state::USAGE_TRACKING_LAG`]); while it has produced none the element is
//! EMPTY, and from the instant it is recording the key `GetKeyLastUsage` reports "the last
//! successful cryptographic operation performed with" it
//! (`smithy_query(query="doc",shape="GetKeyLastUsage")`). So once a record has been
//! reported for a key, a LATER use replaces it — on the read taken straight after that
//! use, with no further wait — and never takes the key back to the "not used since
//! tracking began" state its empty record reports: "if the KeyLastUsage response element
//! is present, the key has been used for a successful cryptographic operation since the
//! TrackingStartDate" (same doc) is a monotone fact about the key, and the reported
//! `Timestamp` only moves forward.
//!
//! What a read taken straight after the FIRST use of a key is answered — the empty record
//! — is also [`super::key_last_usage`]'s subject; this suite is about every use after
//! that one.
//!
//! The rows are the tracked operations
//! (`smithy_query(query="shape",shape="KeyLastUsageTrackingOperation")`) that use the key
//! on their own input, so they differ in the operation, in the kind of key it needs
//! (symmetric encryption, HMAC, elliptic-curve signing) and in what the call produces.
//! Each row is driven through THREE successive uses, so the advance is asserted on a
//! record that replaced a record that had itself replaced one.
//!
//! The rows are driven in PHASES — every row's first call first, then one wait for the
//! tracking — so the suite waits out the start-up span once rather than once per row.

use serde_json::{json, Value};

use super::harness::{b64, create_key, ok, router, text};

use service_com_amazonaws_kms_trentservice::state::USAGE_TRACKING_LAG;

/// The message every row seals, MACs or signs.
const MESSAGE: &[u8] = b"the advancing last usage record";

/// How many successive uses of one key each row performs. Three, so the last record
/// asserted replaced a record that had itself replaced one.
const USES_PER_ROW: usize = 3;

/// How far apart the rows space two uses of one key. The wire renders an instant in
/// milliseconds, so two uses are spaced past that resolution for the advance of the
/// reported `Timestamp` to be observable at all.
const PAST_THE_WIRE_RESOLUTION: std::time::Duration = std::time::Duration::from_millis(5);

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
async fn reported(router: &smithy_server_runtime::MultiServiceRouter, key_id: &str) -> Value {
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
        answer["KeyLastUsage"]["Operation"]
            .as_str()
            .unwrap_or_default(),
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

/// Assert the record `advanced` carries is the one the use just made wrote: it names
/// `operation`, its `Timestamp` is strictly after `previous`, and its CloudTrail event id
/// and KMS request id are this call's own rather than the replaced record's.
fn assert_advanced(case: &str, operation: &str, advanced: &Value, before: &Value) {
    let previous = timestamp(before).unwrap_or_default();
    let reported_at = assert_present(case, operation, advanced, previous);
    assert!(
        reported_at > previous,
        "case {case}: the record of the later use replaces the one reported, so the \
         Timestamp must advance past {previous}, got {reported_at}"
    );
    for member in ["CloudTrailEventId", "KmsRequestId"] {
        let value = advanced["KeyLastUsage"][member]
            .as_str()
            .unwrap_or_default();
        assert!(
            !value.is_empty(),
            "case {case}: expected a {member} on the advanced record, got {advanced}"
        );
        assert_ne!(
            value,
            before["KeyLastUsage"][member].as_str().unwrap_or_default(),
            "case {case}: expected a {member} of this call's own, got the replaced record's"
        );
    }
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

/// The rule: once the tracking is recording a key, the record of a later use is what the
/// very next read reports, and the reported `Timestamp` advances strictly.
#[tokio::test]
async fn the_reported_last_usage_record_advances_with_every_later_use() {
    let router = router();
    let rows = table();

    // One key per row, and the record every row's first use produced.
    let mut key_ids: Vec<String> = Vec::new();
    let mut creation_dates: Vec<f64> = Vec::new();
    for row in &rows {
        let metadata = create_key(&router, &row.key).await;
        key_ids.push(text(row.operation, &metadata, "KeyId"));
        creation_dates.push(metadata["CreationDate"].as_f64().unwrap_or_default());
    }

    // Round 1: the first use of every key. The tracking has produced nothing for the key
    // yet, so the read taken straight after it is answered the empty record …
    for (row, key_id) in rows.iter().zip(&key_ids) {
        ok(&router, row.operation, &(row.request)(key_id)).await;
        let straight_after = reported(&router, key_id).await;
        assert_eq!(
            straight_after["KeyLastUsage"],
            json!({}),
            "case {} use 1 straight after the call: the tracking has produced no record \
             for this key yet, so the element is empty, got {straight_after}",
            row.operation
        );
    }
    // … and then one wait, for the whole table, until it has.
    let mut records: Vec<Value> = Vec::new();
    for ((row, key_id), creation_date) in rows.iter().zip(&key_ids).zip(&creation_dates) {
        let case = format!("{} use 1", row.operation);
        // The key's creation date is the floor of its first record: tracking starts when
        // the key is minted.
        records.push(
            poll_for_first_record(&router, &case, row.operation, key_id, *creation_date).await,
        );
    }

    // Rounds 2..: each later use is reported by the read that follows it, with a
    // Timestamp strictly after the record it replaced.
    for use_number in 2..=USES_PER_ROW {
        for (index, (row, key_id)) in rows.iter().zip(&key_ids).enumerate() {
            let case = format!("{} use {use_number}", row.operation);
            let before = records[index].clone();

            tokio::time::sleep(PAST_THE_WIRE_RESOLUTION).await;
            ok(&router, row.operation, &(row.request)(key_id)).await;

            // The read a caller takes straight after this use: the tracking is recording
            // the key, so what it reports is this use — the element never goes back to
            // the "not used since tracking began" state an empty record reports, and the
            // Timestamp moves forward.
            let advanced = reported(&router, key_id).await;
            assert_advanced(&case, row.operation, &advanced, &before);

            // Reading the record does not change it: asking again answers the same.
            let again = reported(&router, key_id).await;
            assert_eq!(
                again, advanced,
                "case {case}: a second read must answer the same record, got {again}"
            );
            records[index] = advanced;
        }
    }

    // Every row advanced, so no row sat on the record its first use produced.
    for ((row, key_id), record) in rows.iter().zip(&key_ids).zip(&records) {
        let answer = reported(&router, key_id).await;
        assert_eq!(
            answer["KeyId"].as_str().unwrap_or_default(),
            key_id,
            "case {} final: this answer names the key by its key ID, got {answer}",
            row.operation
        );
        assert_eq!(
            answer, *record,
            "case {} final: the key has been used {USES_PER_ROW} times, so the record of \
             the last of them is what is reported, got {answer}",
            row.operation
        );
    }
}

/// The rule, on one key beside another: the record reported belongs to the key that was
/// used, and a key the tracking has produced no record for holds nothing — neither its
/// own first use, until the tracking produces it, nor another key's.
#[tokio::test]
async fn the_reported_record_belongs_to_the_key_that_was_used() {
    let router = router();
    let used = create_key(&router, &json!({"Description": "used twice"})).await;
    let used_id = text("setup", &used, "KeyId");
    let untouched = create_key(&router, &json!({"Description": "never used"})).await;
    let untouched_id = text("setup", &untouched, "KeyId");

    let encrypt = json!({"KeyId": used_id, "Plaintext": b64(MESSAGE)});
    ok(&router, "Encrypt", &encrypt).await;
    // Nothing is reported for a FIRST use the tracking has not produced yet: the record
    // is reported empty, which is what the recorded answer of a read taken straight after
    // the first cryptographic operation on a key carries.
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

    tokio::time::sleep(PAST_THE_WIRE_RESOLUTION).await;
    ok(&router, "Encrypt", &encrypt).await;
    // Reported for the key that was used …
    let advanced = reported(&router, &used_id).await;
    assert_advanced("second use", "Encrypt", &advanced, &first);
    // … and for the key that never was, nothing is reported at all: its record stays
    // empty, because the element is empty exactly while the key has not been used since
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

    // The untouched key is still untouched after a further use of the other one: the
    // advance belongs to one key.
    tokio::time::sleep(PAST_THE_WIRE_RESOLUTION).await;
    ok(&router, "Encrypt", &encrypt).await;
    let advanced_again = reported(&router, &used_id).await;
    assert_advanced("third use", "Encrypt", &advanced_again, &advanced);
    let other_again = reported(&router, &untouched_id).await;
    assert_eq!(
        other_again, other,
        "case untouched again: a use of another key must not fill this key's record"
    );
    // And the reads above, which are the only calls this key has had besides the three
    // uses, left the record where it was: asking again answers the same.
    let again = reported(&router, &used_id).await;
    assert_eq!(
        again, advanced_again,
        "case reread: a second read must answer the same record, got {again}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the key last-usage record `GetKeyLastUsage` reports, driven
//! over the wire through the shared [`super::harness`].
//!
//! The rule under test: a KMS key's last-usage record is written by the CRYPTOGRAPHIC
//! operation that used the key and by nothing else, so every call that succeeds
//! replaces the record with one naming itself, at an instant that never moves
//! backwards and with a CloudTrail event id and a KMS request id of its own; a call
//! that is refused writes nothing; `GetKeyLastUsage` only reports it; and each key
//! keeps its own record, including each key of a multi-Region key set.
//!
//! The table's rows are EVERY operation the model tracks a use for
//! (`smithy_query(query="shape",shape="KeyLastUsageTrackingOperation")`), so they
//! differ in the operation, in the kind of key it needs (symmetric encryption, HMAC,
//! asymmetric signing, key agreement) and in whether the operation writes a record at
//! all today: one still answering the scaffold 501 must leave the record exactly as it
//! was. A row whose request is built from another tracked operation's answer is
//! skipped while that producer is a scaffold stub.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state::REPLICA_CREATING_SPAN;

use super::harness::{b64, call, create_key, ok, ok_in, router, text, REGION};

/// Every value of the model's `KeyLastUsageTrackingOperation` enum
/// (`smithy_query(query="shape",shape="KeyLastUsageTrackingOperation")`): the
/// operations a successful call of which is recorded as a use of the key.
const TRACKED_OPERATIONS: [&str; 12] = [
    "Decrypt",
    "DeriveSharedSecret",
    "Encrypt",
    "GenerateDataKey",
    "GenerateDataKeyPair",
    "GenerateDataKeyPairWithoutPlaintext",
    "GenerateDataKeyWithoutPlaintext",
    "GenerateMac",
    "ReEncrypt",
    "Sign",
    "Verify",
    "VerifyMac",
];

/// The `CreateKey` request for the key `operation` needs: the `KeySpec` and `KeyUsage`
/// that operation's key must have
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")`).
fn key_for(operation: &str) -> Value {
    match operation {
        "Sign" | "Verify" => {
            json!({"Description": operation, "KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"})
        }
        "GenerateMac" | "VerifyMac" => json!({
            "Description": operation,
            "KeySpec": "HMAC_256",
            "KeyUsage": "GENERATE_VERIFY_MAC"
        }),
        "DeriveSharedSecret" => json!({
            "Description": operation,
            "KeySpec": "ECC_NIST_P256",
            "KeyUsage": "KEY_AGREEMENT"
        }),
        // Everything else uses the symmetric encryption key `CreateKey` mints by
        // default.
        _ => json!({"Description": operation}),
    }
}

/// The message every row signs, MACs or seals.
const MESSAGE: &[u8] = b"the last usage record";

/// A request that performs `operation` on `key_id`, or `None` when the input it needs
/// is another tracked operation's answer and that operation is still a scaffold stub.
async fn request_for(
    router: &smithy_server_runtime::MultiServiceRouter,
    operation: &str,
    key_id: &str,
) -> Option<Value> {
    match operation {
        "Encrypt" => Some(json!({"KeyId": key_id, "Plaintext": b64(MESSAGE)})),
        "Decrypt" => Some(json!({"CiphertextBlob": sealed(router, key_id).await})),
        "ReEncrypt" => Some(json!({
            "CiphertextBlob": sealed(router, key_id).await,
            "DestinationKeyId": key_id
        })),
        "GenerateDataKey" => Some(json!({"KeyId": key_id, "KeySpec": "AES_256"})),
        "GenerateDataKeyWithoutPlaintext" => Some(json!({"KeyId": key_id, "NumberOfBytes": 32})),
        // The smallest `DataKeyPairSpec`
        // (`smithy_query(query="shape",shape="DataKeyPairSpec")`), so a row generating
        // a pair costs the least.
        "GenerateDataKeyPair" | "GenerateDataKeyPairWithoutPlaintext" => {
            Some(json!({"KeyId": key_id, "KeyPairSpec": "ECC_NIST_P256"}))
        }
        "Sign" => Some(sign_request(key_id)),
        "Verify" => {
            let signed = answer(router, "Sign", &sign_request(key_id)).await?;
            Some(json!({
                "KeyId": key_id,
                "Message": b64(MESSAGE),
                "MessageType": "RAW",
                "Signature": signed["Signature"].clone(),
                "SigningAlgorithm": "RSASSA_PSS_SHA_256"
            }))
        }
        "GenerateMac" => Some(mac_request(key_id)),
        "VerifyMac" => {
            let generated = answer(router, "GenerateMac", &mac_request(key_id)).await?;
            Some(json!({
                "KeyId": key_id,
                "Message": b64(MESSAGE),
                "MacAlgorithm": "HMAC_SHA_256",
                "Mac": generated["Mac"].clone()
            }))
        }
        "DeriveSharedSecret" => Some(json!({
            "KeyId": key_id,
            "KeyAgreementAlgorithm": "ECDH",
            "PublicKey": peer_public_key(router).await?
        })),
        other => panic!("case {other}: the table has no request for this tracked operation"),
    }
}

/// `Sign` over [`MESSAGE`] with an algorithm an `RSA_2048` signing key supports
/// (`smithy_query(query="shape",shape="SigningAlgorithmSpec")`).
fn sign_request(key_id: &str) -> Value {
    json!({
        "KeyId": key_id,
        "Message": b64(MESSAGE),
        "MessageType": "RAW",
        "SigningAlgorithm": "RSASSA_PSS_SHA_256"
    })
}

/// `GenerateMac` over [`MESSAGE`] with the algorithm an `HMAC_256` key supports
/// (`smithy_query(query="shape",shape="MacAlgorithmSpec")`).
fn mac_request(key_id: &str) -> Value {
    json!({
        "KeyId": key_id,
        "Message": b64(MESSAGE),
        "MacAlgorithm": "HMAC_SHA_256"
    })
}

/// A ciphertext sealed under `key_id`, for the rows whose input is one.
async fn sealed(router: &smithy_server_runtime::MultiServiceRouter, key_id: &str) -> Value {
    ok(
        router,
        "Encrypt",
        &json!({"KeyId": key_id, "Plaintext": b64(MESSAGE)}),
    )
    .await["CiphertextBlob"]
        .clone()
}

/// A DER-encoded X.509 public key of a `ECC_NIST_P256` pair, as the peer public key a
/// key-agreement request carries. It is the public half of a data key pair, so it comes
/// from the service rather than from bytes written here.
async fn peer_public_key(router: &smithy_server_runtime::MultiServiceRouter) -> Option<Value> {
    let metadata = create_key(router, &json!({"Description": "key agreement peer"})).await;
    let key_id = text("peer", &metadata, "KeyId");
    let pair = answer(
        router,
        "GenerateDataKeyPairWithoutPlaintext",
        &json!({"KeyId": key_id, "KeyPairSpec": "ECC_NIST_P256"}),
    )
    .await?;
    Some(pair["PublicKey"].clone())
}

/// The answer of a call that is allowed to be a scaffold stub: `Some` when it
/// succeeded, `None` when the operation is still unimplemented. Any other failure is
/// the test's own request being wrong, so it panics.
async fn answer(
    router: &smithy_server_runtime::MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> Option<Value> {
    let (status, payload) = call(router, REGION, operation, body).await;
    match status {
        200 => Some(payload),
        501 => None,
        other => panic!("case {operation}: expected HTTP 200 or 501, got {other} with {payload}"),
    }
}

/// The last-usage record `GetKeyLastUsage` reports for `key_id` at `region`'s endpoint,
/// together with the whole answer the assertions report.
async fn last_usage_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    key_id: &str,
) -> Value {
    ok_in(router, region, "GetKeyLastUsage", &json!({"KeyId": key_id})).await
}

/// [`last_usage_in`] at [`REGION`].
async fn last_usage(router: &smithy_server_runtime::MultiServiceRouter, key_id: &str) -> Value {
    last_usage_in(router, REGION, key_id).await
}

/// Assert the record in `reported` is one `operation` just wrote: it names that
/// operation, carries ids of its own, and its timestamp is at or after `floor` and the
/// record it replaced.
fn assert_fresh_record(
    case: &str,
    operation: &str,
    reported: &Value,
    previous: &Value,
    floor: f64,
) {
    let usage = &reported["KeyLastUsage"];
    assert_eq!(
        usage["Operation"].as_str().unwrap_or_default(),
        operation,
        "case {case}: expected the record to name {operation}, got {reported}"
    );
    let timestamp = usage["Timestamp"].as_f64().unwrap_or_default();
    assert!(
        timestamp >= floor,
        "case {case}: expected a timestamp at or after {floor}, got {reported}"
    );
    let previous_timestamp = previous["KeyLastUsage"]["Timestamp"]
        .as_f64()
        .unwrap_or_default();
    assert!(
        timestamp >= previous_timestamp,
        "case {case}: the timestamp must not move backwards from {previous_timestamp}, got \
         {timestamp}"
    );
    for member in ["CloudTrailEventId", "KmsRequestId"] {
        let value = usage[member].as_str().unwrap_or_default();
        assert!(
            !value.is_empty(),
            "case {case}: expected a {member} on the record, got {reported}"
        );
        let before = previous["KeyLastUsage"][member].as_str().unwrap_or_default();
        assert_ne!(
            value, before,
            "case {case}: expected a {member} of this call's own, got the previous one"
        );
    }
}

/// The rule: every tracked cryptographic operation that succeeds replaces the key's
/// last-usage record with one naming itself, a call that is refused as unimplemented
/// writes nothing, and reading the record never changes it.
#[tokio::test]
async fn every_tracked_cryptographic_operation_writes_the_key_last_usage_record() {
    let router = router();

    // The table covers the model's catalog of tracked operations, with no row invented
    // beside it.
    let covered: BTreeSet<&str> = TRACKED_OPERATIONS.iter().copied().collect();
    assert_eq!(
        covered.len(),
        TRACKED_OPERATIONS.len(),
        "case table: the tracked operations must be distinct, got {TRACKED_OPERATIONS:?}"
    );

    let mut implemented: Vec<&str> = Vec::new();
    for operation in TRACKED_OPERATIONS {
        let metadata = create_key(&router, &key_for(operation)).await;
        let key_id = text(operation, &metadata, "KeyId");
        let key_arn = text(operation, &metadata, "Arn");
        let creation_date = metadata["CreationDate"].as_f64().unwrap_or_default();

        // A key that has had no cryptographic operation reports no usage at all, and
        // its tracking starts when it was created.
        let fresh = last_usage(&router, &key_id).await;
        assert_eq!(
            fresh["KeyId"].as_str().unwrap_or_default(),
            key_arn,
            "case {operation}: the answer names the key by its ARN, got {fresh}"
        );
        assert!(
            fresh["KeyLastUsage"].is_null(),
            "case {operation}: a key with no recorded use reports no KeyLastUsage, got {fresh}"
        );
        assert_eq!(
            fresh["TrackingStartDate"].as_f64().unwrap_or_default(),
            creation_date,
            "case {operation}: tracking starts when the key is created, got {fresh}"
        );

        let Some(request) = request_for(&router, operation, &key_id).await else {
            // The input this row needs comes from an operation that is still a stub.
            continue;
        };
        // Read the record only once the request is built: building it may itself have
        // used the key.
        let before = last_usage(&router, &key_id).await;

        let (status, payload) = call(&router, REGION, operation, &request).await;
        if status == 501 {
            // A scaffold stub performed no cryptographic operation, so it recorded
            // none.
            let after = last_usage(&router, &key_id).await;
            assert_eq!(
                after, before,
                "case {operation}: an unimplemented operation must not be recorded as a use \
                 of the key"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {operation}: expected HTTP 200 or 501, got {status} with {payload}"
        );
        implemented.push(operation);

        let reported = last_usage(&router, &key_id).await;
        assert_fresh_record(operation, operation, &reported, &before, creation_date);

        // Reading the record does not change it: asking again answers the same.
        let again = last_usage(&router, &key_id).await;
        assert_eq!(
            again, reported,
            "case {operation}: a second read must answer the same record"
        );

        // Calling the operation again replaces the record with the later call's, which
        // keeps the operation and moves neither the timestamp backwards nor the ids.
        ok(&router, operation, &request).await;
        let second = last_usage(&router, &key_id).await;
        assert_fresh_record(operation, operation, &second, &reported, creation_date);
    }

    // The suite is only evidence of the rule if some row actually performed a
    // cryptographic operation.
    assert!(
        !implemented.is_empty(),
        "case table: no tracked operation was served, so nothing was recorded"
    );
}

/// The rule, continued: each key keeps its OWN record, so the keys of a multi-Region
/// key set — which share a key id — track their last usage independently.
#[tokio::test]
async fn each_key_of_a_multi_region_set_tracks_its_own_last_usage() {
    let replica_region = "eu-west-2";
    let router = router();
    let metadata = create_key(
        &router,
        &json!({"Description": "usage per key", "MultiRegion": true}),
    )
    .await;
    let key_id = text("setup", &metadata, "KeyId");
    let creation_date = metadata["CreationDate"].as_f64().unwrap_or_default();

    ok(
        &router,
        "ReplicateKey",
        &json!({"KeyId": key_id, "ReplicaRegion": replica_region}),
    )
    .await;
    // The replica reaches the caller in the transient `Creating` key state and the
    // clock alone ends it, so wait for the key it was created to be.
    let deadline = std::time::Instant::now() + REPLICA_CREATING_SPAN * 20;
    loop {
        let replica = ok_in(
            &router,
            replica_region,
            "DescribeKey",
            &json!({"KeyId": key_id}),
        )
        .await["KeyMetadata"]
            .clone();
        if replica["KeyState"] != json!("Creating") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case replica: the transient Creating key state must end by itself, got {replica}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // Use the PRIMARY key only: the replica shares its key id, and must still report no
    // usage of its own.
    ok(
        &router,
        "Encrypt",
        &json!({"KeyId": key_id, "Plaintext": b64(MESSAGE)}),
    )
    .await;
    let primary = last_usage(&router, &key_id).await;
    assert_fresh_record(
        "primary used",
        "Encrypt",
        &primary,
        &json!({"KeyLastUsage": Value::Null}),
        creation_date,
    );
    let replica = last_usage_in(&router, replica_region, &key_id).await;
    assert!(
        replica["KeyLastUsage"].is_null(),
        "case replica unused: a use of the primary key is not a use of the replica, got {replica}"
    );

    // And the other way round: using the replica leaves the primary's record alone.
    ok_in(
        &router,
        replica_region,
        "GenerateDataKey",
        &json!({"KeyId": key_id, "KeySpec": "AES_256"}),
    )
    .await;
    let replica_used = last_usage_in(&router, replica_region, &key_id).await;
    assert_fresh_record(
        "replica used",
        "GenerateDataKey",
        &replica_used,
        &json!({"KeyLastUsage": Value::Null}),
        creation_date,
    );
    let primary_again = last_usage(&router, &key_id).await;
    assert_eq!(
        primary_again, primary,
        "case primary unchanged: a use of the replica must not touch the primary's record"
    );
}

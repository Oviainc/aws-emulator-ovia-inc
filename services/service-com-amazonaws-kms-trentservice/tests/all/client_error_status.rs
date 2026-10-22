// SPDX-License-Identifier: CC0-1.0

//! The HTTP status and the resource wording this service puts on a refusal, driven
//! end-to-end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: TrentService.<Operation>`) through the same multi-service router
//! the emulator boots.
//!
//! Three rules, each asserted over a table rather than one call:
//!
//! * TrentService answers EVERY client error with the `awsJson1_1` client status,
//!   HTTP 400, and distinguishes the error only by the body's `__type`; a server
//!   error keeps the status the model gives it. The rows span the four distinct
//!   `@httpError` values the model records for client errors (400, 404, 409, 412 —
//!   `smithy_query(query="operation",operation="DescribeKey,Encrypt,Decrypt,ScheduleKeyDeletion")`
//!   and `smithy_query(query="shape",shape="DisabledException,DryRunOperationException")`),
//!   over control-plane and data-plane operations alike.
//! * A `KeyId` lookup that found nothing is reported as the resource the request
//!   asked for, named by its ARN: an alias as an alias, a key as a key — unless the
//!   identifier is an ARN pinned to another Region, which this endpoint can never
//!   serve, and which is reported by naming that Region.
//! * A refusal caused by the key's own key state names the key by its ARN and that
//!   state as prose.

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::scaffolded::wire_status;

// The wire plumbing lives in `tests/all/harness.rs`, shared with every other suite
// in this crate.
use super::harness::{b64, call, create_key, error_code, ok, router, text, REGION};

/// The `message` an error body carries. The harness reports the `__type`; only this
/// suite asserts on the wording, so the accessor lives here.
fn error_message(payload: &Value) -> String {
    payload["message"]
        .as_str()
        .or_else(|| payload["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Create a key with `description` and return its `KeyMetadata`.
async fn described_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    description: &str,
) -> Value {
    create_key(router, &json!({"Description": description})).await
}

/// Every refusal this service can be driven into answers HTTP 400 — whatever
/// `@httpError` the model puts on the error shape — and stays distinguishable by its
/// `__type`.
///
/// The rows span the modelled client statuses (400 `InvalidArnException` and friends,
/// 404 `NotFoundException`, 409 `KMSInvalidStateException` / `DisabledException`, 412
/// `DryRunOperationException`), the control plane and the data plane, and both the
/// modelled errors and the framework's own `ValidationException`.
#[tokio::test]
async fn every_client_error_answers_http_400_with_its_own_type() {
    let router = router();
    let account = engine::emulator_account_id();

    let enabled = described_key(&router, "enabled").await;
    let enabled_id = text("setup", &enabled, "KeyId");
    let other = described_key(&router, "other").await;
    let other_id = text("setup", &other, "KeyId");
    let disabled = described_key(&router, "disabled").await;
    let disabled_id = text("setup", &disabled, "KeyId");
    let pending = described_key(&router, "pending").await;
    let pending_id = text("setup", &pending, "KeyId");

    // A ciphertext of each key, taken while every key is still usable.
    let sealed = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": enabled_id, "Plaintext": b64(b"hello kms")}),
    )
    .await;
    let blob = text("setup", &sealed, "CiphertextBlob");
    let sealed_by_disabled = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": disabled_id, "Plaintext": b64(b"hello kms")}),
    )
    .await;
    let disabled_blob = text("setup", &sealed_by_disabled, "CiphertextBlob");
    let sealed_by_pending = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": pending_id, "Plaintext": b64(b"hello kms")}),
    )
    .await;
    let pending_blob = text("setup", &sealed_by_pending, "CiphertextBlob");

    ok(&router, "DisableKey", &json!({"KeyId": disabled_id})).await;
    ok(&router, "ScheduleKeyDeletion", &json!({"KeyId": pending_id})).await;

    // Rows: the case, the operation, the request, the error code expected, and the
    // status the MODEL puts on that error shape — the value the wire replaces with
    // 400.
    let cases: Vec<(&str, &str, Value, &str, u16)> = vec![
        (
            "describe an unknown key id",
            "DescribeKey",
            json!({"KeyId": "00000000-0000-0000-0000-000000000000"}),
            "NotFoundException",
            404,
        ),
        (
            "describe an unknown alias",
            "DescribeKey",
            json!({"KeyId": "alias/no-such-alias"}),
            "NotFoundException",
            404,
        ),
        (
            "describe an ARN that is not a KMS key ARN",
            "DescribeKey",
            json!({"KeyId": format!("arn:aws:kms:{REGION}:{account}:thing/abc")}),
            "InvalidArnException",
            400,
        ),
        (
            "schedule a key already pending deletion",
            "ScheduleKeyDeletion",
            json!({"KeyId": pending_id}),
            "KMSInvalidStateException",
            409,
        ),
        (
            "disable a key pending deletion",
            "DisableKey",
            json!({"KeyId": pending_id}),
            "KMSInvalidStateException",
            409,
        ),
        (
            "enable a key pending deletion",
            "EnableKey",
            json!({"KeyId": pending_id}),
            "KMSInvalidStateException",
            409,
        ),
        (
            "update the description of a key pending deletion",
            "UpdateKeyDescription",
            json!({"KeyId": pending_id, "Description": "nope"}),
            "KMSInvalidStateException",
            409,
        ),
        (
            "cancel a deletion that was never scheduled",
            "CancelKeyDeletion",
            json!({"KeyId": enabled_id}),
            "KMSInvalidStateException",
            409,
        ),
        (
            "encrypt under a key pending deletion",
            "Encrypt",
            json!({"KeyId": pending_id, "Plaintext": b64(b"hello kms")}),
            "KMSInvalidStateException",
            409,
        ),
        (
            "decrypt a ciphertext of a key pending deletion",
            "Decrypt",
            json!({"CiphertextBlob": pending_blob}),
            "KMSInvalidStateException",
            409,
        ),
        (
            "encrypt under a disabled key",
            "Encrypt",
            json!({"KeyId": disabled_id, "Plaintext": b64(b"hello kms")}),
            "DisabledException",
            409,
        ),
        (
            "decrypt a ciphertext of a disabled key",
            "Decrypt",
            json!({"CiphertextBlob": disabled_blob}),
            "DisabledException",
            409,
        ),
        (
            "encrypt as a dry run",
            "Encrypt",
            json!({"KeyId": enabled_id, "Plaintext": b64(b"hello kms"), "DryRun": true}),
            "DryRunOperationException",
            412,
        ),
        (
            "decrypt as a dry run",
            "Decrypt",
            json!({"CiphertextBlob": blob, "DryRun": true}),
            "DryRunOperationException",
            412,
        ),
        (
            "encrypt with a grant token KMS never issued",
            "Encrypt",
            json!({"KeyId": enabled_id, "Plaintext": b64(b"hello kms"),
                   "GrantTokens": ["not-a-token"]}),
            "InvalidGrantTokenException",
            400,
        ),
        (
            "encrypt with an algorithm the key spec does not support",
            "Encrypt",
            json!({"KeyId": enabled_id, "Plaintext": b64(b"hello kms"),
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "InvalidKeyUsageException",
            400,
        ),
        (
            "decrypt bytes that are not a ciphertext",
            "Decrypt",
            json!({"CiphertextBlob": b64(&[0u8; 64])}),
            "InvalidCiphertextException",
            400,
        ),
        (
            "decrypt naming a key that did not seal it",
            "Decrypt",
            json!({"CiphertextBlob": blob, "KeyId": other_id}),
            "IncorrectKeyException",
            400,
        ),
        (
            // `CreateKey` mints every modelled KeySpec now; what it still refuses is
            // an Origin that needs a custom key store, which is the same modelled
            // 400 this row is here to exercise.
            "create a key whose origin needs a custom key store",
            "CreateKey",
            json!({"Origin": "AWS_CLOUDHSM"}),
            "UnsupportedOperationException",
            400,
        ),
        (
            "create a key usage its key spec does not support",
            "CreateKey",
            json!({"KeySpec": "RSA_2048", "KeyUsage": "GENERATE_VERIFY_MAC"}),
            "ValidationException",
            400,
        ),
        (
            "list keys from a marker this service never minted",
            "ListKeys",
            json!({"Marker": "not-a-marker"}),
            "InvalidMarkerException",
            400,
        ),
        (
            "schedule a waiting period outside the documented window",
            "ScheduleKeyDeletion",
            json!({"KeyId": enabled_id, "PendingWindowInDays": 1}),
            "ValidationException",
            400,
        ),
    ];

    for (case, operation, body, expected_code, modeled_status) in cases {
        let (status, payload) = call(&router, REGION, operation, &body).await;
        let code = error_code(&payload);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 (the model says {modeled_status}), \
             got {status} with {payload}"
        );
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected __type {expected_code}, got {code:?} in {payload}"
        );
        assert!(
            !error_message(&payload).is_empty(),
            "case {case:?}: an error must say what was refused, got {payload}"
        );
    }
}

/// The status mapping collapses the client band and nothing else: a server error
/// keeps the status the model gives it, and the scaffold's own 501 is untouched.
///
/// The catalog is the set of statuses the KMS model actually uses — client 400, 404,
/// 409, 412 and server 500 (`KMSInternalException`, `KeyUnavailableException`), 503
/// (`DependencyTimeoutException`), from
/// `smithy_query(query="shape",shape="DisabledException,DryRunOperationException,KeyUnavailableException,NotFoundException")`
/// — plus both edges of the client band (400, 499) and one status past each edge
/// (399, 500).
#[test]
fn the_status_mapping_collapses_only_the_client_band() {
    // Rows: a modelled status, and the status the wire carries for it.
    let cases: [(u16, u16); 10] = [
        (400, 400),
        (404, 400),
        (409, 400),
        (412, 400),
        (429, 400),
        (499, 400),
        (399, 399),
        (500, 500),
        (503, 503),
        (501, 501),
    ];
    for (modeled, expected) in cases {
        let actual = wire_status(modeled);
        assert_eq!(
            actual, expected,
            "case modelled {modeled}: expected HTTP {expected} on the wire, got {actual}"
        );
    }
}

/// An operation that is still a stub keeps answering HTTP 501: the client-error
/// mapping must not swallow the scaffold signal, which is a different claim from a
/// refusal.
#[tokio::test]
async fn a_scaffolded_operation_still_answers_501() {
    let router = router();
    let metadata = described_key(&router, "stub census").await;
    let key_id = text("setup", &metadata, "KeyId");

    // Rows: operations this crate has not implemented yet, control plane and data
    // plane (`grep -l 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/`).
    let cases: Vec<(&str, Value)> = vec![
        (
            "CreateAlias",
            json!({"AliasName": "alias/demo", "TargetKeyId": key_id}),
        ),
        ("GetKeyPolicy", json!({"KeyId": key_id, "PolicyName": "default"})),
        ("ListAliases", json!({})),
        (
            "GenerateDataKey",
            json!({"KeyId": key_id, "KeySpec": "AES_256"}),
        ),
    ];
    for (operation, body) in cases {
        let (status, payload) = call(&router, REGION, operation, &body).await;
        let code = error_code(&payload);
        assert_eq!(
            status, 501,
            "case {operation}: a stub must answer HTTP 501, got {status} with {payload}"
        );
        assert_eq!(
            code, "NotImplemented",
            "case {operation}: expected __type NotImplemented, got {code:?} in {payload}"
        );
    }
}

/// A `KeyId` lookup that found nothing names the resource the request asked for, by
/// its ARN: an alias that resolves to no key is reported AS an alias, and a key id or
/// key ARN as a key. An ARN pinned to ANOTHER Region is reported by naming that
/// Region, because no endpoint of this one can serve it.
///
/// The rows differ in the form of the identifier (bare name and ARN), in the alias
/// name (the characters `AliasNameType`'s `^[a-zA-Z0-9:/_-]+$` pattern allows:
/// letters, digits, `/`, `_`, `-`, `:`), in the Region the request is signed for, and
/// in the kind of identifier (alias versus key), and cover every operation that
/// resolves a `KeyId` and models `NotFoundException`.
#[tokio::test]
async fn a_failed_key_id_lookup_names_the_resource_the_request_asked_for() {
    let router = router();
    let account = engine::emulator_account_id();

    // Rows: the case, the Region signed for, the identifier, and the message
    // expected. An alias ARN in the request is reported verbatim; a bare
    // `alias/<name>` is reported as the alias ARN of the endpoint it was sent to.
    let mut cases: Vec<(String, &str, String, String)> = Vec::new();
    for (region, name) in [
        (REGION, "no-such-alias"),
        (REGION, "team/project/key"),
        ("us-west-2", "Alias_1-2:3"),
        ("eu-west-1", "a"),
    ] {
        let alias = format!("alias/{name}");
        let alias_arn = format!("arn:aws:kms:{region}:{account}:{alias}");
        cases.push((
            format!("bare {alias} in {region}"),
            region,
            alias.clone(),
            format!("Alias {alias_arn} is not found."),
        ));
        cases.push((
            format!("alias ARN {alias_arn}"),
            region,
            alias_arn.clone(),
            format!("Alias {alias_arn} is not found."),
        ));
    }
    // An ARN of another Region cannot be served by the endpoint it was sent to at
    // all, so the refusal names that Region rather than a resource of this one —
    // the form recorded on the wire, `NotFoundException: Invalid arn <region>`.
    let foreign = format!("arn:aws:kms:ap-south-1:{account}:alias/elsewhere");
    cases.push((
        format!("alias ARN of another Region {foreign}"),
        REGION,
        foreign.clone(),
        "Invalid arn ap-south-1".to_string(),
    ));
    // A key identifier is NOT an alias: it is reported as a key, by its key ARN.
    for (region, key_id) in [
        (REGION, "00000000-0000-0000-0000-000000000000"),
        ("us-west-2", "11111111-2222-3333-4444-555555555555"),
        ("eu-west-1", "mrk-00000000000000000000000000000000"),
    ] {
        let key_arn = format!("arn:aws:kms:{region}:{account}:key/{key_id}");
        cases.push((
            format!("bare key id {key_id} in {region}"),
            region,
            key_id.to_string(),
            format!("Key '{key_arn}' does not exist"),
        ));
        cases.push((
            format!("key ARN {key_arn}"),
            region,
            key_arn.clone(),
            format!("Key '{key_arn}' does not exist"),
        ));
    }
    // As above, for a key ARN: the Region it names is what the refusal reports.
    let foreign_key = format!("arn:aws:kms:ap-south-1:{account}:key/{}", "0".repeat(8));
    cases.push((
        format!("key ARN of another Region {foreign_key}"),
        REGION,
        foreign_key.clone(),
        "Invalid arn ap-south-1".to_string(),
    ));

    for (case, region, identifier, expected) in cases {
        for operation in [
            "DescribeKey",
            "ScheduleKeyDeletion",
            "DisableKey",
            "EnableKey",
            "CancelKeyDeletion",
            "UpdateKeyDescription",
            "Encrypt",
        ] {
            let mut body = json!({"KeyId": identifier});
            if operation == "Encrypt" {
                body["Plaintext"] = json!(b64(b"hello kms"));
            }
            if operation == "UpdateKeyDescription" {
                body["Description"] = json!("nowhere");
            }
            let (status, payload) = call(&router, region, operation, &body).await;
            let code = error_code(&payload);
            let message = error_message(&payload);
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?} on {operation}: expected NotFoundException, got {code:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected the message to name the resource the \
                 request did ({expected:?}), got {message:?}"
            );
        }
    }
}

/// A refusal caused by the key's own key state names the key by its ARN and the state
/// in question as prose: an operation refused because of the state the key is IN names
/// that state ("pending deletion", "disabled"), and `CancelKeyDeletion`, which needs
/// the one `PendingDeletion` state, names the state the key is missing ("not pending
/// deletion") — every value worded from `KeyState` itself
/// (`smithy_query(query="shape",shape="KeyState")`).
///
/// The rows span the operations that refuse on key state and the states they refuse
/// in, so one wording covers them all.
#[tokio::test]
async fn a_state_refusal_names_the_key_and_its_state() {
    let router = router();

    let enabled = described_key(&router, "enabled").await;
    let enabled_arn = text("setup", &enabled, "Arn");
    let disabled = described_key(&router, "disabled").await;
    let disabled_arn = text("setup", &disabled, "Arn");
    ok(&router, "DisableKey", &json!({"KeyId": disabled_arn})).await;
    let pending = described_key(&router, "pending").await;
    let pending_arn = text("setup", &pending, "Arn");
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": pending_arn, "PendingWindowInDays": 7}),
    )
    .await;

    // Rows: the call, the key it names, and the state prose the refusal must carry.
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "schedule a key pending deletion",
            "ScheduleKeyDeletion",
            json!({"KeyId": pending_arn, "PendingWindowInDays": 7}),
            &pending_arn,
            "pending deletion",
        ),
        (
            "disable a key pending deletion",
            "DisableKey",
            json!({"KeyId": pending_arn}),
            &pending_arn,
            "pending deletion",
        ),
        (
            "enable a key pending deletion",
            "EnableKey",
            json!({"KeyId": pending_arn}),
            &pending_arn,
            "pending deletion",
        ),
        (
            "update the description of a key pending deletion",
            "UpdateKeyDescription",
            json!({"KeyId": pending_arn, "Description": "nope"}),
            &pending_arn,
            "pending deletion",
        ),
        (
            "encrypt under a key pending deletion",
            "Encrypt",
            json!({"KeyId": pending_arn, "Plaintext": b64(b"hello kms")}),
            &pending_arn,
            "pending deletion",
        ),
        (
            "cancel a deletion on an enabled key",
            "CancelKeyDeletion",
            json!({"KeyId": enabled_arn}),
            &enabled_arn,
            "not pending deletion",
        ),
        (
            "cancel a deletion on a disabled key",
            "CancelKeyDeletion",
            json!({"KeyId": disabled_arn}),
            &disabled_arn,
            "not pending deletion",
        ),
        (
            "encrypt under a disabled key",
            "Encrypt",
            json!({"KeyId": disabled_arn, "Plaintext": b64(b"hello kms")}),
            &disabled_arn,
            "disabled",
        ),
    ];

    for (case, operation, body, arn, prose) in cases {
        let (status, payload) = call(&router, REGION, operation, &body).await;
        let message = error_message(&payload);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            message,
            format!("{arn} is {prose}."),
            "case {case:?}: expected the refusal to name the key and its state, got {message:?}"
        );
    }
}

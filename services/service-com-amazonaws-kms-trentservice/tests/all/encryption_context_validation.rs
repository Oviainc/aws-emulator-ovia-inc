// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the encryption context as INPUT, driven over the wire through
//! the shared [`super::harness`].
//!
//! The rule under test: an encryption context is "a collection of non-secret key-value
//! pairs" (`smithy_query(query="doc",shape="EncryptRequest",member="EncryptionContext")`),
//! so an entry whose name or whose value is the empty string makes the REQUEST
//! malformed — refused with `ValidationException` and the message
//! `"Invalid EncryptionContext"` by every operation that takes an encryption context,
//! before any ciphertext is authenticated against it.
//!
//! That ordering is what the second test pins: a context that is WELL FORMED but not
//! the one a ciphertext was sealed under is an authentication failure
//! (`InvalidCiphertextException`), which only an operation that opens a ciphertext can
//! report at all — so the two faults stay distinguishable, and the operations that
//! merely seal never answer the authentication one.
//!
//! The rows span every operation that accepts an encryption context (both of
//! `ReEncrypt`'s), the position of the offending entry (its name, its value, both), the
//! number of entries beside it, and the forms of a legal context — the recorded case is
//! one row of the first table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    b64, call, create_key, error_code, error_message, ok, router, text, REGION,
};

/// The whole of the message KMS words this refusal with: fixed text, with no variable
/// part naming the entry or the value.
const INVALID_CONTEXT_MESSAGE: &str = "Invalid EncryptionContext";

/// The encryption contexts that are not legal encryption contexts: an entry whose
/// value is empty, whose name is empty, or both, alone and beside well-formed entries.
fn malformed_contexts() -> Vec<(&'static str, Value)> {
    vec![
        ("one entry with an empty value", json!({"purpose": ""})),
        (
            "an empty value beside a well-formed entry",
            json!({"tenant": "acme", "purpose": ""}),
        ),
        (
            "an empty value among several entries",
            json!({"tenant": "acme", "purpose": "", "stage": "prod"}),
        ),
        ("one entry with an empty name", json!({"": "acme"})),
        (
            "an empty name beside a well-formed entry",
            json!({"tenant": "acme", "": "invoice"})
        ),
        ("an entry that is empty on both sides", json!({"": ""})),
        (
            "an empty name and an empty value in one context",
            json!({"": "acme", "purpose": ""}),
        ),
    ]
}

/// A request body for `operation` carrying `context` as its encryption context, over a
/// key and a ciphertext the operation can otherwise serve.
fn request(operation: &str, key_arn: &str, blob: &str, context: &Value) -> Value {
    let plaintext = b64(b"aad-bound-payload");
    match operation {
        "Encrypt" => json!({"KeyId": key_arn, "Plaintext": plaintext,
                            "EncryptionContext": context}),
        "Decrypt" => json!({"CiphertextBlob": blob, "KeyId": key_arn,
                            "EncryptionContext": context}),
        "GenerateDataKey" | "GenerateDataKeyWithoutPlaintext" => {
            json!({"KeyId": key_arn, "KeySpec": "AES_256", "EncryptionContext": context})
        }
        "GenerateDataKeyPair" | "GenerateDataKeyPairWithoutPlaintext" => {
            json!({"KeyId": key_arn, "KeyPairSpec": "ECC_NIST_P256",
                   "EncryptionContext": context})
        }
        // Each half of a re-encryption carries its own context, and each is input.
        "ReEncrypt/source" => json!({"CiphertextBlob": blob, "DestinationKeyId": key_arn,
                                     "SourceEncryptionContext": context}),
        "ReEncrypt/destination" => json!({"CiphertextBlob": blob,
                                          "DestinationKeyId": key_arn,
                                          "DestinationEncryptionContext": context}),
        other => panic!("case {other}: not an operation this suite drives"),
    }
}

/// The operation an entry of [`request`] calls on the wire.
fn wire_operation(operation: &str) -> &str {
    operation.split('/').next().unwrap_or(operation)
}

/// A symmetric encryption key and a ciphertext it sealed with no encryption context, so
/// a row's own context is the only thing wrong with its request.
async fn key_and_ciphertext(router: &MultiServiceRouter) -> (String, String) {
    let metadata = create_key(router, &json!({"Description": "encryption context"})).await;
    let key_arn = text("setup", &metadata, "Arn");
    let sealed = ok(
        router,
        "Encrypt",
        &json!({"KeyId": key_arn, "Plaintext": b64(b"aad-bound-payload")}),
    )
    .await;
    let blob = text("setup", &sealed, "CiphertextBlob");
    (key_arn, blob)
}

/// Every operation that takes an encryption context refuses one that is not a legal
/// encryption context as malformed INPUT, with the one message KMS words it with.
#[tokio::test]
async fn an_empty_encryption_context_name_or_value_is_invalid_input() {
    let router = router();
    let (key_arn, blob) = key_and_ciphertext(&router).await;

    for operation in [
        "Encrypt",
        "Decrypt",
        "GenerateDataKey",
        "GenerateDataKeyWithoutPlaintext",
        "GenerateDataKeyPair",
        "GenerateDataKeyPairWithoutPlaintext",
        "ReEncrypt/source",
        "ReEncrypt/destination",
    ] {
        for (described, context) in malformed_contexts() {
            let case = format!("{operation} with {described}");
            let (status, payload) = call(
                &router,
                REGION,
                wire_operation(operation),
                &request(operation, &key_arn, &blob, &context),
            )
            .await;
            let code = error_code(&payload);
            let message = error_message(&payload);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: a malformed encryption context is invalid input, got \
                 {code:?} in {payload}"
            );
            assert_eq!(
                message, INVALID_CONTEXT_MESSAGE,
                "case {case:?}: expected the fixed refusal text, got {message:?}"
            );
        }
    }
}

/// A WELL-FORMED encryption context is judged by the ciphertext's authentication tag,
/// not by input validation: the one that sealed the ciphertext opens it, and any other
/// one fails authentication — a different fault from a malformed context, and one the
/// operations that only seal never answer.
#[tokio::test]
async fn a_well_formed_encryption_context_is_authenticated_rather_than_validated() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "aad binding"})).await;
    let key_arn = text("setup", &metadata, "Arn");
    let plaintext = b"aad-bound-payload".to_vec();

    // Rows: a legal encryption context to seal under, differing in entry count, in the
    // characters its names and values use, and in length.
    let bound: Vec<(&str, Value)> = vec![
        ("no context at all", json!(null)),
        ("one entry", json!({"tenant": "acme"})),
        (
            "two entries",
            json!({"tenant": "acme", "purpose": "invoice"}),
        ),
        (
            "entries whose values differ only in case",
            json!({"tenant": "ACME", "Tenant": "acme"}),
        ),
        (
            "a long value",
            json!({"tenant": "a".repeat(256), "purpose": "invoice/2026-09"}),
        ),
    ];
    for (described, context) in bound {
        let mut seal = json!({"KeyId": key_arn, "Plaintext": b64(&plaintext)});
        if !context.is_null() {
            seal["EncryptionContext"] = context.clone();
        }
        let sealed = ok(&router, "Encrypt", &seal).await;
        let blob = text(described, &sealed, "CiphertextBlob");

        // The same context opens it.
        let mut open = json!({"CiphertextBlob": blob, "KeyId": key_arn});
        if !context.is_null() {
            open["EncryptionContext"] = context.clone();
        }
        let opened = ok(&router, "Decrypt", &open).await;
        assert_eq!(
            opened["Plaintext"],
            json!(b64(&plaintext)),
            "case {described:?}: the context it was sealed under opens it, got {opened}"
        );

        // Rows: a DIFFERENT but well-formed context, which the tag refuses.
        let deviations: Vec<(&str, Value)> = vec![
            ("a context where there was one more", json!({"tenant": "acme"})),
            (
                "an entry the ciphertext does not carry",
                json!({"tenant": "acme", "purpose": "invoice", "stage": "prod"}),
            ),
            (
                "a value that differs in case",
                json!({"tenant": "Acme", "purpose": "invoice"}),
            ),
        ];
        for (deviation, other) in deviations {
            if other == context {
                continue;
            }
            let case = format!("{described:?} decrypted under {deviation:?}");
            let (status, payload) = call(
                &router,
                REGION,
                "Decrypt",
                &json!({"CiphertextBlob": blob, "KeyId": key_arn,
                        "EncryptionContext": other}),
            )
            .await;
            let code = error_code(&payload);
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                code, "InvalidCiphertextException",
                "case {case}: a well-formed context that does not match is an \
                 authentication failure, got {code:?} in {payload}"
            );
        }
    }
}

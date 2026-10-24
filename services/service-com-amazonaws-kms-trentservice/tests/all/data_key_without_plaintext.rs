// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GenerateDataKeyWithoutPlaintext`, driven over the wire through
//! the shared [`super::harness`].
//!
//! The rule under test: the operation draws a random data key of the length the request
//! states — through `KeySpec` or through `NumberOfBytes`, never both and never neither —
//! seals it under the current key material of the symmetric encryption KMS key the
//! `KeyId` resolves to, and answers ONLY the sealed copy, the key ARN and the id of the
//! material that sealed it. The plaintext data key is nowhere in the response, so the
//! only way back to it is `Decrypt` on the `CiphertextBlob` with the same encryption
//! context, and two calls never yield the same data key.
//!
//! Every assertion is made over a TABLE: the rows differ in how the length is stated
//! (each `DataKeySpec` value, and byte counts across the modelled `NumberOfBytes`
//! range including both of its bounds), in the encryption context bound into the
//! ciphertext, in how the request names the key (key id, key ARN, alias), and in which
//! key seals it.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, contains, create_alias, create_key, describe, error_code, is_lower_hex, ok, router, text,
    unb64, REGION,
};

/// The operation under test.
const OPERATION: &str = "GenerateDataKeyWithoutPlaintext";

/// The length of a `BackingKeyIdType`: 64 characters matching `^[a-f0-9]+$`
/// (`smithy_query(query="operation",operation="GenerateDataKeyWithoutPlaintext")`).
const KEY_MATERIAL_ID_LEN: usize = 64;

/// Every member `GenerateDataKeyWithoutPlaintextResponse` declares
/// (`smithy_query(query="operation",operation="GenerateDataKeyWithoutPlaintext")`).
/// The set has no plaintext member at all, which is what distinguishes this operation
/// from `GenerateDataKey`.
const RESPONSE_MEMBERS: [&str; 3] = ["CiphertextBlob", "KeyId", "KeyMaterialId"];

/// How a row states the length of the data key it asks for.
#[derive(Clone, Copy)]
enum Length {
    /// A `DataKeySpec` value.
    Spec(&'static str),
    /// A `NumberOfBytes` value.
    Bytes(i64),
}

/// One row of the length table: the case name, how the length is stated, and how many
/// bytes of data key that states.
struct LengthCase {
    case: &'static str,
    length: Length,
    bytes: usize,
}

/// The ways a request can state the length of its data key.
///
/// The `Spec` rows are EVERY value of `DataKeySpec`
/// (`smithy_query(query="shape",shape="DataKeySpec")`: `AES_128`, `AES_256`), with the
/// byte count each one names — "Use AES_128 to generate a 128-bit symmetric key, or
/// AES_256 to generate a 256-bit symmetric key"
/// (`smithy_query(query="doc",shape="GenerateDataKeyWithoutPlaintextRequest",member="KeySpec")`).
/// The `Bytes` rows span `NumberOfBytesType`'s modelled range (1..1024): both published
/// bounds, the documented example "use the value 64 to generate a 512-bit data key",
/// and a middling value that is not a whole number of AES blocks.
const LENGTH_CASES: &[LengthCase] = &[
    LengthCase {
        case: "KeySpec AES_128",
        length: Length::Spec("AES_128"),
        bytes: 16,
    },
    LengthCase {
        case: "KeySpec AES_256",
        length: Length::Spec("AES_256"),
        bytes: 32,
    },
    LengthCase {
        case: "NumberOfBytes 1, the modelled minimum",
        length: Length::Bytes(1),
        bytes: 1,
    },
    LengthCase {
        case: "NumberOfBytes 23, not a whole number of AES blocks",
        length: Length::Bytes(23),
        bytes: 23,
    },
    LengthCase {
        case: "NumberOfBytes 64, the documented 512-bit example",
        length: Length::Bytes(64),
        bytes: 64,
    },
    LengthCase {
        case: "NumberOfBytes 1024, the modelled maximum",
        length: Length::Bytes(1024),
        bytes: 1024,
    },
];

/// An encryption-context map as awsJson1_1 carries one.
fn context(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// The encryption contexts the rows are crossed with: absent, one entry, several.
fn context_cases() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no encryption context", None),
        ("one context entry", Some(context(&[("purpose", "test")]))),
        (
            "several context entries",
            Some(context(&[("a", "1"), ("b", "2"), ("c", "3")])),
        ),
    ]
}

/// The request a row describes against `key_id`.
fn request(length: Length, key_id: &str, encryption_context: Option<&Value>) -> Value {
    let mut request = json!({"KeyId": key_id});
    match length {
        Length::Spec(spec) => request["KeySpec"] = json!(spec),
        Length::Bytes(bytes) => request["NumberOfBytes"] = json!(bytes),
    }
    if let Some(encryption_context) = encryption_context {
        request["EncryptionContext"] = encryption_context.clone();
    }
    request
}

/// Assert the answer carries only the modelled members, and in particular no plaintext
/// data key, and return its ciphertext blob as the wire carries it.
fn sealed_only(case: &str, answer: &Value) -> Value {
    let members = answer
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a response object, got {answer}"));
    for member in members.keys() {
        assert!(
            RESPONSE_MEMBERS.contains(&member.as_str()),
            "case {case}: the response must carry only {RESPONSE_MEMBERS:?}, got the member \
             {member:?} in {answer}"
        );
    }
    assert!(
        answer["CiphertextBlob"].is_string(),
        "case {case}: expected a CiphertextBlob, got {answer}"
    );
    answer["CiphertextBlob"].clone()
}

/// The plaintext `Decrypt` recovers from `blob`, with the ciphertext's own encryption
/// context, together with the key material id it reports.
async fn decrypted(
    router: &MultiServiceRouter,
    case: &str,
    blob: &Value,
    encryption_context: Option<&Value>,
) -> (Vec<u8>, Value) {
    let mut request = json!({"CiphertextBlob": blob.clone()});
    if let Some(encryption_context) = encryption_context {
        request["EncryptionContext"] = encryption_context.clone();
    }
    let answer = ok(router, "Decrypt", &request).await;
    (
        unb64(case, &answer, "Plaintext"),
        answer["KeyMaterialId"].clone(),
    )
}

/// A data key of the requested length comes back sealed and only sealed, opens under
/// `Decrypt` with the same encryption context, and is different on every call.
#[tokio::test]
async fn a_data_key_of_the_requested_length_comes_back_only_sealed() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "data keys"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");
    let material_id = text("setup", &describe(&router, &key_id).await, "CurrentKeyMaterialId");

    for row in LENGTH_CASES {
        for (context_case, encryption_context) in context_cases() {
            let case = format!("{} / {context_case}", row.case);
            let body = request(row.length, &key_id, encryption_context.as_ref());

            let first = ok(&router, OPERATION, &body).await;
            let first_blob = sealed_only(&case, &first);
            let second = ok(&router, OPERATION, &body).await;
            let second_blob = sealed_only(&case, &second);

            assert_eq!(
                first["KeyId"],
                json!(key_arn),
                "case {case}: the answer must name the sealing key by its ARN, got {first}"
            );
            let reported_material = text(&case, &first, "KeyMaterialId");
            assert_eq!(
                reported_material, material_id,
                "case {case}: the answer must report the key material that sealed the data \
                 key ({material_id}), got {reported_material}"
            );
            assert!(
                reported_material.len() == KEY_MATERIAL_ID_LEN
                    && is_lower_hex(&reported_material),
                "case {case}: a KeyMaterialId is {KEY_MATERIAL_ID_LEN} lowercase hex \
                 characters, got {reported_material:?}"
            );

            // The data key itself is reachable only through `Decrypt`, which is the
            // round trip the operation exists for.
            let (plaintext, decrypt_material) =
                decrypted(&router, &case, &first_blob, encryption_context.as_ref()).await;
            assert_eq!(
                plaintext.len(),
                row.bytes,
                "case {case}: expected a {}-byte data key, got {} bytes",
                row.bytes,
                plaintext.len()
            );
            assert_eq!(
                decrypt_material,
                json!(material_id),
                "case {case}: Decrypt must report the material that sealed the data key, \
                 got {decrypt_material}"
            );
            let blob_bytes = unb64(&case, &first, "CiphertextBlob");
            // A very short data key can appear in a ciphertext's own random bytes by
            // chance, so this is asserted on the rows where it cannot: an 8-byte or
            // longer run coincides with probability below 2^-45 for a blob this size.
            if plaintext.len() >= 8 {
                assert!(
                    !contains(&blob_bytes, &plaintext),
                    "case {case}: the ciphertext must not carry the data key in the clear"
                );
            }

            // "The bytes in the key are random": a second call on the same request
            // yields a different data key, not the same one re-sealed.
            let (other, _) =
                decrypted(&router, &case, &second_blob, encryption_context.as_ref()).await;
            assert_eq!(
                other.len(),
                row.bytes,
                "case {case}: every call must answer a data key of the requested length, \
                 got {} bytes",
                other.len()
            );
            assert_ne!(
                plaintext, other,
                "case {case}: two calls must draw two different data keys"
            );
            assert_ne!(
                first_blob, second_blob,
                "case {case}: two calls must answer two different ciphertexts"
            );

            // The encryption context is authenticated, so opening the ciphertext under
            // any other context fails rather than returning the data key.
            let wrong = match &encryption_context {
                // A ciphertext sealed with no context is not openable under one.
                None => Some(context(&[("unexpected", "context")])),
                // A ciphertext sealed WITH a context is not openable without it.
                Some(_) => None,
            };
            let mut probe = json!({"CiphertextBlob": first_blob.clone()});
            if let Some(wrong) = &wrong {
                probe["EncryptionContext"] = wrong.clone();
            }
            let (status, payload) = call(&router, REGION, "Decrypt", &probe).await;
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (400, "InvalidCiphertextException"),
                "case {case}: a data key sealed under one encryption context must not open \
                 under another, got {status} with {payload}"
            );
        }
    }
}

/// Whatever identifier names the key — key id, key ARN or alias — the answer reports
/// that key's ARN and the material of THAT key, read back through `DescribeKey`.
#[tokio::test]
async fn the_answer_names_the_resolved_key_by_arn_whatever_identifier_named_it() {
    let router = router();

    // Two keys, so the answer is read per key rather than from one global value, and
    // an alias so each can also be named indirectly.
    let mut keys = Vec::new();
    for (case, description) in [("first key", "first"), ("second key", "second")] {
        let metadata = create_key(&router, &json!({"Description": description})).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");
        let alias = format!("alias/data-key-{description}");
        create_alias(&router, case, &alias, &key_id).await;
        let material_id = text(case, &describe(&router, &key_id).await, "CurrentKeyMaterialId");
        keys.push((case, key_id, key_arn, alias, material_id));
    }

    for (key_case, key_id, key_arn, alias, material_id) in &keys {
        for (form, identifier) in [
            ("by key id", key_id.as_str()),
            ("by key ARN", key_arn.as_str()),
            ("by alias", alias.as_str()),
        ] {
            for row in LENGTH_CASES {
                let case = format!("{key_case} / {form} / {}", row.case);
                let answer = ok(&router, OPERATION, &request(row.length, identifier, None)).await;
                let blob = sealed_only(&case, &answer);
                assert_eq!(
                    answer["KeyId"],
                    json!(key_arn),
                    "case {case}: the answer must name the resolved key by its ARN, got \
                     {answer}"
                );
                assert_eq!(
                    answer["KeyMaterialId"],
                    json!(material_id),
                    "case {case}: the answer must report that key's current material, got \
                     {answer}"
                );

                // The ciphertext is a ciphertext OF that key: `Decrypt` opens it and
                // reports the same key.
                let decrypted = ok(&router, "Decrypt", &json!({"CiphertextBlob": blob})).await;
                assert_eq!(
                    decrypted["KeyId"],
                    json!(key_arn),
                    "case {case}: Decrypt must report the key that sealed the data key, got \
                     {decrypted}"
                );
                assert_eq!(
                    unb64(&case, &decrypted, "Plaintext").len(),
                    row.bytes,
                    "case {case}: expected a {}-byte data key",
                    row.bytes
                );
            }
        }
    }
}

/// A request that does not state exactly one length, or names a key that cannot
/// generate a data key, is refused — and nothing is generated.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_with_its_own_error() {
    let router = router();
    let account = engine::emulator_account_id();

    let enabled = create_key(&router, &json!({"Description": "enabled"})).await;
    let enabled_id = text("setup", &enabled, "KeyId");
    let disabled = create_key(&router, &json!({"Description": "disabled"})).await;
    let disabled_id = text("setup", &disabled, "KeyId");
    ok(&router, "DisableKey", &json!({"KeyId": disabled_id})).await;
    let pending = create_key(&router, &json!({"Description": "pending"})).await;
    let pending_id = text("setup", &pending, "KeyId");
    ok(&router, "ScheduleKeyDeletion", &json!({"KeyId": pending_id})).await;

    // Keys of each other key kind: a `KeyUsage` that is not ENCRYPT_DECRYPT, and an
    // asymmetric ENCRYPT_DECRYPT key, both of which "cannot be used to generate a data
    // key".
    let hmac = text(
        "setup",
        &create_key(
            &router,
            &json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        )
        .await,
        "KeyId",
    );
    let signing = text(
        "setup",
        &create_key(
            &router,
            &json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
        )
        .await,
        "KeyId",
    );
    let asymmetric_encrypt = text(
        "setup",
        &create_key(
            &router,
            &json!({"KeySpec": "RSA_3072", "KeyUsage": "ENCRYPT_DECRYPT"}),
        )
        .await,
        "KeyId",
    );
    let agreement = text(
        "setup",
        &create_key(
            &router,
            &json!({"KeySpec": "ECC_NIST_P256", "KeyUsage": "KEY_AGREEMENT"}),
        )
        .await,
        "KeyId",
    );

    // Rows: the case, the request, and the wire error code expected. Every client
    // refusal of this service answers HTTP 400.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a key id that names no key",
            json!({"KeyId": "00000000-0000-0000-0000-000000000000", "KeySpec": "AES_256"}),
            "NotFoundException",
        ),
        (
            "an alias that resolves to no key",
            json!({"KeyId": "alias/no-such-alias", "NumberOfBytes": 32}),
            "NotFoundException",
        ),
        (
            // This operation models no `InvalidArnException`, so a malformed ARN is
            // reported the way an identifier naming nothing is.
            "an ARN that is not a KMS key ARN",
            json!({"KeyId": format!("arn:aws:kms:{REGION}:{account}:thing/abc"),
                   "KeySpec": "AES_256"}),
            "NotFoundException",
        ),
        (
            "a disabled key",
            json!({"KeyId": disabled_id, "KeySpec": "AES_256"}),
            "DisabledException",
        ),
        (
            "a key pending deletion",
            json!({"KeyId": pending_id, "KeySpec": "AES_256"}),
            "KMSInvalidStateException",
        ),
        (
            "an HMAC key",
            json!({"KeyId": hmac, "KeySpec": "AES_256"}),
            "InvalidKeyUsageException",
        ),
        (
            "a signing key",
            json!({"KeyId": signing, "KeySpec": "AES_256"}),
            "InvalidKeyUsageException",
        ),
        (
            "an asymmetric encryption key",
            json!({"KeyId": asymmetric_encrypt, "KeySpec": "AES_256"}),
            "InvalidKeyUsageException",
        ),
        (
            "a key agreement key",
            json!({"KeyId": agreement, "KeySpec": "AES_256"}),
            "InvalidKeyUsageException",
        ),
        (
            "both KeySpec and NumberOfBytes",
            json!({"KeyId": enabled_id, "KeySpec": "AES_256", "NumberOfBytes": 32}),
            "ValidationException",
        ),
        (
            "neither KeySpec nor NumberOfBytes",
            json!({"KeyId": enabled_id}),
            "ValidationException",
        ),
        (
            "NumberOfBytes 0, one below the modelled minimum",
            json!({"KeyId": enabled_id, "NumberOfBytes": 0}),
            "ValidationException",
        ),
        (
            "NumberOfBytes 1025, one above the modelled maximum",
            json!({"KeyId": enabled_id, "NumberOfBytes": 1025}),
            "ValidationException",
        ),
        (
            "a dry run",
            json!({"KeyId": enabled_id, "KeySpec": "AES_256", "DryRun": true}),
            "DryRunOperationException",
        ),
        (
            "a grant token KMS never issued",
            json!({"KeyId": enabled_id, "KeySpec": "AES_256",
                   "GrantTokens": ["not-a-grant-token"]}),
            "InvalidGrantTokenException",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, payload) = call(&router, REGION, OPERATION, &body).await;
        let code = error_code(&payload);
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected HTTP 400 {expected}, got {status} with {payload}"
        );
    }
}

/// A request whose permission comes from a grant token is served exactly when that
/// grant covers it: the right operation on the right key, with the request's encryption
/// context satisfying the grant's constraints.
#[tokio::test]
async fn a_grant_token_is_honoured_only_for_what_the_grant_permits() {
    let router = router();
    let key_id = text(
        "setup",
        &create_key(&router, &json!({"Description": "granted"})).await,
        "KeyId",
    );
    let grantee = format!(
        "arn:aws:iam::{account}:user/Alice",
        account = engine::emulator_account_id()
    );

    let permitting = text(
        "setup",
        &ok(
            &router,
            "CreateGrant",
            &json!({"KeyId": key_id, "GranteePrincipal": grantee,
                    "Operations": [OPERATION]}),
        )
        .await,
        "GrantToken",
    );
    let unrelated = text(
        "setup",
        &ok(
            &router,
            "CreateGrant",
            &json!({"KeyId": key_id, "GranteePrincipal": grantee,
                    "Operations": ["Encrypt"]}),
        )
        .await,
        "GrantToken",
    );
    let constrained = text(
        "setup",
        &ok(
            &router,
            "CreateGrant",
            &json!({"KeyId": key_id, "GranteePrincipal": grantee,
                    "Operations": [OPERATION],
                    "Constraints": {"EncryptionContextEquals": {"purpose": "test"}}}),
        )
        .await,
        "GrantToken",
    );

    // Rows: the case, the token presented, the encryption context the request carries,
    // and the error code expected — `None` where the grant covers the request.
    let cases: Vec<(&str, &str, Option<Value>, Option<&str>)> = vec![
        (
            "a grant permitting this operation",
            &permitting,
            None,
            None,
        ),
        (
            "a grant permitting only another operation",
            &unrelated,
            None,
            Some("AccessDeniedException"),
        ),
        (
            "a constrained grant with the context it requires",
            &constrained,
            Some(context(&[("purpose", "test")])),
            None,
        ),
        (
            "a constrained grant with another context",
            &constrained,
            Some(context(&[("purpose", "other")])),
            Some("AccessDeniedException"),
        ),
        (
            "a constrained grant with no context",
            &constrained,
            None,
            Some("AccessDeniedException"),
        ),
    ];

    for (case, token, encryption_context, expected) in cases {
        let mut body = request(Length::Spec("AES_256"), &key_id, encryption_context.as_ref());
        body["GrantTokens"] = json!([token]);
        let (status, payload) = call(&router, REGION, OPERATION, &body).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {payload}"
                );
                let blob = sealed_only(case, &payload);
                let mut decrypt = json!({"CiphertextBlob": blob});
                if let Some(encryption_context) = &encryption_context {
                    decrypt["EncryptionContext"] = encryption_context.clone();
                }
                let opened = ok(&router, "Decrypt", &decrypt).await;
                assert_eq!(
                    unb64(case, &opened, "Plaintext").len(),
                    32,
                    "case {case:?}: expected a 32-byte AES_256 data key, got {opened}"
                );
            }
            Some(expected) => {
                let code = error_code(&payload);
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {case:?}: expected HTTP 400 {expected}, got {status} with {payload}"
                );
            }
        }
    }
}

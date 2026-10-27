// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ReEncrypt`, driven over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: `ReEncrypt` opens the ciphertext with the key material that
//! sealed it, authenticating the SOURCE encryption context, and seals the recovered
//! plaintext again under the current material of the destination KMS key,
//! authenticating the DESTINATION encryption context — reporting both keys by ARN,
//! the algorithm each half used, and the key material id each half used. The plaintext
//! is unchanged by the move and the input ciphertext is left as it was; a request whose
//! source key, destination key, ciphertext or algorithm cannot support that is refused
//! and nothing is produced.
//!
//! Every assertion is made over a TABLE: the rows differ in the length of the plaintext
//! that was sealed, in which of the two encryption contexts are set, in how the request
//! names the destination key (key id, key ARN, alias, and the source key itself), in
//! whether the optional `SourceKeyId` is stated, in how many rotations stand between
//! the two halves, and — for the refusals — in which part of the request is wrong.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    b64, call, contains, create_alias, create_key, describe, error_code, failure, is_lower_hex, ok,
    rotate_on_demand, router, text, unb64, REGION,
};

/// The operation under test.
const OPERATION: &str = "ReEncrypt";

/// The only `EncryptionAlgorithmSpec` a symmetric encryption KMS key uses: "the default
/// value, SYMMETRIC_DEFAULT, represents the encryption algorithm used for symmetric
/// encryption KMS keys"
/// (`smithy_query(query="doc",shape="ReEncryptRequest",member="DestinationEncryptionAlgorithm")`).
const SYMMETRIC_DEFAULT: &str = "SYMMETRIC_DEFAULT";

/// The length of a `BackingKeyIdType`: 64 characters matching `^[a-f0-9]+$`
/// (`smithy_query(query="operation",operation="ReEncrypt")`).
const KEY_MATERIAL_ID_LEN: usize = 64;

/// The whole of the message KMS words for `IncorrectKeyException` — fixed text that
/// names no key, so a caller that presented the wrong identifier does not learn which
/// key did seal the ciphertext.
const INCORRECT_KEY_MESSAGE: &str =
    "The key ID in the request does not identify a CMK that can perform this operation.";

/// The `message` an error body carries.
fn error_message(payload: &Value) -> String {
    payload["message"].as_str().unwrap_or_default().to_string()
}

/// An encryption-context map as awsJson1_1 carries one.
fn context(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// A deterministic plaintext of `len` bytes, so a case names the length it is about and
/// no row depends on a value drawn at random.
fn plaintext(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect()
}

/// Seal `bytes` under `key_id` with `encryption_context`, requiring success, and answer
/// the `CiphertextBlob`.
async fn encrypt(
    router: &MultiServiceRouter,
    case: &str,
    key_id: &str,
    bytes: &[u8],
    encryption_context: &[(&str, &str)],
) -> Vec<u8> {
    let mut request = json!({"KeyId": key_id, "Plaintext": b64(bytes)});
    if !encryption_context.is_empty() {
        request["EncryptionContext"] = context(encryption_context);
    }
    let sealed = ok(router, "Encrypt", &request).await;
    unb64(case, &sealed, "CiphertextBlob")
}

/// Open `blob` with `encryption_context`, requiring success, and answer the recovered
/// plaintext together with the whole `Decrypt` answer.
async fn decrypt(
    router: &MultiServiceRouter,
    case: &str,
    blob: &[u8],
    encryption_context: &[(&str, &str)],
) -> (Vec<u8>, Value) {
    let mut request = json!({"CiphertextBlob": b64(blob)});
    if !encryption_context.is_empty() {
        request["EncryptionContext"] = context(encryption_context);
    }
    let opened = ok(router, "Decrypt", &request).await;
    let bytes = unb64(case, &opened, "Plaintext");
    (bytes, opened)
}

/// The `ReEncrypt` request a row describes.
fn re_encrypt_request(
    blob: &[u8],
    destination: &str,
    source_key_id: Option<&str>,
    source_context: &[(&str, &str)],
    destination_context: &[(&str, &str)],
) -> Value {
    let mut request = json!({"CiphertextBlob": b64(blob), "DestinationKeyId": destination});
    if let Some(source_key_id) = source_key_id {
        request["SourceKeyId"] = json!(source_key_id);
    }
    if !source_context.is_empty() {
        request["SourceEncryptionContext"] = context(source_context);
    }
    if !destination_context.is_empty() {
        request["DestinationEncryptionContext"] = context(destination_context);
    }
    request
}

/// Assert that `value` is a `BackingKeyIdType`: 64 lowercase hexadecimal characters.
fn assert_key_material_id(case: &str, member: &str, value: &str) {
    assert!(
        value.len() == KEY_MATERIAL_ID_LEN && is_lower_hex(value),
        "case {case}: expected {member} to be {KEY_MATERIAL_ID_LEN} lowercase hex \
         characters, got {value:?}"
    );
}

/// One row of the round-trip table: the plaintext length and the two encryption
/// contexts the move runs under.
struct RoundTrip {
    case: &'static str,
    plaintext_len: usize,
    source_context: &'static [(&'static str, &'static str)],
    destination_context: &'static [(&'static str, &'static str)],
}

/// The round-trip rows: plaintext lengths spanning `PlaintextType`'s modelled 1..4096
/// range (`smithy_query(query="operation",operation="Encrypt")`) — both published
/// bounds and a middling value — crossed with each combination of the two optional
/// encryption contexts, since each half authenticates its own.
const ROUND_TRIPS: &[RoundTrip] = &[
    RoundTrip {
        case: "1 byte, the modelled minimum plaintext, no encryption context",
        plaintext_len: 1,
        source_context: &[],
        destination_context: &[],
    },
    RoundTrip {
        case: "32 bytes, no encryption context",
        plaintext_len: 32,
        source_context: &[],
        destination_context: &[],
    },
    RoundTrip {
        case: "4096 bytes, the modelled maximum plaintext, no encryption context",
        plaintext_len: 4096,
        source_context: &[],
        destination_context: &[],
    },
    RoundTrip {
        case: "a source context dropped by the move",
        plaintext_len: 32,
        source_context: &[("Department", "Finance")],
        destination_context: &[],
    },
    RoundTrip {
        case: "a destination context added by the move",
        plaintext_len: 32,
        source_context: &[],
        destination_context: &[("Project", "Ledger")],
    },
    RoundTrip {
        case: "both contexts set, and different",
        plaintext_len: 71,
        source_context: &[("Department", "Finance")],
        destination_context: &[("Department", "Legal"), ("Project", "Ledger")],
    },
    RoundTrip {
        case: "both contexts set to the same several pairs",
        plaintext_len: 4096,
        source_context: &[("a", "1"), ("b", "2"), ("c", "3")],
        destination_context: &[("a", "1"), ("b", "2"), ("c", "3")],
    },
];

/// `ReEncrypt` moves a ciphertext onto the destination key without changing the
/// plaintext it carries, and reports both halves of what it did.
#[tokio::test]
async fn re_encrypt_rewraps_the_same_plaintext_under_the_destination_key() {
    let router = router();

    for row in ROUND_TRIPS {
        // How the request names the destination key: every form
        // `smithy_query(query="doc",shape="ReEncryptRequest",member="DestinationKeyId")`
        // accepts, plus the documented reencrypt-to-self case ("you can also use it to
        // reencrypt ciphertext under the same KMS key").
        for destination_form in ["key id", "key ARN", "alias", "the source key itself"] {
            // `SourceKeyId` "is optional" for a symmetric ciphertext, because the blob
            // carries the key it was sealed under.
            for names_source in [false, true] {
                let case = format!(
                    "{}, destination named by {destination_form}, SourceKeyId {}",
                    row.case,
                    if names_source { "stated" } else { "omitted" }
                );

                let source = create_key(&router, &json!({"Description": "source"})).await;
                let source_id = text(&case, &source, "KeyId");
                let source_arn = text(&case, &source, "Arn");
                let source_material = text(&case, &source, "CurrentKeyMaterialId");

                let (destination_arn, destination_material, destination) = match destination_form {
                    "the source key itself" => (source_arn.clone(), source_material.clone(), {
                        source_id.clone()
                    }),
                    form => {
                        let created =
                            create_key(&router, &json!({"Description": "destination"})).await;
                        let id = text(&case, &created, "KeyId");
                        let arn = text(&case, &created, "Arn");
                        let material = text(&case, &created, "CurrentKeyMaterialId");
                        let named = match form {
                            "key id" => id.clone(),
                            "key ARN" => arn.clone(),
                            _ => {
                                let alias = format!("alias/reencrypt-to-{id}");
                                create_alias(&router, &case, &alias, &id).await;
                                alias
                            }
                        };
                        (arn, material, named)
                    }
                };

                let bytes = plaintext(row.plaintext_len);
                let sealed =
                    encrypt(&router, &case, &source_id, &bytes, row.source_context).await;

                let answer = ok(
                    &router,
                    OPERATION,
                    &re_encrypt_request(
                        &sealed,
                        &destination,
                        names_source.then_some(source_id.as_str()),
                        row.source_context,
                        row.destination_context,
                    ),
                )
                .await;

                // Each key is reported by its ARN, whatever form the request named it
                // in: "the Amazon Resource Name (key ARN) of the KMS key that was used
                // to reencrypt the data".
                assert_eq!(
                    text(&case, &answer, "SourceKeyId"),
                    source_arn,
                    "case {case:?}: expected SourceKeyId {source_arn}, got {answer}"
                );
                assert_eq!(
                    text(&case, &answer, "KeyId"),
                    destination_arn,
                    "case {case:?}: expected KeyId {destination_arn}, got {answer}"
                );
                for member in ["SourceEncryptionAlgorithm", "DestinationEncryptionAlgorithm"] {
                    let reported = text(&case, &answer, member);
                    assert_eq!(
                        reported, SYMMETRIC_DEFAULT,
                        "case {case:?}: expected {member} {SYMMETRIC_DEFAULT}, got {reported:?}"
                    );
                }

                // The material that did each half: the version that sealed the input,
                // and the destination key's current version.
                let reported_source_material = text(&case, &answer, "SourceKeyMaterialId");
                let reported_destination_material =
                    text(&case, &answer, "DestinationKeyMaterialId");
                assert_key_material_id(&case, "SourceKeyMaterialId", &reported_source_material);
                assert_key_material_id(
                    &case,
                    "DestinationKeyMaterialId",
                    &reported_destination_material,
                );
                assert_eq!(
                    reported_source_material, source_material,
                    "case {case:?}: expected SourceKeyMaterialId {source_material}, got \
                     {reported_source_material}"
                );
                assert_eq!(
                    reported_destination_material, destination_material,
                    "case {case:?}: expected DestinationKeyMaterialId \
                     {destination_material}, got {reported_destination_material}"
                );

                // A new envelope, not the one the request handed in, and opaque. A run
                // as short as a byte or two appears in any blob by chance, so the
                // opacity check is made on the rows whose plaintext is long enough for
                // a coincidental match to be impossible in practice.
                let rewrapped = unb64(&case, &answer, "CiphertextBlob");
                assert_ne!(
                    rewrapped, sealed,
                    "case {case:?}: the reencrypted ciphertext must not be the input one"
                );
                if row.plaintext_len >= 16 {
                    assert!(
                        !contains(&rewrapped, &bytes),
                        "case {case:?}: the reencrypted ciphertext must not carry its \
                         plaintext, got {} bytes",
                        rewrapped.len()
                    );
                }

                // The plaintext survived the move, and the new ciphertext belongs to the
                // destination key under the DESTINATION context.
                let (opened, decrypted) =
                    decrypt(&router, &case, &rewrapped, row.destination_context).await;
                assert_eq!(
                    opened,
                    bytes,
                    "case {case:?}: expected the same {} byte plaintext back, got {} bytes",
                    bytes.len(),
                    opened.len()
                );
                assert_eq!(
                    text(&case, &decrypted, "KeyId"),
                    destination_arn,
                    "case {case:?}: the reencrypted ciphertext must open under \
                     {destination_arn}, got {decrypted}"
                );
                assert_eq!(
                    text(&case, &decrypted, "KeyMaterialId"),
                    reported_destination_material,
                    "case {case:?}: Decrypt must name the material ReEncrypt sealed \
                     with, got {decrypted}"
                );

                // The destination context is the one bound into the new ciphertext: when
                // the two differ, the source context no longer opens it.
                if row.source_context != row.destination_context {
                    let mut request = json!({"CiphertextBlob": b64(&rewrapped)});
                    if !row.source_context.is_empty() {
                        request["EncryptionContext"] = context(row.source_context);
                    }
                    let (status, payload) = call(&router, REGION, "Decrypt", &request).await;
                    assert_eq!(
                        status,
                        400,
                        "case {case:?}: the source context must not open the reencrypted \
                         ciphertext, got HTTP {status} with {payload}"
                    );
                    assert_eq!(
                        error_code(&payload),
                        "InvalidCiphertextException",
                        "case {case:?}: expected InvalidCiphertextException, got {payload}"
                    );
                }

                // `ReEncrypt` consumed nothing: the input ciphertext still opens as it
                // did before.
                let (again, _) = decrypt(&router, &case, &sealed, row.source_context).await;
                assert_eq!(
                    again, bytes,
                    "case {case:?}: the input ciphertext must still decrypt after the move"
                );
            }
        }
    }
}

/// Reencrypting to the same key rewraps the ciphertext under the material that is
/// current NOW, while still opening it with the material that sealed it — which is what
/// makes a reencrypt-to-self after a rotation move a ciphertext onto the new material.
///
/// The rows differ in how many rotations stand between the two halves and in the length
/// of the plaintext.
#[tokio::test]
async fn re_encrypt_to_the_same_key_moves_the_ciphertext_onto_the_current_material() {
    let router = router();

    for rotations in [1usize, 2, 5] {
        for plaintext_len in [1usize, 32, 4096] {
            let case = format!("{rotations} rotation(s), a {plaintext_len} byte plaintext");

            let key = create_key(&router, &json!({"Description": "rotated"})).await;
            let key_id = text(&case, &key, "KeyId");
            let arn = text(&case, &key, "Arn");
            let sealing_material = text(&case, &key, "CurrentKeyMaterialId");

            let bytes = plaintext(plaintext_len);
            let sealed = encrypt(&router, &case, &key_id, &bytes, &[]).await;

            for _ in 0..rotations {
                rotate_on_demand(&router, &case, &key_id).await;
            }
            let current_material = text(&case, &describe(&router, &key_id).await, "CurrentKeyMaterialId");
            assert_ne!(
                current_material, sealing_material,
                "case {case:?}: a rotation must make a different key material current"
            );

            let answer = ok(
                &router,
                OPERATION,
                &re_encrypt_request(&sealed, &key_id, Some(&key_id), &[], &[]),
            )
            .await;

            assert_eq!(
                text(&case, &answer, "SourceKeyMaterialId"),
                sealing_material,
                "case {case:?}: expected the material that sealed the input, \
                 {sealing_material}, got {answer}"
            );
            assert_eq!(
                text(&case, &answer, "DestinationKeyMaterialId"),
                current_material,
                "case {case:?}: expected the material current now, {current_material}, \
                 got {answer}"
            );
            for member in ["SourceKeyId", "KeyId"] {
                let reported = text(&case, &answer, member);
                assert_eq!(
                    reported, arn,
                    "case {case:?}: expected {member} {arn}, got {reported}"
                );
            }

            // The rewrapped ciphertext opens under the material the key uses now.
            let rewrapped = unb64(&case, &answer, "CiphertextBlob");
            let (opened, decrypted) = decrypt(&router, &case, &rewrapped, &[]).await;
            assert_eq!(
                opened,
                bytes,
                "case {case:?}: expected the same {plaintext_len} byte plaintext back, got {} \
                 bytes",
                opened.len()
            );
            assert_eq!(
                text(&case, &decrypted, "KeyMaterialId"),
                current_material,
                "case {case:?}: the rewrapped ciphertext must open under the current \
                 material, got {decrypted}"
            );

            // And the pre-rotation ciphertext still opens under the material that sealed
            // it, untouched by the move.
            let (before, decrypted_before) = decrypt(&router, &case, &sealed, &[]).await;
            assert_eq!(
                before, bytes,
                "case {case:?}: the pre-rotation ciphertext must still decrypt"
            );
            assert_eq!(
                text(&case, &decrypted_before, "KeyMaterialId"),
                sealing_material,
                "case {case:?}: the pre-rotation ciphertext must still name the material \
                 that sealed it, got {decrypted_before}"
            );
        }
    }
}

/// A data key's two halves still correspond after the sealed half has been moved onto
/// another KMS key: the plaintext `GenerateDataKey` answered is exactly what `Decrypt`
/// recovers from the reencrypted blob.
///
/// The rows differ in how the data key's length was stated — each `DataKeySpec` value
/// and a `NumberOfBytes` count (`smithy_query(query="operation",operation="GenerateDataKey")`).
#[tokio::test]
async fn re_encrypt_moves_a_data_key_without_changing_the_key_it_wraps() {
    let router = router();

    let lengths: [(&str, Value); 4] = [
        ("KeySpec AES_128", json!({"KeySpec": "AES_128"})),
        ("KeySpec AES_256", json!({"KeySpec": "AES_256"})),
        ("NumberOfBytes 1", json!({"NumberOfBytes": 1})),
        ("NumberOfBytes 1024", json!({"NumberOfBytes": 1024})),
    ];

    for (case, length) in lengths {
        let source = create_key(&router, &json!({"Description": "data key source"})).await;
        let destination = create_key(&router, &json!({"Description": "data key target"})).await;
        let source_id = text(case, &source, "KeyId");
        let destination_id = text(case, &destination, "KeyId");

        let mut request = length.clone();
        request["KeyId"] = json!(source_id);
        let generated = ok(&router, "GenerateDataKey", &request).await;
        let data_key = unb64(case, &generated, "Plaintext");
        let wrapped = unb64(case, &generated, "CiphertextBlob");

        let answer = ok(
            &router,
            OPERATION,
            &re_encrypt_request(&wrapped, &destination_id, None, &[], &[]),
        )
        .await;
        let rewrapped = unb64(case, &answer, "CiphertextBlob");

        let (opened, decrypted) = decrypt(&router, case, &rewrapped, &[]).await;
        assert_eq!(
            opened,
            data_key,
            "case {case}: expected the same {} byte data key back, got {} bytes",
            data_key.len(),
            opened.len()
        );
        assert_eq!(
            text(case, &decrypted, "KeyId"),
            text(case, &destination, "Arn"),
            "case {case}: the moved data key must open under the destination key, got \
             {decrypted}"
        );
    }
}

/// A request `ReEncrypt` cannot serve is refused with the modelled error for what is
/// wrong with it, and nothing is produced: the ciphertext the request handed in still
/// decrypts exactly as it did.
///
/// The rows differ in which part of the request is wrong — the ciphertext, the source
/// key, the destination key, an algorithm, a grant token, or the `DryRun` switch — and
/// in whether the refusal is a client fault or a capability this emulator does not hold
/// the key material for.
#[tokio::test]
async fn a_request_re_encrypt_cannot_serve_is_refused_and_produces_nothing() {
    let router = router();

    let source = create_key(&router, &json!({"Description": "refusals"})).await;
    let source_id = text("setup", &source, "KeyId");
    let other = create_key(&router, &json!({"Description": "another key"})).await;
    let other_id = text("setup", &other, "KeyId");
    let other_arn = text("setup", &other, "Arn");

    let disabled = create_key(&router, &json!({"Description": "disabled"})).await;
    let disabled_id = text("setup", &disabled, "KeyId");
    ok(&router, "DisableKey", &json!({"KeyId": disabled_id})).await;
    let pending = create_key(&router, &json!({"Description": "pending deletion"})).await;
    let pending_id = text("setup", &pending, "KeyId");
    ok(&router, "ScheduleKeyDeletion", &json!({"KeyId": pending_id})).await;

    let signing = create_key(
        &router,
        &json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let signing_id = text("setup", &signing, "KeyId");
    let hmac = create_key(
        &router,
        &json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
    )
    .await;
    let hmac_id = text("setup", &hmac, "KeyId");
    let asymmetric = create_key(
        &router,
        &json!({"KeySpec": "RSA_2048", "KeyUsage": "ENCRYPT_DECRYPT"}),
    )
    .await;
    let asymmetric_id = text("setup", &asymmetric, "KeyId");

    let bytes = plaintext(64);
    let sealed = encrypt(&router, "setup", &source_id, &bytes, &[("k", "v")]).await;
    let blob = b64(&sealed);

    // A ciphertext sealed under a key that is then disabled, so the SOURCE half is the
    // half that cannot run.
    let frozen = create_key(&router, &json!({"Description": "disabled source"})).await;
    let frozen_id = text("setup", &frozen, "KeyId");
    let frozen_blob = b64(&encrypt(&router, "setup", &frozen_id, &bytes, &[]).await);
    ok(&router, "DisableKey", &json!({"KeyId": frozen_id})).await;

    // An identifier of the right shape that names no key in this Region.
    let absent = "11111111-2222-4333-8444-555555555555";

    let mut truncated = sealed.clone();
    truncated.truncate(10);
    let mut tampered = sealed.clone();
    if let Some(last) = tampered.last_mut() {
        *last ^= 0xff;
    }

    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "no CiphertextBlob at all",
            json!({"DestinationKeyId": other_id}),
            400,
            "ValidationException",
        ),
        (
            "a CiphertextBlob that is not a KMS envelope",
            json!({"CiphertextBlob": b64(b"not-a-kms-ciphertext"),
                   "DestinationKeyId": other_id}),
            400,
            "InvalidCiphertextException",
        ),
        (
            "a truncated envelope",
            json!({"CiphertextBlob": b64(&truncated), "DestinationKeyId": other_id}),
            400,
            "InvalidCiphertextException",
        ),
        (
            "an envelope whose last byte was flipped",
            json!({"CiphertextBlob": b64(&tampered), "DestinationKeyId": other_id,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "InvalidCiphertextException",
        ),
        (
            "the wrong SourceEncryptionContext",
            json!({"CiphertextBlob": blob, "DestinationKeyId": other_id,
                   "SourceEncryptionContext": context(&[("k", "other")])}),
            400,
            "InvalidCiphertextException",
        ),
        (
            "no SourceEncryptionContext where the ciphertext has one",
            json!({"CiphertextBlob": blob, "DestinationKeyId": other_id}),
            400,
            "InvalidCiphertextException",
        ),
        (
            "a SourceKeyId naming a key that did not seal the ciphertext",
            json!({"CiphertextBlob": blob, "DestinationKeyId": other_id,
                   "SourceKeyId": other_arn,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "IncorrectKeyException",
        ),
        (
            "a SourceKeyId naming no key",
            json!({"CiphertextBlob": blob, "DestinationKeyId": other_id,
                   "SourceKeyId": absent,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "NotFoundException",
        ),
        (
            "a DestinationKeyId naming no key",
            json!({"CiphertextBlob": blob, "DestinationKeyId": absent,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "NotFoundException",
        ),
        (
            "a DestinationKeyId naming no alias",
            json!({"CiphertextBlob": blob, "DestinationKeyId": "alias/not-created",
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "NotFoundException",
        ),
        (
            "a disabled destination key",
            json!({"CiphertextBlob": blob, "DestinationKeyId": disabled_id,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "DisabledException",
        ),
        (
            "a destination key pending deletion",
            json!({"CiphertextBlob": blob, "DestinationKeyId": pending_id,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "KMSInvalidStateException",
        ),
        (
            "a disabled source key",
            json!({"CiphertextBlob": frozen_blob, "DestinationKeyId": other_id}),
            400,
            "DisabledException",
        ),
        (
            "a destination key whose KeyUsage is SIGN_VERIFY",
            json!({"CiphertextBlob": blob, "DestinationKeyId": signing_id,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "InvalidKeyUsageException",
        ),
        (
            "a destination key whose KeyUsage is GENERATE_VERIFY_MAC",
            json!({"CiphertextBlob": blob, "DestinationKeyId": hmac_id,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "InvalidKeyUsageException",
        ),
        (
            "a SourceEncryptionAlgorithm the source key spec does not support",
            json!({"CiphertextBlob": blob, "DestinationKeyId": other_id,
                   "SourceEncryptionAlgorithm": "RSAES_OAEP_SHA_1",
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "InvalidKeyUsageException",
        ),
        (
            "a DestinationEncryptionAlgorithm the destination key spec does not support",
            json!({"CiphertextBlob": blob, "DestinationKeyId": other_id,
                   "DestinationEncryptionAlgorithm": "SM2PKE",
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "InvalidKeyUsageException",
        ),
        (
            "a grant token this service never minted",
            json!({"CiphertextBlob": blob, "DestinationKeyId": other_id,
                   "GrantTokens": ["not-a-grant-token"],
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "InvalidGrantTokenException",
        ),
        (
            "DryRun on a request that would otherwise have succeeded",
            json!({"CiphertextBlob": blob, "DestinationKeyId": other_id, "DryRun": true,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "DryRunOperationException",
        ),
        (
            "DryRun with IGNORE_CIPHERTEXT and no ciphertext at all",
            json!({"DestinationKeyId": other_id, "SourceKeyId": source_id, "DryRun": true,
                   "DryRunModifiers": ["IGNORE_CIPHERTEXT"]}),
            400,
            "DryRunOperationException",
        ),
        (
            "DryRun with IGNORE_CIPHERTEXT, no ciphertext and no SourceKeyId",
            json!({"DestinationKeyId": other_id, "DryRun": true,
                   "DryRunModifiers": ["IGNORE_CIPHERTEXT"]}),
            400,
            "ValidationException",
        ),
        (
            "DryRun on a request that is wrong for its own reason",
            json!({"CiphertextBlob": blob, "DestinationKeyId": absent, "DryRun": true,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            400,
            "NotFoundException",
        ),
        (
            // The same answer `Encrypt` gives an asymmetric KMS key: the RSAES-OAEP
            // scheme it calls for is not implemented here, so the request is reported as
            // not implemented rather than answered with a ciphertext another scheme
            // produced.
            "an asymmetric destination key, whose encryption scheme is not implemented",
            json!({"CiphertextBlob": blob, "DestinationKeyId": asymmetric_id,
                   "SourceEncryptionContext": context(&[("k", "v")])}),
            501,
            "NotImplemented",
        ),
    ];

    for (case, body, expected_status, expected_code) in cases {
        let (status, code) = failure(&router, OPERATION, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({code})"
        );
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code}"
        );
    }

    // The refusal for a ciphertext pinned to the wrong KMS key is FIXED text: it names
    // neither the key the request gave nor the key that sealed the ciphertext, which a
    // caller that never named it must not learn from a refusal.
    let pinned = json!({"CiphertextBlob": blob, "DestinationKeyId": other_id,
                        "SourceKeyId": other_arn,
                        "SourceEncryptionContext": context(&[("k", "v")])});
    let (_, payload) = call(&router, REGION, OPERATION, &pinned).await;
    let message = error_message(&payload);
    assert_eq!(
        message, INCORRECT_KEY_MESSAGE,
        "case {pinned}: expected the fixed IncorrectKeyException text, got {message:?}"
    );
    for named in [&source_id, &other_id, &other_arn] {
        assert!(
            !message.contains(named.as_str()),
            "case {pinned}: the refusal must not name {named}, got {message:?}"
        );
    }

    // Nothing above moved any state: the ciphertext the refused requests handed in
    // still opens, under the key and context it was sealed with.
    let (opened, _) = decrypt(&router, "after the refusals", &sealed, &[("k", "v")]).await;
    assert_eq!(
        opened, bytes,
        "a refused ReEncrypt must leave the input ciphertext exactly as it was"
    );
}

/// A request whose only permission comes from grant tokens needs a grant for EACH half:
/// `ReEncryptFrom` on the source key and `ReEncryptTo` on the destination key.
///
/// The rows differ in which of the two permissions the presented tokens carry.
#[tokio::test]
async fn each_half_of_a_re_encrypt_draws_on_its_own_grant_permission() {
    let router = router();

    let source = create_key(&router, &json!({"Description": "grant source"})).await;
    let destination = create_key(&router, &json!({"Description": "grant destination"})).await;
    let source_id = text("setup", &source, "KeyId");
    let destination_id = text("setup", &destination, "KeyId");

    let grantee = format!(
        "arn:aws:iam::{account}:role/Rewrapper",
        account = engine::emulator_account_id()
    );

    /// A grant token for `operations` on `key_id`.
    async fn token(
        router: &MultiServiceRouter,
        case: &str,
        key_id: &str,
        grantee: &str,
        operations: &[&str],
    ) -> String {
        let answer = ok(
            router,
            "CreateGrant",
            &json!({"KeyId": key_id, "GranteePrincipal": grantee, "Operations": operations}),
        )
        .await;
        text(case, &answer, "GrantToken")
    }

    let from = token(
        &router,
        "setup",
        &source_id,
        &grantee,
        &["ReEncryptFrom"],
    )
    .await;
    let to = token(&router, "setup", &destination_id, &grantee, &["ReEncryptTo"]).await;
    let decrypt_only = token(&router, "setup", &source_id, &grantee, &["Decrypt"]).await;

    let bytes = plaintext(48);
    let sealed = encrypt(&router, "setup", &source_id, &bytes, &[]).await;

    // Both halves granted: the move succeeds.
    let case = "grants for both halves";
    let mut request = re_encrypt_request(&sealed, &destination_id, None, &[], &[]);
    request["GrantTokens"] = json!([from, to]);
    let answer = ok(&router, OPERATION, &request).await;
    let rewrapped = unb64(case, &answer, "CiphertextBlob");
    let (opened, _) = decrypt(&router, case, &rewrapped, &[]).await;
    assert_eq!(
        opened, bytes,
        "case {case:?}: expected the plaintext back through the granted move"
    );

    // A half with no grant is refused, and so is a grant for another operation.
    let refusals: [(&str, Value); 3] = [
        ("only ReEncryptFrom", json!([from])),
        ("only ReEncryptTo", json!([to])),
        ("a grant for Decrypt instead", json!([decrypt_only])),
    ];
    for (case, tokens) in refusals {
        let mut request = re_encrypt_request(&sealed, &destination_id, None, &[], &[]);
        request["GrantTokens"] = tokens;
        let (status, code) = failure(&router, OPERATION, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {code}"
        );
    }
}

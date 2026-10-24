// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for on-demand rotation of a KMS key's key material, driven over
//! the wire through the shared [`super::harness`].
//!
//! Four rules, each asserted over a TABLE rather than one call:
//!
//! 1. An on-demand rotation of a symmetric encryption KMS key replaces the key
//!    material the key encrypts under: `DescribeKey` reports a `CurrentKeyMaterialId`
//!    that has not been this key's before, and nothing else about the key moves.
//! 2. A rotation does not discard the material already in use: a ciphertext sealed
//!    before any rotation still decrypts after it, and `Decrypt` reports the
//!    `KeyMaterialId` of the version that sealed it — not whichever version is
//!    current.
//! 3. Only a symmetric encryption KMS key rotates on demand; a key of any other kind,
//!    a key whose key state is not `Enabled`, and an identifier that names no key are
//!    refused, and the key's material is left as it was.
//! 4. On-demand rotation is quota-bounded per key: rotations up to the quota succeed
//!    and the one past it is refused.

use serde_json::{json, Value};

use super::harness::{
    b64, call, create_alias, create_key, describe, error_code, is_lower_hex, ok, router, text,
    unb64,
};

/// The members of `KeyMetadata` that a key-material rotation must leave exactly as
/// they were: a rotation changes which material the key uses, not the key's identity,
/// kind or key state (`smithy_query(query="shape",shape="KeyMetadata")`).
const INVARIANT_METADATA: [&str; 9] = [
    "KeyId",
    "Arn",
    "KeySpec",
    "KeyUsage",
    "KeyState",
    "Enabled",
    "CreationDate",
    "Origin",
    "MultiRegion",
];

/// Every rotation of a symmetric encryption KMS key makes a key material version
/// current that the key has not had before, and leaves the rest of the key alone.
///
/// The rows differ in how the key was created (the default symmetric encryption key,
/// the same spec and usage stated explicitly, and a multi-Region primary key), in the
/// form of the identifier the request names the key by (key id, key ARN, alias — the
/// forms `smithy_query(query="doc",shape="RotateKeyOnDemandRequest",member="KeyId")`
/// accepts), and in how many rotations in a row the key gets.
#[tokio::test]
async fn every_rotation_makes_a_new_key_material_version_current() {
    let router = router();

    let creates: [(&str, Value); 3] = [
        ("the default symmetric encryption key", json!({})),
        (
            "an explicit SYMMETRIC_DEFAULT ENCRYPT_DECRYPT key",
            json!({"KeySpec": "SYMMETRIC_DEFAULT", "KeyUsage": "ENCRYPT_DECRYPT"}),
        ),
        ("a multi-Region primary key", json!({"MultiRegion": true})),
    ];

    for (described, request) in creates {
        for identifier_form in ["key id", "key ARN", "alias"] {
            for rotations in [1usize, 2, 5] {
                let case = format!("{described} named by {identifier_form}, {rotations} rotation(s)");
                let created = create_key(&router, &request).await;
                let key_id = text(&case, &created, "KeyId");
                let arn = text(&case, &created, "Arn");
                let identifier = match identifier_form {
                    "key id" => key_id.clone(),
                    "key ARN" => arn.clone(),
                    _ => {
                        let alias = format!("alias/rotate-{key_id}");
                        create_alias(&router, &case, &alias, &key_id).await;
                        alias
                    }
                };

                let mut seen = vec![text(&case, &created, "CurrentKeyMaterialId")];
                for round in 1..=rotations {
                    let answer =
                        ok(&router, "RotateKeyOnDemand", &json!({"KeyId": identifier})).await;
                    let answered = text(&case, &answer, "KeyId");
                    assert_eq!(
                        answered, arn,
                        "case {case:?} round {round}: expected the answer to identify the key \
                         by its ARN {arn}, got {answered:?}"
                    );

                    let after = describe(&router, &key_id).await;
                    let material = text(&case, &after, "CurrentKeyMaterialId");
                    assert_eq!(
                        material.len(),
                        64,
                        "case {case:?} round {round}: a KeyMaterialId is 64 characters, \
                         got {material:?}"
                    );
                    assert!(
                        is_lower_hex(&material),
                        "case {case:?} round {round}: a KeyMaterialId is lowercase hex, \
                         got {material:?}"
                    );
                    assert!(
                        !seen.contains(&material),
                        "case {case:?} round {round}: expected a key material version the key \
                         has not had, got {material:?} among {seen:?}"
                    );
                    seen.push(material);

                    for member in INVARIANT_METADATA {
                        assert_eq!(
                            after[member], created[member],
                            "case {case:?} round {round}: expected {member} to be untouched by \
                             a rotation, got {:?} (was {:?})",
                            after[member], created[member]
                        );
                    }
                }
                assert_eq!(
                    seen.len(),
                    rotations + 1,
                    "case {case:?}: expected one key material version per rotation plus the \
                     original, got {seen:?}"
                );
            }
        }
    }
}

/// A ciphertext sealed before a rotation decrypts after it, to the same plaintext, and
/// `Decrypt` names the key material version that sealed it rather than the current
/// one. A ciphertext sealed AFTER a rotation names the version that rotation made
/// current.
///
/// The rows differ in the plaintext (one byte, text, every byte value, and the
/// modelled maximum — `PlaintextType` is 1..4096 bytes,
/// `smithy_query(query="operation",operation="Encrypt")`), in the encryption context
/// (absent, one entry, two entries), and in how many rotations have happened since
/// the ciphertext was produced (0 through 3).
#[tokio::test]
async fn a_ciphertext_decrypts_under_the_material_version_that_sealed_it() {
    let router = router();
    let created = create_key(&router, &json!({"Description": "rotation and old ciphertext"})).await;
    let key_id = text("setup", &created, "KeyId");
    let arn = text("setup", &created, "Arn");

    let plaintexts: [(&str, Vec<u8>); 4] = [
        ("one byte", vec![0x01]),
        ("text", b"hello kms".to_vec()),
        ("every byte value", (0u8..=255).collect()),
        ("the modelled maximum of 4096 bytes", vec![0x5a; 4096]),
    ];
    let contexts: [(&str, Value); 3] = [
        ("no encryption context", Value::Null),
        ("one context entry", json!({"purpose": "rotation"})),
        ("two context entries", json!({"a": "1", "b": "2"})),
    ];

    /// One ciphertext, with what it was sealed from and which material sealed it.
    struct Sealed {
        case: String,
        blob: String,
        plaintext: Vec<u8>,
        context: Value,
        key_material_id: String,
    }

    let mut sealed: Vec<Sealed> = Vec::new();
    // Round 0 seals every row under the key's original material; each later round
    // rotates the key and then re-seals every row under the material that rotation
    // made current. Every ciphertext taken so far is decrypted after each rotation,
    // so each version is exercised with one, two and three rotations behind it.
    for round in 0..=3usize {
        if round > 0 {
            ok(&router, "RotateKeyOnDemand", &json!({"KeyId": key_id})).await;
        }
        let current = text("current material", &describe(&router, &key_id).await, "CurrentKeyMaterialId");

        for (plaintext_case, plaintext) in &plaintexts {
            for (context_case, context) in &contexts {
                let case = format!("{plaintext_case} with {context_case}, sealed in round {round}");
                let mut request = json!({"KeyId": key_id, "Plaintext": b64(plaintext)});
                if !context.is_null() {
                    request["EncryptionContext"] = context.clone();
                }
                let answer = ok(&router, "Encrypt", &request).await;
                sealed.push(Sealed {
                    case,
                    blob: text("encrypt", &answer, "CiphertextBlob"),
                    plaintext: plaintext.clone(),
                    context: context.clone(),
                    key_material_id: current.clone(),
                });
            }
        }

        for entry in &sealed {
            let case = format!("{} decrypted in round {round}", entry.case);
            let mut request = json!({"CiphertextBlob": entry.blob});
            if !entry.context.is_null() {
                request["EncryptionContext"] = entry.context.clone();
            }
            let answer = ok(&router, "Decrypt", &request).await;
            let plaintext = unb64(&case, &answer, "Plaintext");
            assert_eq!(
                plaintext,
                entry.plaintext,
                "case {case:?}: expected the plaintext back, got {} byte(s)",
                plaintext.len()
            );
            let material = text(&case, &answer, "KeyMaterialId");
            assert_eq!(
                material, entry.key_material_id,
                "case {case:?}: expected the key material version that sealed the ciphertext \
                 ({}), got {material:?}",
                entry.key_material_id
            );
            let answered_key = text(&case, &answer, "KeyId");
            assert_eq!(
                answered_key, arn,
                "case {case:?}: expected the key ARN {arn}, got {answered_key:?}"
            );
        }
    }

    // The retained versions are still bound to their encryption context: a ciphertext
    // sealed under one context does not open under another, however many rotations
    // have happened since.
    for entry in &sealed {
        let case = format!("{} decrypted under the wrong context", entry.case);
        let mut request = json!({"CiphertextBlob": entry.blob});
        let mut context = entry
            .context
            .as_object()
            .cloned()
            .unwrap_or_else(serde_json::Map::new);
        context.insert("tampered".to_string(), json!("yes"));
        request["EncryptionContext"] = Value::Object(context);
        let (status, payload) = call(&router, super::harness::REGION, "Decrypt", &request).await;
        let code = error_code(&payload);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            code, "InvalidCiphertextException",
            "case {case:?}: expected InvalidCiphertextException, got {code:?} in {payload}"
        );
    }
}

/// A key that is not a symmetric encryption KMS key, a key whose key state is not
/// `Enabled`, and an identifier that names no key are all refused, and the refusal
/// leaves the key's material exactly as it was.
///
/// The rows span the key kinds `smithy_query(query="shape",shape="KeySpec")` names
/// outside symmetric encryption (HMAC keys of every HMAC spec, RSA and elliptic-curve
/// and SM2 key pairs, under each usage that spec accepts), the key states a key can be
/// in besides `Enabled` (`smithy_query(query="shape",shape="KeyState")`), and the
/// identifier forms that resolve to nothing.
#[tokio::test]
async fn rotation_is_refused_outside_an_enabled_symmetric_encryption_key() {
    let router = router();
    let account = engine::emulator_account_id();

    // Rows: a key spec and the usage to create it with, for every kind of key that is
    // not a symmetric encryption key.
    let other_kinds: [(&str, &str); 10] = [
        ("HMAC_224", "GENERATE_VERIFY_MAC"),
        ("HMAC_256", "GENERATE_VERIFY_MAC"),
        ("HMAC_512", "GENERATE_VERIFY_MAC"),
        ("RSA_2048", "ENCRYPT_DECRYPT"),
        ("RSA_2048", "SIGN_VERIFY"),
        ("RSA_4096", "SIGN_VERIFY"),
        ("ECC_NIST_P256", "SIGN_VERIFY"),
        ("ECC_NIST_P384", "KEY_AGREEMENT"),
        ("ECC_SECG_P256K1", "SIGN_VERIFY"),
        ("SM2", "ENCRYPT_DECRYPT"),
    ];
    for (key_spec, key_usage) in other_kinds {
        let case = format!("{key_spec} key with usage {key_usage}");
        let created = create_key(
            &router,
            &json!({"KeySpec": key_spec, "KeyUsage": key_usage}),
        )
        .await;
        let key_id = text(&case, &created, "KeyId");
        let (status, payload) = call(
            &router,
            super::harness::REGION,
            "RotateKeyOnDemand",
            &json!({"KeyId": key_id}),
        )
        .await;
        let code = error_code(&payload);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            code, "UnsupportedOperationException",
            "case {case:?}: expected UnsupportedOperationException, got {code:?} in {payload}"
        );
        let after = describe(&router, &key_id).await;
        assert_eq!(
            after, created,
            "case {case:?}: a refused rotation must persist nothing, got {after}"
        );
    }

    // Rows: a key that cannot rotate because of its key state, and the error that
    // reports it. A disabled key has its own modelled refusal; any other key state
    // outside `Enabled` is reported as an incompatible key state.
    let disabled = create_key(&router, &json!({"Description": "disabled"})).await;
    let disabled_id = text("setup", &disabled, "KeyId");
    ok(&router, "DisableKey", &json!({"KeyId": disabled_id})).await;
    let pending = create_key(&router, &json!({"Description": "pending deletion"})).await;
    let pending_id = text("setup", &pending, "KeyId");
    ok(&router, "ScheduleKeyDeletion", &json!({"KeyId": pending_id})).await;
    let importing = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let importing_id = text("setup", &importing, "KeyId");

    let states: [(&str, &str, &str); 3] = [
        ("a disabled key", &disabled_id, "DisabledException"),
        (
            "a key pending deletion",
            &pending_id,
            "KMSInvalidStateException",
        ),
        (
            "a key awaiting imported key material",
            &importing_id,
            "KMSInvalidStateException",
        ),
    ];
    for (case, identifier, expected) in states {
        let before = describe(&router, identifier).await;
        let (status, payload) = call(
            &router,
            super::harness::REGION,
            "RotateKeyOnDemand",
            &json!({"KeyId": identifier}),
        )
        .await;
        let code = error_code(&payload);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code:?} in {payload}"
        );
        let after = describe(&router, identifier).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused rotation must persist nothing, got {after}"
        );
    }

    // Rows: an identifier that resolves to no key of this account and Region, by each
    // form it can take, and the error that reports it.
    let lookups: [(&str, String, &str); 4] = [
        (
            "an unknown key id",
            "00000000-0000-0000-0000-000000000000".to_string(),
            "NotFoundException",
        ),
        (
            "an unknown key ARN",
            format!("arn:aws:kms:us-east-1:{account}:key/mrk-{}", "0".repeat(32)),
            "NotFoundException",
        ),
        (
            "an unknown alias",
            "alias/no-such-key".to_string(),
            "NotFoundException",
        ),
        (
            "an ARN that is not a KMS key or alias ARN",
            format!("arn:aws:kms:us-east-1:{account}:thing/abc"),
            "InvalidArnException",
        ),
    ];
    for (case, identifier, expected) in lookups {
        let (status, payload) = call(
            &router,
            super::harness::REGION,
            "RotateKeyOnDemand",
            &json!({"KeyId": identifier}),
        )
        .await;
        let code = error_code(&payload);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code:?} in {payload}"
        );
    }
}

/// On-demand rotation is bounded per key: every rotation up to the quota succeeds, and
/// the first one past it is refused as an exceeded quota with nothing persisted.
///
/// The bound is the documented one — "you can perform on-demand key rotation a maximum
/// of 25 times per KMS key"
/// (`smithy_query(query="doc",shape="RotateKeyOnDemand")`) — so the rows are every
/// rotation up to it and one past it.
#[tokio::test]
async fn on_demand_rotation_is_bounded_per_key() {
    /// See the doc comment: the documented per-key on-demand rotation quota.
    const QUOTA: usize = 25;

    let router = router();
    let created = create_key(&router, &json!({"Description": "quota"})).await;
    let key_id = text("setup", &created, "KeyId");

    let mut seen = vec![text("setup", &created, "CurrentKeyMaterialId")];
    for round in 1..=QUOTA {
        let case = format!("rotation {round} of {QUOTA}");
        ok(&router, "RotateKeyOnDemand", &json!({"KeyId": key_id})).await;
        let material = text(
            &case,
            &describe(&router, &key_id).await,
            "CurrentKeyMaterialId",
        );
        assert!(
            !seen.contains(&material),
            "case {case:?}: expected a key material version the key has not had, \
             got {material:?}"
        );
        seen.push(material);
    }

    let case = format!("rotation {} of {QUOTA}", QUOTA + 1);
    let before = describe(&router, &key_id).await;
    let (status, payload) = call(
        &router,
        super::harness::REGION,
        "RotateKeyOnDemand",
        &json!({"KeyId": key_id}),
    )
    .await;
    let code = error_code(&payload);
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        code, "LimitExceededException",
        "case {case:?}: expected LimitExceededException, got {code:?} in {payload}"
    );
    let after = describe(&router, &key_id).await;
    assert_eq!(
        after, before,
        "case {case:?}: a refused rotation must persist nothing, got {after}"
    );
}

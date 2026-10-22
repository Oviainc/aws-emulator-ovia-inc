// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a key's `KeySpec` decides, and for the key metadata a
//! caller writes and reads back, driven over the wire through the shared
//! [`super::harness`].
//!
//! Three rules, each asserted over a TABLE rather than one call:
//!
//! 1. `CreateKey` mints a key of every `KeySpec` the model declares, with each
//!    `KeyUsage` that spec accepts, and the `KeyMetadata` it answers reports the
//!    algorithms that spec supports under that usage.
//! 2. A `KeyUsage` the spec does not support is refused, and nothing is minted.
//! 3. `UpdateKeyDescription` replaces the description `DescribeKey` reports, for any
//!    description within the modelled length and for a key of any spec.

use serde_json::{json, Value};

use super::harness::{
    b64, create_key, describe, failure, is_key_id, is_lower_hex, ok, router, text, REGION,
};

/// The four per-usage algorithm lists `KeyMetadata` carries
/// (`smithy_query(shape="KeyMetadata")`). Exactly one of them is present on a key,
/// and it is the one belonging to the key's own `KeyUsage`.
const ALGORITHM_MEMBERS: [&str; 4] = [
    "EncryptionAlgorithms",
    "SigningAlgorithms",
    "MacAlgorithms",
    "KeyAgreementAlgorithms",
];

/// The `KeySpec` values the DEPRECATED `CustomerMasterKeySpec` enum can name
/// (`smithy_query(shape="CustomerMasterKeySpec")`); a key of any other spec reports
/// its spec through `KeySpec` alone.
const DEPRECATED_SPEC_VALUES: [&str; 13] = [
    "ECC_NIST_P256",
    "ECC_NIST_P384",
    "ECC_NIST_P521",
    "ECC_SECG_P256K1",
    "HMAC_224",
    "HMAC_256",
    "HMAC_384",
    "HMAC_512",
    "RSA_2048",
    "RSA_3072",
    "RSA_4096",
    "SM2",
    "SYMMETRIC_DEFAULT",
];

/// One row of the spec table: a `KeySpec`, the `KeyUsage` the request carries (or
/// `None` to leave it out), and the algorithm list the key then reports.
struct SpecCase {
    key_spec: &'static str,
    requested_usage: Option<&'static str>,
    expected_usage: &'static str,
    algorithms_member: &'static str,
    algorithms: &'static [&'static str],
}

/// Every `KeySpec` of `smithy_query(shape="KeySpec")`, under each `KeyUsage`
/// `smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")` says that
/// spec accepts. The algorithms are the ones the "Key spec reference" the `KeySpec`
/// doc links gives for the spec, named with the `EncryptionAlgorithmSpec`,
/// `SigningAlgorithmSpec`, `MacAlgorithmSpec` and `KeyAgreementAlgorithmSpec` enum
/// values, and listed in ascending order of the algorithm name — the order KMS
/// reports them in.
const SPEC_CASES: &[SpecCase] = &[
    // Symmetric encryption key (default), with the usage omitted and with it stated.
    SpecCase {
        key_spec: "SYMMETRIC_DEFAULT",
        requested_usage: None,
        expected_usage: "ENCRYPT_DECRYPT",
        algorithms_member: "EncryptionAlgorithms",
        algorithms: &["SYMMETRIC_DEFAULT"],
    },
    SpecCase {
        key_spec: "SYMMETRIC_DEFAULT",
        requested_usage: Some("ENCRYPT_DECRYPT"),
        expected_usage: "ENCRYPT_DECRYPT",
        algorithms_member: "EncryptionAlgorithms",
        algorithms: &["SYMMETRIC_DEFAULT"],
    },
    // HMAC keys (symmetric).
    SpecCase {
        key_spec: "HMAC_224",
        requested_usage: Some("GENERATE_VERIFY_MAC"),
        expected_usage: "GENERATE_VERIFY_MAC",
        algorithms_member: "MacAlgorithms",
        algorithms: &["HMAC_SHA_224"],
    },
    SpecCase {
        key_spec: "HMAC_256",
        requested_usage: Some("GENERATE_VERIFY_MAC"),
        expected_usage: "GENERATE_VERIFY_MAC",
        algorithms_member: "MacAlgorithms",
        algorithms: &["HMAC_SHA_256"],
    },
    SpecCase {
        key_spec: "HMAC_384",
        requested_usage: Some("GENERATE_VERIFY_MAC"),
        expected_usage: "GENERATE_VERIFY_MAC",
        algorithms_member: "MacAlgorithms",
        algorithms: &["HMAC_SHA_384"],
    },
    SpecCase {
        key_spec: "HMAC_512",
        requested_usage: Some("GENERATE_VERIFY_MAC"),
        expected_usage: "GENERATE_VERIFY_MAC",
        algorithms_member: "MacAlgorithms",
        algorithms: &["HMAC_SHA_512"],
    },
    // Asymmetric RSA key pairs: the six RSASSA signing algorithms, or the two
    // RSAES-OAEP encryption algorithms.
    SpecCase {
        key_spec: "RSA_2048",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &[
            "RSASSA_PKCS1_V1_5_SHA_256",
            "RSASSA_PKCS1_V1_5_SHA_384",
            "RSASSA_PKCS1_V1_5_SHA_512",
            "RSASSA_PSS_SHA_256",
            "RSASSA_PSS_SHA_384",
            "RSASSA_PSS_SHA_512",
        ],
    },
    SpecCase {
        key_spec: "RSA_2048",
        requested_usage: Some("ENCRYPT_DECRYPT"),
        expected_usage: "ENCRYPT_DECRYPT",
        algorithms_member: "EncryptionAlgorithms",
        algorithms: &["RSAES_OAEP_SHA_1", "RSAES_OAEP_SHA_256"],
    },
    SpecCase {
        key_spec: "RSA_3072",
        requested_usage: Some("ENCRYPT_DECRYPT"),
        expected_usage: "ENCRYPT_DECRYPT",
        algorithms_member: "EncryptionAlgorithms",
        algorithms: &["RSAES_OAEP_SHA_1", "RSAES_OAEP_SHA_256"],
    },
    SpecCase {
        key_spec: "RSA_4096",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &[
            "RSASSA_PKCS1_V1_5_SHA_256",
            "RSASSA_PKCS1_V1_5_SHA_384",
            "RSASSA_PKCS1_V1_5_SHA_512",
            "RSASSA_PSS_SHA_256",
            "RSASSA_PSS_SHA_384",
            "RSASSA_PSS_SHA_512",
        ],
    },
    // Asymmetric NIST-standard elliptic curve key pairs: signing with the curve's
    // digest, or deriving a shared secret.
    SpecCase {
        key_spec: "ECC_NIST_P256",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ECDSA_SHA_256"],
    },
    SpecCase {
        key_spec: "ECC_NIST_P256",
        requested_usage: Some("KEY_AGREEMENT"),
        expected_usage: "KEY_AGREEMENT",
        algorithms_member: "KeyAgreementAlgorithms",
        algorithms: &["ECDH"],
    },
    SpecCase {
        key_spec: "ECC_NIST_P384",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ECDSA_SHA_384"],
    },
    SpecCase {
        key_spec: "ECC_NIST_P521",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ECDSA_SHA_512"],
    },
    SpecCase {
        key_spec: "ECC_NIST_P521",
        requested_usage: Some("KEY_AGREEMENT"),
        expected_usage: "KEY_AGREEMENT",
        algorithms_member: "KeyAgreementAlgorithms",
        algorithms: &["ECDH"],
    },
    // "ECC_NIST_EDWARDS25519 (ed25519) - signing and verification only", with both
    // Ed25519 signing algorithms.
    SpecCase {
        key_spec: "ECC_NIST_EDWARDS25519",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ED25519_PH_SHA_512", "ED25519_SHA_512"],
    },
    // Other asymmetric elliptic curve key pairs (signing and verification).
    SpecCase {
        key_spec: "ECC_SECG_P256K1",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ECDSA_SHA_256"],
    },
    // Asymmetric ML-DSA key pairs (signing and verification).
    SpecCase {
        key_spec: "ML_DSA_44",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ML_DSA_SHAKE_256"],
    },
    SpecCase {
        key_spec: "ML_DSA_65",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ML_DSA_SHAKE_256"],
    },
    SpecCase {
        key_spec: "ML_DSA_87",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ML_DSA_SHAKE_256"],
    },
    // SM2 key pairs: encryption, signing, or key agreement.
    SpecCase {
        key_spec: "SM2",
        requested_usage: Some("ENCRYPT_DECRYPT"),
        expected_usage: "ENCRYPT_DECRYPT",
        algorithms_member: "EncryptionAlgorithms",
        algorithms: &["SM2PKE"],
    },
    SpecCase {
        key_spec: "SM2",
        requested_usage: Some("SIGN_VERIFY"),
        expected_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["SM2DSA"],
    },
    SpecCase {
        key_spec: "SM2",
        requested_usage: Some("KEY_AGREEMENT"),
        expected_usage: "KEY_AGREEMENT",
        algorithms_member: "KeyAgreementAlgorithms",
        algorithms: &["ECDH"],
    },
];

/// `CreateKey` mints a key of every modelled `KeySpec` under every `KeyUsage` that
/// spec accepts, and the `KeyMetadata` it answers with — the same record
/// `DescribeKey` reports afterwards — carries the identity of the key and the
/// algorithms its spec supports under that usage.
#[tokio::test]
async fn create_key_mints_every_modelled_key_spec_with_the_algorithms_it_supports() {
    let router = router();
    let account = engine::emulator_account_id();
    let mut seen_key_ids: Vec<String> = Vec::new();

    for spec_case in SPEC_CASES {
        let case = format!(
            "{} with KeyUsage {:?}",
            spec_case.key_spec, spec_case.requested_usage
        );
        let description = format!("{} key", spec_case.key_spec);
        let mut request = json!({
            "KeySpec": spec_case.key_spec,
            "Description": description,
            "Tags": [{"TagKey": "spec", "TagValue": spec_case.key_spec}],
        });
        if let Some(usage) = spec_case.requested_usage {
            request["KeyUsage"] = json!(usage);
        }

        let metadata = create_key(&router, &request).await;

        // Identity: a fresh key id, an ARN naming it in the signed Region, and the
        // state a key with KMS-generated material starts in.
        let key_id = text(&case, &metadata, "KeyId");
        assert!(
            is_key_id(&key_id, false),
            "case {case}: expected a version-4 UUID key id, got {key_id:?}"
        );
        assert!(
            !seen_key_ids.contains(&key_id),
            "case {case}: every create mints a fresh key id, but {key_id} repeated"
        );
        seen_key_ids.push(key_id.clone());
        let arn = text(&case, &metadata, "Arn");
        assert_eq!(
            arn,
            format!("arn:aws:kms:{REGION}:{account}:key/{key_id}"),
            "case {case}: the Arn must name the minted key id in the signed Region"
        );
        assert_eq!(
            text(&case, &metadata, "AWSAccountId"),
            account,
            "case {case}: expected the emulator account id, got {metadata}"
        );
        assert_eq!(
            metadata["Enabled"],
            json!(true),
            "case {case}: a new key is enabled, got {metadata}"
        );
        assert_eq!(
            text(&case, &metadata, "KeyState"),
            "Enabled",
            "case {case}: a new key with KMS-generated material is Enabled, got {metadata}"
        );
        assert_eq!(
            text(&case, &metadata, "KeyManager"),
            "CUSTOMER",
            "case {case}: a key the caller created is customer managed, got {metadata}"
        );
        assert_eq!(
            text(&case, &metadata, "Origin"),
            "AWS_KMS",
            "case {case}: KMS generated this key's material, got {metadata}"
        );
        assert_eq!(
            metadata["MultiRegion"],
            json!(false),
            "case {case}: the request asked for a single-Region key, got {metadata}"
        );
        assert_eq!(
            text(&case, &metadata, "Description"),
            description,
            "case {case}: the description must be the one the request carried"
        );

        // Capability: the spec, the usage, and exactly the one algorithm list that
        // belongs to that usage.
        assert_eq!(
            text(&case, &metadata, "KeySpec"),
            spec_case.key_spec,
            "case {case}: expected the requested KeySpec, got {metadata}"
        );
        assert_eq!(
            text(&case, &metadata, "KeyUsage"),
            spec_case.expected_usage,
            "case {case}: expected KeyUsage {}, got {metadata}",
            spec_case.expected_usage
        );
        if DEPRECATED_SPEC_VALUES.contains(&spec_case.key_spec) {
            assert_eq!(
                metadata["CustomerMasterKeySpec"], metadata["KeySpec"],
                "case {case}: the deprecated spec member reports the same value, got {metadata}"
            );
        } else {
            assert!(
                metadata["CustomerMasterKeySpec"].is_null(),
                "case {case}: the deprecated CustomerMasterKeySpec enum cannot name this \
                 spec, so it must be omitted, got {metadata}"
            );
        }
        assert_eq!(
            metadata[spec_case.algorithms_member],
            json!(spec_case.algorithms),
            "case {case}: expected {} {:?}, got {metadata}",
            spec_case.algorithms_member,
            spec_case.algorithms
        );
        for member in ALGORITHM_MEMBERS {
            if member == spec_case.algorithms_member {
                continue;
            }
            assert!(
                metadata[member].is_null(),
                "case {case}: {member} belongs to another KeyUsage and must be omitted, \
                 got {metadata}"
            );
        }

        // `CurrentKeyMaterialId` "is present for symmetric encryption keys with
        // AWS_KMS or EXTERNAL origin" (`smithy_query(shape="KeyMetadata")`), and is
        // 64 lowercase hex characters (`smithy_query(shape="BackingKeyIdType")`).
        if spec_case.key_spec == "SYMMETRIC_DEFAULT" {
            let material_id = text(&case, &metadata, "CurrentKeyMaterialId");
            assert!(
                material_id.len() == 64 && is_lower_hex(&material_id),
                "case {case}: expected 64 lowercase hex characters of KeyMaterialId, \
                 got {material_id:?}"
            );
        } else {
            assert!(
                metadata["CurrentKeyMaterialId"].is_null(),
                "case {case}: only a symmetric encryption key reports a key material id, \
                 got {metadata}"
            );
        }

        // The record is persisted: every identifier form reports the same metadata.
        for identifier in [key_id.as_str(), arn.as_str()] {
            assert_eq!(
                describe(&router, identifier).await,
                metadata,
                "case {case}: DescribeKey({identifier}) must report what CreateKey returned"
            );
        }
    }
}

/// A `KeyUsage` the `KeySpec` does not support is refused with HTTP 400
/// `ValidationException`, and so is an omitted `KeyUsage` for a spec that does not
/// default it — and nothing is minted either way.
///
/// The rows span the whole matrix of mismatches: a symmetric key asked to sign, an
/// HMAC key asked to encrypt, an elliptic-curve key asked to encrypt, an Ed25519 and
/// a secp256k1 key asked to derive a shared secret, an ML-DSA key asked to encrypt,
/// an RSA key asked to generate MACs, and every non-symmetric family with no usage
/// at all.
#[tokio::test]
async fn create_key_refuses_a_key_usage_its_key_spec_does_not_support() {
    let router = router();

    // One key that IS created, so "nothing was minted" has something to compare to.
    let kept = create_key(&router, &json!({"Description": "kept"})).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "symmetric key asked to sign",
            json!({"KeySpec": "SYMMETRIC_DEFAULT", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "symmetric key asked to derive a shared secret",
            json!({"KeyUsage": "KEY_AGREEMENT"}),
        ),
        (
            "symmetric key asked to generate MACs",
            json!({"KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        (
            "HMAC key asked to encrypt",
            json!({"KeySpec": "HMAC_256", "KeyUsage": "ENCRYPT_DECRYPT"}),
        ),
        (
            "HMAC key asked to sign",
            json!({"KeySpec": "HMAC_512", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "RSA key asked to generate MACs",
            json!({"KeySpec": "RSA_2048", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        (
            "elliptic-curve key asked to encrypt",
            json!({"KeySpec": "ECC_NIST_P256", "KeyUsage": "ENCRYPT_DECRYPT"}),
        ),
        (
            "Ed25519 key asked to derive a shared secret",
            json!({"KeySpec": "ECC_NIST_EDWARDS25519", "KeyUsage": "KEY_AGREEMENT"}),
        ),
        (
            "secp256k1 key asked to derive a shared secret",
            json!({"KeySpec": "ECC_SECG_P256K1", "KeyUsage": "KEY_AGREEMENT"}),
        ),
        (
            "ML-DSA key asked to encrypt",
            json!({"KeySpec": "ML_DSA_65", "KeyUsage": "ENCRYPT_DECRYPT"}),
        ),
        (
            "RSA key with no usage",
            json!({"KeySpec": "RSA_3072"}),
        ),
        (
            "HMAC key with no usage",
            json!({"KeySpec": "HMAC_224"}),
        ),
        (
            "elliptic-curve key with no usage",
            json!({"KeySpec": "ECC_NIST_P384"}),
        ),
        (
            "SM2 key with no usage",
            json!({"KeySpec": "SM2"}),
        ),
        (
            "deprecated spec member with an incompatible usage",
            json!({"CustomerMasterKeySpec": "HMAC_256", "KeyUsage": "ENCRYPT_DECRYPT"}),
        ),
    ];

    for (case, request) in cases {
        let (status, code) = failure(&router, "CreateKey", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a refused create, got {status} ({code})"
        );
        assert_eq!(
            code, "ValidationException",
            "case {case}: expected ValidationException, got {code}"
        );
    }

    let listed = ok(&router, "ListKeys", &json!({})).await;
    assert_eq!(
        listed["Keys"],
        json!([{"KeyId": kept["KeyId"], "KeyArn": kept["Arn"]}]),
        "a refused create must leave no key behind, got {listed}"
    );
}

/// `UpdateKeyDescription` replaces the key's description, and the `DescribeKey` that
/// follows reports the value just written — for any description the modelled
/// `@length(0..8192)` allows, on a key of any spec, and over repeated updates.
///
/// Nothing else about the key moves: the rest of the metadata is compared against the
/// record before the write.
#[tokio::test]
async fn update_key_description_replaces_what_describe_key_reports() {
    let router = router();

    // The same rule holds whatever the key is made of.
    let keys = [
        (
            "symmetric",
            json!({"Description": "created", "KeySpec": "SYMMETRIC_DEFAULT"}),
        ),
        (
            "asymmetric signing",
            json!({"Description": "created", "KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "HMAC",
            json!({"Description": "created", "KeySpec": "HMAC_256",
                   "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        (
            "multi-Region",
            json!({"Description": "created", "MultiRegion": true}),
        ),
    ];

    // The rows differ in what a description can be: a plain rename, the empty string
    // the modelled minimum allows, the 8192-character maximum, non-ASCII text, and
    // the value the key already had.
    let longest = "d".repeat(8192);
    let descriptions = [
        "renamed-updated-description",
        "",
        longest.as_str(),
        "clé ✓ 鍵",
        "created",
    ];

    for (kind, request) in keys {
        let created = create_key(&router, &request).await;
        let key_id = text(kind, &created, "KeyId");
        let arn = text(kind, &created, "Arn");

        for (index, description) in descriptions.iter().enumerate() {
            let case = format!("{kind} #{index}");
            // Address the key by id on even rounds and by ARN on odd ones: every
            // identifier form names the same record.
            let identifier = if index % 2 == 0 { &key_id } else { &arn };

            let acknowledged = ok(
                &router,
                "UpdateKeyDescription",
                &json!({"KeyId": identifier, "Description": description}),
            )
            .await;
            assert!(
                acknowledged.is_null() || acknowledged == json!({}),
                "case {case}: UpdateKeyDescription models a Unit output, got {acknowledged}"
            );

            let described = describe(&router, &key_id).await;
            assert_eq!(
                described["Description"],
                json!(description),
                "case {case}: DescribeKey must report the description just written, \
                 got {described}"
            );

            // Only the description moved.
            let mut expected = created.clone();
            expected["Description"] = json!(description);
            assert_eq!(
                described, expected,
                "case {case}: the update must touch nothing but the description"
            );
        }
    }
}

/// `UpdateKeyDescription` refuses an identifier it cannot resolve and a key state it
/// cannot write, each with its own modelled error code — and the stored description is
/// unchanged after every refusal.
///
/// Every refusal carries HTTP 400: this service answers the whole client band with the
/// `awsJson1_1` client status and distinguishes the error by `__type` alone (see
/// `super::client_error_status` and `scaffolded::wire_status`).
#[tokio::test]
async fn update_key_description_refuses_an_identifier_or_state_it_cannot_write() {
    let router = router();
    let account = engine::emulator_account_id();
    let live = create_key(&router, &json!({"Description": "untouched"})).await;
    let live_id = text("live", &live, "KeyId");

    // A key scheduled for deletion: its metadata stays as it was until the deletion
    // is cancelled.
    let pending = create_key(&router, &json!({"Description": "pending"})).await;
    let pending_id = text("pending", &pending, "KeyId");
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": pending_id, "PendingWindowInDays": 7}),
    )
    .await;

    let cases: Vec<(&str, String, &str)> = vec![
        (
            "unknown key id",
            "00000000-0000-0000-0000-000000000000".to_string(),
            "NotFoundException",
        ),
        (
            "unknown key ARN",
            format!("arn:aws:kms:{REGION}:{account}:key/00000000-0000-0000-0000-000000000000"),
            "NotFoundException",
        ),
        (
            "unknown alias",
            "alias/no-such-alias".to_string(),
            "NotFoundException",
        ),
        (
            "ARN of another account",
            format!("arn:aws:kms:{REGION}:999999999999:key/{live_id}"),
            "NotFoundException",
        ),
        (
            "ARN of another resource type",
            format!("arn:aws:kms:{REGION}:{account}:thing/{live_id}"),
            "InvalidArnException",
        ),
        (
            "ARN with an empty resource",
            format!("arn:aws:kms:{REGION}:{account}:key/"),
            "InvalidArnException",
        ),
        (
            "key pending deletion",
            pending_id.clone(),
            "KMSInvalidStateException",
        ),
    ];

    for (case, identifier, expected_code) in cases {
        let (status, code) = failure(
            &router,
            "UpdateKeyDescription",
            &json!({"KeyId": identifier, "Description": "should not be written"}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a refused update, got {status} ({code})"
        );
        assert_eq!(
            code, expected_code,
            "case {case}: expected {expected_code}, got {code}"
        );
    }

    assert_eq!(
        describe(&router, &live_id).await["Description"],
        json!("untouched"),
        "a refused update must not write the description of any key"
    );
    assert_eq!(
        describe(&router, &pending_id).await["Description"],
        json!("pending"),
        "a key pending deletion keeps the description it had"
    );
}

/// `CreateKey` with `Origin` EXTERNAL mints a key with NO key material: it waits in
/// the `PendingImport` key state, is not enabled, reports no key material id, and
/// refuses every operation that needs material — while still being a real, described
/// key of the spec that was asked for.
///
/// The rows differ in the spec the pending key was created for.
#[tokio::test]
async fn create_key_with_external_origin_mints_a_key_awaiting_imported_material() {
    let router = router();

    let cases: Vec<(&str, Value, &str, &str, &[&str])> = vec![
        (
            "symmetric",
            json!({"Origin": "EXTERNAL", "Description": "to import"}),
            "SYMMETRIC_DEFAULT",
            "EncryptionAlgorithms",
            &["SYMMETRIC_DEFAULT"],
        ),
        (
            "asymmetric signing",
            json!({"Origin": "EXTERNAL", "KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
            "RSA_2048",
            "SigningAlgorithms",
            &[
                "RSASSA_PKCS1_V1_5_SHA_256",
                "RSASSA_PKCS1_V1_5_SHA_384",
                "RSASSA_PKCS1_V1_5_SHA_512",
                "RSASSA_PSS_SHA_256",
                "RSASSA_PSS_SHA_384",
                "RSASSA_PSS_SHA_512",
            ],
        ),
        (
            "HMAC",
            json!({"Origin": "EXTERNAL", "KeySpec": "HMAC_384",
                   "KeyUsage": "GENERATE_VERIFY_MAC"}),
            "HMAC_384",
            "MacAlgorithms",
            &["HMAC_SHA_384"],
        ),
    ];

    for (case, request, expected_spec, algorithms_member, algorithms) in cases {
        let metadata = create_key(&router, &request).await;
        let key_id = text(case, &metadata, "KeyId");

        assert_eq!(
            text(case, &metadata, "Origin"),
            "EXTERNAL",
            "case {case}: the key was asked for with no key material, got {metadata}"
        );
        assert_eq!(
            text(case, &metadata, "KeyState"),
            "PendingImport",
            "case {case}: a key with no key material waits for the import, got {metadata}"
        );
        assert_eq!(
            metadata["Enabled"],
            json!(false),
            "case {case}: a key with no key material is not enabled, got {metadata}"
        );
        assert!(
            metadata["CurrentKeyMaterialId"].is_null(),
            "case {case}: there is no key material to identify yet, got {metadata}"
        );
        // The capability the caller asked for is still recorded.
        assert_eq!(
            text(case, &metadata, "KeySpec"),
            expected_spec,
            "case {case}: expected KeySpec {expected_spec}, got {metadata}"
        );
        assert_eq!(
            metadata[algorithms_member],
            json!(algorithms),
            "case {case}: expected {algorithms_member} {algorithms:?}, got {metadata}"
        );
        assert_eq!(
            describe(&router, &key_id).await,
            metadata,
            "case {case}: DescribeKey must report what CreateKey returned"
        );

        // Nothing can use, enable or disable a key that has no key material.
        for (operation, body) in [
            (
                "Encrypt",
                json!({"KeyId": key_id, "Plaintext": b64(b"not yet")}),
            ),
            ("EnableKey", json!({"KeyId": key_id})),
            ("DisableKey", json!({"KeyId": key_id})),
        ] {
            let (status, code) = failure(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400 from {operation} on a PendingImport key, \
                 got {status} ({code})"
            );
            assert_eq!(
                code, "KMSInvalidStateException",
                "case {case}: expected KMSInvalidStateException from {operation}, got {code}"
            );
        }

        assert_eq!(
            text(case, &describe(&router, &key_id).await, "KeyState"),
            "PendingImport",
            "case {case}: a refused operation must leave the key state where it was"
        );
    }
}

/// A cryptographic operation over a key whose material this emulator does not hold is
/// answered as not implemented or with the modelled key-usage error — never with a
/// result nothing computed.
#[tokio::test]
async fn cryptographic_operations_over_a_non_symmetric_key_are_never_fabricated() {
    let router = router();

    let rsa_encrypt = create_key(
        &router,
        &json!({"KeySpec": "RSA_2048", "KeyUsage": "ENCRYPT_DECRYPT"}),
    )
    .await;
    let rsa_sign = create_key(
        &router,
        &json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let hmac = create_key(
        &router,
        &json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
    )
    .await;
    let symmetric = create_key(&router, &json!({"Description": "symmetric"})).await;

    let plaintext = b64(b"attack at dawn");
    let cases: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "Encrypt with an asymmetric encryption key",
            "Encrypt",
            json!({"KeyId": rsa_encrypt["KeyId"], "Plaintext": plaintext,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            501,
            "NotImplemented",
        ),
        (
            "Encrypt with a signing key",
            "Encrypt",
            json!({"KeyId": rsa_sign["KeyId"], "Plaintext": plaintext}),
            400,
            "InvalidKeyUsageException",
        ),
        (
            "Encrypt with an HMAC key",
            "Encrypt",
            json!({"KeyId": hmac["KeyId"], "Plaintext": plaintext}),
            400,
            "InvalidKeyUsageException",
        ),
        (
            "Sign with a signing key",
            "Sign",
            json!({"KeyId": rsa_sign["KeyId"], "Message": plaintext,
                   "SigningAlgorithm": "RSASSA_PSS_SHA_256"}),
            501,
            "NotImplemented",
        ),
        (
            "GenerateMac with an HMAC key",
            "GenerateMac",
            json!({"KeyId": hmac["KeyId"], "Message": plaintext,
                   "MacAlgorithm": "HMAC_SHA_256"}),
            501,
            "NotImplemented",
        ),
    ];

    for (case, operation, body, expected_status, expected_code) in cases {
        let (status, code) = failure(&router, operation, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} ({code})"
        );
        assert_eq!(
            code, expected_code,
            "case {case}: expected {expected_code}, got {code}"
        );
    }

    // The symmetric key is unaffected: the key this service DOES hold material for
    // still encrypts.
    let sealed = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": symmetric["KeyId"], "Plaintext": plaintext}),
    )
    .await;
    assert!(
        sealed["CiphertextBlob"].is_string(),
        "a symmetric encryption key must still produce a ciphertext, got {sealed}"
    );
}

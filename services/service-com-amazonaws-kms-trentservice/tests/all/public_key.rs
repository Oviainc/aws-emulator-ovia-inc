// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetPublicKey`, driven over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: for an asymmetric KMS key in a usable key state,
//! `GetPublicKey` reports the PUBLIC half of that key's own key pair as a DER-encoded
//! X.509 `SubjectPublicKeyInfo`, together with the key's ARN, its `KeySpec` (and the
//! deprecated `CustomerMasterKeySpec` for every spec that enum can name), its
//! `KeyUsage` and exactly the one algorithm list that usage selects; a key that has no
//! public half is refused with the modelled error for why; and the call only reports —
//! it changes nothing a later caller sees.
//!
//! Three suites assert it over TABLES rather than one call:
//!
//! 1. every asymmetric `KeySpec` the model declares, under each `KeyUsage` that spec
//!    accepts, with the published key checked against the algorithm the spec names;
//! 2. a key whose key pair the CALLER imported, where the test holds the private key
//!    and so knows byte for byte which public key must come back;
//! 3. the refusals, over every kind of key and identifier that has no public half.

use serde_json::{json, Value};

use super::harness::{
    b64, call, contains, create_key, describe, ok, refused, router, text, unb64, wrap, REGION,
};

/// One row of the spec table: an asymmetric `KeySpec`, the `KeyUsage` the key is
/// created with, the algorithm list the answer must carry under that usage, the
/// `CustomerMasterKeySpec` the answer reports (absent for a spec the deprecated enum
/// cannot name) and the DER `OBJECT IDENTIFIER` the published key must carry.
struct SpecCase {
    key_spec: &'static str,
    key_usage: &'static str,
    algorithms_member: &'static str,
    algorithms: &'static [&'static str],
    customer_master_key_spec: Option<&'static str>,
    /// The RSA modulus size in bits, for a row whose published key is an RSA key, and
    /// `None` for every other family.
    rsa_modulus_bits: Option<usize>,
    object_identifier: &'static [u8],
}

/// The three per-usage algorithm lists a `GetPublicKey` answer can carry
/// (`smithy_query(query="operation",operation="GetPublicKey")`). Exactly one of them is
/// present, and it is the one belonging to the key's own `KeyUsage`.
const ALGORITHM_MEMBERS: [&str; 3] = [
    "EncryptionAlgorithms",
    "SigningAlgorithms",
    "KeyAgreementAlgorithms",
];

// The DER encoding of the `OBJECT IDENTIFIER` that identifies each published key's
// algorithm inside its `SubjectPublicKeyInfo` (RFC 5280 §4.1.2.7). An RSA key is
// identified by `rsaEncryption` and distinguished further by its modulus size; a NIST
// or secp curve key by its own named-curve identifier, which differs per key spec; an
// Edwards25519 key by `id-Ed25519` (RFC 8410 §3); an SM2 key by the `sm2p256v1` curve
// identifier of GM/T 0003-2012.

/// `rsaEncryption`, 1.2.840.113549.1.1.1.
const OID_RSA_ENCRYPTION: &[u8] = &[0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01];
/// `prime256v1` (secp256r1), 1.2.840.10045.3.1.7.
const OID_PRIME256V1: &[u8] = &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
/// `secp384r1`, 1.3.132.0.34.
const OID_SECP384R1: &[u8] = &[0x06, 0x05, 0x2B, 0x81, 0x04, 0x00, 0x22];
/// `secp521r1`, 1.3.132.0.35.
const OID_SECP521R1: &[u8] = &[0x06, 0x05, 0x2B, 0x81, 0x04, 0x00, 0x23];
/// `secp256k1`, 1.3.132.0.10.
const OID_SECP256K1: &[u8] = &[0x06, 0x05, 0x2B, 0x81, 0x04, 0x00, 0x0A];
/// `id-Ed25519`, 1.3.101.112.
const OID_ED25519: &[u8] = &[0x06, 0x03, 0x2B, 0x65, 0x70];
/// `sm2p256v1`, 1.2.156.10197.1.301.
const OID_SM2: &[u8] = &[0x06, 0x08, 0x2A, 0x81, 0x1C, 0xCF, 0x55, 0x01, 0x82, 0x2D];

/// Every `KeySpec` of `smithy_query(query="shape",shape="KeySpec")` whose material is a
/// key pair — RSA at each modulus size, every NIST curve, secp256k1, Edwards25519 and
/// SM2 — under each `KeyUsage`
/// `smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")` says that spec
/// accepts, with the algorithms the "Key spec reference" gives for the pair. The
/// `CustomerMasterKeySpec` column is the value list of
/// `smithy_query(query="shape",shape="CustomerMasterKeySpec")`, the deprecated enum,
/// which names no Edwards25519 key.
///
/// The ML-DSA specs are deliberately NOT here: no library in this workspace produces an
/// ML-DSA key pair, so such a key holds no private key to publish the public half of —
/// see [`ml_dsa_keys_have_no_published_public_key`].
const SPEC_CASES: &[SpecCase] = &[
    SpecCase {
        key_spec: "RSA_2048",
        key_usage: "ENCRYPT_DECRYPT",
        algorithms_member: "EncryptionAlgorithms",
        algorithms: &["RSAES_OAEP_SHA_1", "RSAES_OAEP_SHA_256"],
        customer_master_key_spec: Some("RSA_2048"),
        rsa_modulus_bits: Some(2048),
        object_identifier: OID_RSA_ENCRYPTION,
    },
    SpecCase {
        key_spec: "RSA_2048",
        key_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &[
            "RSASSA_PKCS1_V1_5_SHA_256",
            "RSASSA_PKCS1_V1_5_SHA_384",
            "RSASSA_PKCS1_V1_5_SHA_512",
            "RSASSA_PSS_SHA_256",
            "RSASSA_PSS_SHA_384",
            "RSASSA_PSS_SHA_512",
        ],
        customer_master_key_spec: Some("RSA_2048"),
        rsa_modulus_bits: Some(2048),
        object_identifier: OID_RSA_ENCRYPTION,
    },
    SpecCase {
        key_spec: "RSA_3072",
        key_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &[
            "RSASSA_PKCS1_V1_5_SHA_256",
            "RSASSA_PKCS1_V1_5_SHA_384",
            "RSASSA_PKCS1_V1_5_SHA_512",
            "RSASSA_PSS_SHA_256",
            "RSASSA_PSS_SHA_384",
            "RSASSA_PSS_SHA_512",
        ],
        customer_master_key_spec: Some("RSA_3072"),
        rsa_modulus_bits: Some(3072),
        object_identifier: OID_RSA_ENCRYPTION,
    },
    SpecCase {
        key_spec: "RSA_4096",
        key_usage: "ENCRYPT_DECRYPT",
        algorithms_member: "EncryptionAlgorithms",
        algorithms: &["RSAES_OAEP_SHA_1", "RSAES_OAEP_SHA_256"],
        customer_master_key_spec: Some("RSA_4096"),
        rsa_modulus_bits: Some(4096),
        object_identifier: OID_RSA_ENCRYPTION,
    },
    SpecCase {
        key_spec: "ECC_NIST_P256",
        key_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ECDSA_SHA_256"],
        customer_master_key_spec: Some("ECC_NIST_P256"),
        rsa_modulus_bits: None,
        object_identifier: OID_PRIME256V1,
    },
    SpecCase {
        key_spec: "ECC_NIST_P256",
        key_usage: "KEY_AGREEMENT",
        algorithms_member: "KeyAgreementAlgorithms",
        algorithms: &["ECDH"],
        customer_master_key_spec: Some("ECC_NIST_P256"),
        rsa_modulus_bits: None,
        object_identifier: OID_PRIME256V1,
    },
    SpecCase {
        key_spec: "ECC_NIST_P384",
        key_usage: "KEY_AGREEMENT",
        algorithms_member: "KeyAgreementAlgorithms",
        algorithms: &["ECDH"],
        customer_master_key_spec: Some("ECC_NIST_P384"),
        rsa_modulus_bits: None,
        object_identifier: OID_SECP384R1,
    },
    SpecCase {
        key_spec: "ECC_NIST_P521",
        key_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ECDSA_SHA_512"],
        customer_master_key_spec: Some("ECC_NIST_P521"),
        rsa_modulus_bits: None,
        object_identifier: OID_SECP521R1,
    },
    SpecCase {
        key_spec: "ECC_SECG_P256K1",
        key_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ECDSA_SHA_256"],
        customer_master_key_spec: Some("ECC_SECG_P256K1"),
        rsa_modulus_bits: None,
        object_identifier: OID_SECP256K1,
    },
    SpecCase {
        key_spec: "ECC_NIST_EDWARDS25519",
        key_usage: "SIGN_VERIFY",
        algorithms_member: "SigningAlgorithms",
        algorithms: &["ED25519_PH_SHA_512", "ED25519_SHA_512"],
        // The deprecated enum was frozen before this spec existed, so the answer
        // reports the spec through `KeySpec` alone.
        customer_master_key_spec: None,
        rsa_modulus_bits: None,
        object_identifier: OID_ED25519,
    },
    SpecCase {
        key_spec: "SM2",
        key_usage: "ENCRYPT_DECRYPT",
        algorithms_member: "EncryptionAlgorithms",
        algorithms: &["SM2PKE"],
        customer_master_key_spec: Some("SM2"),
        rsa_modulus_bits: None,
        object_identifier: OID_SM2,
    },
    SpecCase {
        key_spec: "SM2",
        key_usage: "KEY_AGREEMENT",
        algorithms_member: "KeyAgreementAlgorithms",
        algorithms: &["ECDH"],
        customer_master_key_spec: Some("SM2"),
        rsa_modulus_bits: None,
        object_identifier: OID_SM2,
    },
];

/// The largest `PublicKey` the model accepts: `PublicKeyType` is `@length(1..8192)`
/// (`smithy_query(query="shape",shape="PublicKeyType")`).
const MAX_PUBLIC_KEY_LEN: usize = 8192;

/// The DER tag of a `SEQUENCE`, which is what a `SubjectPublicKeyInfo` is.
const DER_SEQUENCE: u8 = 0x30;

/// `GetPublicKey` for an identifier, requiring success.
async fn public_key_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
) -> Value {
    ok(router, "GetPublicKey", &json!({"KeyId": identifier})).await
}

/// Assert the answer's own description of the key matches `case`'s row, and that
/// exactly the one algorithm list that key's usage selects is present.
fn assert_describes_the_key(case: &str, row: &SpecCase, key_arn: &str, answer: &Value) {
    assert_eq!(
        answer["KeyId"].as_str().unwrap_or_default(),
        key_arn,
        "case {case}: expected the key ARN {key_arn}, got {answer}"
    );
    assert_eq!(
        answer["KeySpec"].as_str().unwrap_or_default(),
        row.key_spec,
        "case {case}: expected KeySpec {}, got {answer}",
        row.key_spec
    );
    assert_eq!(
        answer["CustomerMasterKeySpec"].as_str(),
        row.customer_master_key_spec,
        "case {case}: expected CustomerMasterKeySpec {:?}, got {answer}",
        row.customer_master_key_spec
    );
    assert_eq!(
        answer["KeyUsage"].as_str().unwrap_or_default(),
        row.key_usage,
        "case {case}: expected KeyUsage {}, got {answer}",
        row.key_usage
    );
    let reported: Vec<String> = answer[row.algorithms_member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected {} in {answer}", row.algorithms_member))
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, row.algorithms,
        "case {case}: expected {} {:?}, got {reported:?}",
        row.algorithms_member, row.algorithms
    );
    for member in ALGORITHM_MEMBERS {
        if member == row.algorithms_member {
            continue;
        }
        assert!(
            answer[member].is_null(),
            "case {case}: {member} belongs to another KeyUsage and must be absent, got {answer}"
        );
    }
}

/// Assert `public_key` is a `SubjectPublicKeyInfo` of the key pair `row` names.
fn assert_is_public_key_of(case: &str, row: &SpecCase, public_key: &[u8]) {
    assert!(
        (1..=MAX_PUBLIC_KEY_LEN).contains(&public_key.len()),
        "case {case}: expected a PublicKey within the modelled 1..{MAX_PUBLIC_KEY_LEN} \
         bytes, got {}",
        public_key.len()
    );
    assert_eq!(
        public_key.first().copied(),
        Some(DER_SEQUENCE),
        "case {case}: expected a DER SEQUENCE, got a first byte of {:?}",
        public_key.first()
    );
    assert!(
        contains(public_key, row.object_identifier),
        "case {case}: expected the published key to carry the object identifier \
         {:02X?}, got {:02X?}",
        row.object_identifier,
        public_key
    );
    if let Some(modulus_bits) = row.rsa_modulus_bits {
        let parsed = aws_lc_rs::rsa::PublicEncryptingKey::from_der(public_key)
            .unwrap_or_else(|error| panic!("case {case}: the published key is an RSA key: {error}"));
        assert_eq!(
            parsed.key_size_bits(),
            modulus_bits,
            "case {case}: expected a {modulus_bits}-bit modulus, got {}",
            parsed.key_size_bits()
        );
    }
}

/// Rule: every asymmetric KMS key publishes the public half of its own key pair, with
/// the spec, usage and algorithms that key carries.
#[tokio::test]
async fn get_public_key_publishes_the_public_half_of_every_asymmetric_key_spec() {
    let router = router();

    for row in SPEC_CASES {
        let case = format!("{} {}", row.key_spec, row.key_usage);
        let case = case.as_str();
        let metadata = create_key(
            &router,
            &json!({"KeySpec": row.key_spec, "KeyUsage": row.key_usage}),
        )
        .await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");

        let answer = public_key_of(&router, &key_id).await;
        assert_describes_the_key(case, row, &key_arn, &answer);
        let public_key = unb64(case, &answer, "PublicKey");
        assert_is_public_key_of(case, row, &public_key);

        // The algorithm lists are the key's own, so the reader of the key metadata
        // reports the same ones.
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described[row.algorithms_member], answer[row.algorithms_member],
            "case {case}: DescribeKey and GetPublicKey must report one list, got {described} \
             and {answer}"
        );

        // Reading the public half is a report: every identifier of the key answers the
        // same bytes, however often it is asked.
        for identifier in [key_id.as_str(), key_arn.as_str()] {
            let again = public_key_of(&router, identifier).await;
            assert_eq!(
                unb64(case, &again, "PublicKey"),
                public_key,
                "case {case}: the public key of {identifier} must not change between reads"
            );
        }

        // The key pair belongs to the KEY: another key of the same spec has another
        // one, so nothing here is a constant published for every caller.
        let other = create_key(
            &router,
            &json!({"KeySpec": row.key_spec, "KeyUsage": row.key_usage}),
        )
        .await;
        let other_key_id = text(case, &other, "KeyId");
        let other_public_key = unb64(case, &public_key_of(&router, &other_key_id).await, "PublicKey");
        assert_ne!(
            other_public_key, public_key,
            "case {case}: two keys of one spec must not share a key pair"
        );
    }
}

/// The wrapping this suite imports asymmetric key material under: the only one a
/// private key is long enough to need — "supported for wrapping RSA and ECC key
/// material" (`smithy_query(query="doc",shape="GetParametersForImportRequest",member="WrappingAlgorithm")`).
const WRAPPING_ALGORITHM: &str = "RSA_AES_KEY_WRAP_SHA_256";

/// A freshly generated key pair of the key pair `key_spec` names: the private half as
/// PKCS#8 DER, which is what a caller importing asymmetric key material wraps, and the
/// public half as X.509 `SubjectPublicKeyInfo` DER, which is what `GetPublicKey` must
/// then answer. Both come from the library that implements the algorithm, taken from the
/// generated pair itself, so the expectation is computed here rather than restated from
/// the service.
fn key_pair_of(case: &str, key_spec: &str) -> (Vec<u8>, Vec<u8>) {
    use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der, PublicKeyX509Der};
    use aws_lc_rs::signature::{
        EcdsaKeyPair, Ed25519KeyPair, KeyPair as _, ECDSA_P384_SHA384_ASN1_SIGNING,
    };
    use sm2::pkcs8::{EncodePrivateKey, EncodePublicKey};

    match key_spec {
        "RSA_2048" => {
            let private = aws_lc_rs::rsa::PrivateDecryptingKey::generate(
                aws_lc_rs::rsa::KeySize::Rsa2048,
            )
            .unwrap_or_else(|error| panic!("case {case}: an RSA key pair generates: {error}"));
            let public = private.public_key();
            let private_der: Pkcs8V1Der<'static> = AsDer::as_der(&private)
                .unwrap_or_else(|error| panic!("case {case}: the private key encodes: {error}"));
            let public_der: PublicKeyX509Der<'static> = AsDer::as_der(&public)
                .unwrap_or_else(|error| panic!("case {case}: the public key encodes: {error}"));
            (
                private_der.as_ref().to_vec(),
                public_der.as_ref().to_vec(),
            )
        }
        "ECC_NIST_P384" => {
            let pair = EcdsaKeyPair::generate(&ECDSA_P384_SHA384_ASN1_SIGNING)
                .unwrap_or_else(|error| panic!("case {case}: an EC key pair generates: {error}"));
            let public_der: PublicKeyX509Der<'static> = AsDer::as_der(pair.public_key())
                .unwrap_or_else(|error| panic!("case {case}: the public key encodes: {error}"));
            (
                pair.to_pkcs8v1()
                    .unwrap_or_else(|error| {
                        panic!("case {case}: the private key encodes: {error}")
                    })
                    .as_ref()
                    .to_vec(),
                public_der.as_ref().to_vec(),
            )
        }
        "ECC_NIST_EDWARDS25519" => {
            let pair = Ed25519KeyPair::generate().unwrap_or_else(|error| {
                panic!("case {case}: an Ed25519 key pair generates: {error}")
            });
            let public_der: PublicKeyX509Der<'static> = AsDer::as_der(pair.public_key())
                .unwrap_or_else(|error| panic!("case {case}: the public key encodes: {error}"));
            (
                pair.to_pkcs8v1()
                    .unwrap_or_else(|error| {
                        panic!("case {case}: the private key encodes: {error}")
                    })
                    .as_ref()
                    .to_vec(),
                public_der.as_ref().to_vec(),
            )
        }
        "SM2" => {
            let scalar = service_com_amazonaws_kms_trentservice::state::random_bytes(32)
                .unwrap_or_else(|| panic!("case {case}: the platform produces entropy"));
            let secret = sm2::SecretKey::from_slice(&scalar)
                .unwrap_or_else(|error| panic!("case {case}: the scalar is an SM2 key: {error}"));
            (
                secret
                    .to_pkcs8_der()
                    .unwrap_or_else(|error| {
                        panic!("case {case}: the private key encodes: {error}")
                    })
                    .as_bytes()
                    .to_vec(),
                secret
                    .public_key()
                    .to_public_key_der()
                    .unwrap_or_else(|error| {
                        panic!("case {case}: the public key encodes: {error}")
                    })
                    .as_ref()
                    .to_vec(),
            )
        }
        other => panic!("case {case}: {other} is not a key spec this test generates"),
    }
}

/// Import `private_key` into the KMS key `key_id`, the documented way: wrapped under the
/// public wrapping key the import parameters hand out.
async fn import_private_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_id: &str,
    private_key: &[u8],
) {
    let parameters = ok(
        router,
        "GetParametersForImport",
        &json!({
            "KeyId": key_id,
            "WrappingAlgorithm": WRAPPING_ALGORITHM,
            "WrappingKeySpec": "RSA_2048",
        }),
    )
    .await;
    let wrapping_key = unb64(case, &parameters, "PublicKey");
    let import_token = text(case, &parameters, "ImportToken");
    let wrapped = wrap(case, &wrapping_key, WRAPPING_ALGORITHM, private_key);
    ok(
        router,
        "ImportKeyMaterial",
        &json!({
            "KeyId": key_id,
            "ImportToken": import_token,
            "EncryptedKeyMaterial": b64(&wrapped),
            "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
        }),
    )
    .await;
}

/// Rule, continued: the published half is the public half of the key's OWN private key.
/// For a key whose material the caller imported, the test holds that private key, so it
/// knows byte for byte which public key must come back.
#[tokio::test]
async fn an_imported_key_pair_publishes_its_own_public_half() {
    let router = router();

    // Rows: one key spec per asymmetric family — RSA, a NIST curve, Edwards25519 and
    // SM2 — each under a usage its spec accepts.
    let rows = [
        ("RSA_2048", "SIGN_VERIFY"),
        ("ECC_NIST_P384", "KEY_AGREEMENT"),
        ("ECC_NIST_EDWARDS25519", "SIGN_VERIFY"),
        ("SM2", "ENCRYPT_DECRYPT"),
    ];
    for (key_spec, key_usage) in rows {
        let case = key_spec;
        let metadata = create_key(
            &router,
            &json!({"Origin": "EXTERNAL", "KeySpec": key_spec, "KeyUsage": key_usage}),
        )
        .await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");

        // Before the material is imported the key has no key pair at all, so there is
        // no public half to report.
        let (status, code, _message) =
            refused(&router, REGION, "GetPublicKey", &json!({"KeyId": key_id})).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "KMSInvalidStateException"),
            "case {case}: a key still awaiting its material has no public key, got \
             {status} {code}"
        );

        let (private_key, expected_public_key) = key_pair_of(case, key_spec);
        import_private_key(&router, case, &key_id, &private_key).await;

        let answer = public_key_of(&router, &key_arn).await;
        assert_eq!(
            unb64(case, &answer, "PublicKey"),
            expected_public_key,
            "case {case}: expected the public half of the imported private key, got {answer}"
        );
        assert_eq!(
            answer["KeyId"].as_str().unwrap_or_default(),
            key_arn,
            "case {case}: expected the key ARN, got {answer}"
        );
        assert_eq!(
            answer["KeySpec"].as_str().unwrap_or_default(),
            key_spec,
            "case {case}: expected KeySpec {key_spec}, got {answer}"
        );
        assert_eq!(
            answer["KeyUsage"].as_str().unwrap_or_default(),
            key_usage,
            "case {case}: expected KeyUsage {key_usage}, got {answer}"
        );
    }
}

/// Rule, continued: a key with no public half is refused with the modelled error for
/// why, and nothing about the key changes.
#[tokio::test]
async fn get_public_key_refuses_every_key_that_has_no_public_half() {
    let router = router();

    let symmetric = create_key(&router, &json!({"Description": "symmetric"})).await;
    let hmac = create_key(
        &router,
        &json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
    )
    .await;
    let disabled = create_key(
        &router,
        &json!({"KeySpec": "ECC_NIST_P256", "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let disabled_key_id = text("disabled", &disabled, "KeyId");
    ok(&router, "DisableKey", &json!({"KeyId": disabled_key_id})).await;
    let pending_deletion = create_key(
        &router,
        &json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let pending_deletion_key_id = text("pending deletion", &pending_deletion, "KeyId");
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": pending_deletion_key_id, "PendingWindowInDays": 7}),
    )
    .await;

    // Rows: the identifier the request names and the refusal it must answer. They span
    // what has no public half — every family of key material, every key state a
    // cryptographic operation cannot run in — and the two kinds of identifier that name
    // no key at all.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a symmetric encryption key",
            text("symmetric", &symmetric, "KeyId"),
            "UnsupportedOperationException",
        ),
        (
            "an HMAC key",
            text("hmac", &hmac, "KeyId"),
            "InvalidKeyUsageException",
        ),
        (
            "a disabled asymmetric key",
            disabled_key_id.clone(),
            "DisabledException",
        ),
        (
            "an asymmetric key pending deletion",
            pending_deletion_key_id.clone(),
            "KMSInvalidStateException",
        ),
        (
            "a key id nothing is bound to",
            "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
            "NotFoundException",
        ),
        (
            "an alias nothing is bound to",
            "alias/nothing-here".to_string(),
            "NotFoundException",
        ),
        (
            "an ARN that is not a KMS key or alias ARN",
            "arn:aws:s3:::my-bucket".to_string(),
            "InvalidArnException",
        ),
        (
            "a key ARN whose account segment names no account",
            format!("arn:aws:kms:{REGION}:not-an-account:key/{disabled_key_id}"),
            "AccessDeniedException",
        ),
    ];
    for (case, identifier, expected_code) in rows {
        let (status, code, _message) = refused(
            &router,
            REGION,
            "GetPublicKey",
            &json!({"KeyId": identifier}),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case}: expected HTTP 400 {expected_code}, got {status} {code}"
        );
    }

    // A read that succeeds is still only a report: `GetPublicKey` is not one of the
    // operations a key's last-usage record tracks, so the record stays empty.
    let signing = create_key(
        &router,
        &json!({"KeySpec": "ECC_NIST_P521", "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let signing_key_id = text("last usage", &signing, "KeyId");
    public_key_of(&router, &signing_key_id).await;
    let last_usage = ok(
        &router,
        "GetKeyLastUsage",
        &json!({"KeyId": signing_key_id}),
    )
    .await;
    assert_eq!(
        last_usage["KeyLastUsage"],
        json!({}),
        "case last usage: downloading the public key is not a tracked use of the key, got \
         {last_usage}"
    );
    // And the key itself is untouched by the read.
    let described = describe(&router, &signing_key_id).await;
    assert_eq!(
        described["KeyState"].as_str().unwrap_or_default(),
        "Enabled",
        "case last usage: reading the public key must leave the key state where it was, \
         got {described}"
    );
}

/// Rule, continued: an ML-DSA KMS key is real, but no library in this workspace produces
/// an ML-DSA key pair, so the key holds no private key and the download says it is not
/// implemented rather than publishing a key nothing holds the private half of.
#[tokio::test]
async fn ml_dsa_keys_have_no_published_public_key() {
    let router = router();

    // Every ML-DSA `KeySpec` of `smithy_query(query="shape",shape="KeySpec")`.
    for key_spec in ["ML_DSA_44", "ML_DSA_65", "ML_DSA_87"] {
        let metadata = create_key(
            &router,
            &json!({"KeySpec": key_spec, "KeyUsage": "SIGN_VERIFY"}),
        )
        .await;
        let key_id = text(key_spec, &metadata, "KeyId");
        let (status, payload) = call(
            &router,
            REGION,
            "GetPublicKey",
            &json!({"KeyId": key_id}),
        )
        .await;
        assert_eq!(
            status, 501,
            "case {key_spec}: expected HTTP 501, got {status} with {payload}"
        );
    }
}

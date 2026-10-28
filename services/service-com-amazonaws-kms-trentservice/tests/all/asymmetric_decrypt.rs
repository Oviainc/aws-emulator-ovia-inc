// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `Decrypt` of a ciphertext produced OUTSIDE KMS, driven over the
//! wire through the shared [`super::harness`].
//!
//! The rule under test: for an asymmetric KMS key whose usage is encryption and
//! decryption, a ciphertext produced outside KMS under the public half `GetPublicKey`
//! published, with one of the `EncryptionAlgorithms` that key advertises, is opened by
//! `Decrypt` with the private half KMS holds — the answer carries the original bytes, the
//! key's ARN (whatever identifier the request used) and the algorithm it was opened
//! under, and no `KeyMaterialId`, which "is present only when the operation uses a
//! symmetric encryption KMS key". The declared algorithm is a CRYPTOGRAPHIC parameter,
//! not a label: the same bytes declared under the key's other advertised algorithm do not
//! open and are refused like any ciphertext that key did not receive. No `Encrypt` call
//! is involved anywhere — the ciphertext is produced entirely from the published key, as
//! the party holding only the public half does.
//!
//! The suites assert it over TABLES rather than one call: every RSA `KeySpec` such a key
//! can have, under each algorithm that spec advertises, over plaintexts from the shortest
//! the model allows to the longest that spec and algorithm can carry, with the request
//! naming the key in each of the three ways it can; then the refusals, whose rows differ
//! in WHY the ciphertext cannot be opened — a mis-declared digest, another key's
//! ciphertext, bytes that are no ciphertext, an algorithm the spec does not advertise, a
//! key whose usage is not encryption, a key state that forbids the call, and a scheme no
//! library here performs.

use serde_json::{json, Value};

use super::harness::{
    b64, contains, create_alias, create_key, describe, failure, ok, refused, text, unb64, wrap,
    REGION,
};

/// One row of the key-spec catalog: an RSA `KeySpec` an asymmetric encryption KMS key can
/// have, the size of its modulus in bytes (which is the length of every ciphertext it
/// receives), and the longest plaintext each `EncryptionAlgorithmSpec` it advertises can
/// carry.
struct SpecCase {
    key_spec: &'static str,
    modulus_bytes: usize,
    /// Each advertised algorithm with the longest plaintext it encrypts under this spec.
    algorithms: &'static [(&'static str, usize)],
}

/// Every RSA `KeySpec` of `smithy_query(query="shape",shape="KeySpec")`, with the maximum
/// plaintext size table of
/// `smithy_query(query="doc",shape="Encrypt")`: "RSA_2048 RSAES_OAEP_SHA_1: 214 bytes
/// RSAES_OAEP_SHA_256: 190 bytes RSA_3072 RSAES_OAEP_SHA_1: 342 bytes RSAES_OAEP_SHA_256:
/// 318 bytes RSA_4096 RSAES_OAEP_SHA_1: 470 bytes RSAES_OAEP_SHA_256: 446 bytes".
///
/// The algorithm lists are the `EncryptionAlgorithms` such a key advertises, which the
/// first suite below re-reads from `GetPublicKey` rather than trusting this table.
const SPEC_CASES: &[SpecCase] = &[
    SpecCase {
        key_spec: "RSA_2048",
        modulus_bytes: 256,
        algorithms: &[("RSAES_OAEP_SHA_1", 214), ("RSAES_OAEP_SHA_256", 190)],
    },
    SpecCase {
        key_spec: "RSA_3072",
        modulus_bytes: 384,
        algorithms: &[("RSAES_OAEP_SHA_1", 342), ("RSAES_OAEP_SHA_256", 318)],
    },
    SpecCase {
        key_spec: "RSA_4096",
        modulus_bytes: 512,
        algorithms: &[("RSAES_OAEP_SHA_1", 470), ("RSAES_OAEP_SHA_256", 446)],
    },
];

/// The two algorithms an RSA encryption KMS key advertises, in the order KMS reports
/// them (`smithy_query(query="shape",shape="EncryptionAlgorithmSpec")` names both, plus
/// the SM2 and symmetric values no RSA key supports).
const RSA_ENCRYPTION_ALGORITHMS: &[&str] = &["RSAES_OAEP_SHA_1", "RSAES_OAEP_SHA_256"];

/// Create an asymmetric encryption KMS key of `key_spec` and report its key id, its ARN
/// and the DER `SubjectPublicKeyInfo` `GetPublicKey` publishes for it.
async fn encryption_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_spec: &str,
) -> (String, String, Vec<u8>) {
    let metadata = create_key(
        router,
        &json!({"KeySpec": key_spec, "KeyUsage": "ENCRYPT_DECRYPT"}),
    )
    .await;
    let key_id = text(case, &metadata, "KeyId");
    let arn = text(case, &metadata, "Arn");
    let published = ok(router, "GetPublicKey", &json!({"KeyId": &arn})).await;
    let public_key = unb64(case, &published, "PublicKey");
    assert_eq!(
        published["KeySpec"],
        json!(key_spec),
        "case {case}: the published key must report the key's own spec, got {published}"
    );
    assert_eq!(
        published["EncryptionAlgorithms"],
        json!(RSA_ENCRYPTION_ALGORITHMS),
        "case {case}: an RSA encryption key advertises both OAEP algorithms, got {published}"
    );
    (key_id, arn, public_key)
}

/// `len` bytes from the emulator's own entropy, so no two rows encrypt the same
/// plaintext and no plaintext is a literal written here.
fn plaintext(case: &str, len: usize) -> Vec<u8> {
    service_com_amazonaws_kms_trentservice::state::random_bytes(len)
        .unwrap_or_else(|| panic!("case {case}: the platform produces entropy"))
}

/// Encrypt `bytes` under the published public key with `encryption_algorithm`, with no
/// KMS call at all — RSA-OAEP, MGF1 and the message digest both the one the algorithm
/// names, and no label.
///
/// It is [`wrap`], whose two `RSAES_OAEP_*` algorithms are exactly that encryption:
/// "you encrypt your key material directly with the RSA public key from KMS".
fn encrypt_outside_kms(
    case: &str,
    public_key: &[u8],
    encryption_algorithm: &str,
    bytes: &[u8],
) -> Vec<u8> {
    wrap(case, public_key, encryption_algorithm, bytes)
}

/// The OAEP parameters an `EncryptionAlgorithmSpec` names, for the probe below.
fn oaep_algorithm(case: &str, encryption_algorithm: &str) -> &'static aws_lc_rs::rsa::OaepAlgorithm {
    match encryption_algorithm {
        "RSAES_OAEP_SHA_1" => &aws_lc_rs::rsa::OAEP_SHA1_MGF1SHA1,
        "RSAES_OAEP_SHA_256" => &aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256,
        other => panic!("case {case}: {other} is not an algorithm an RSA key advertises"),
    }
}

/// Whether the published public key REFUSES to encrypt `len` bytes under
/// `encryption_algorithm`.
///
/// The OAEP encoding of RFC 8017 carries at most `k - 2*hLen - 2` bytes, which is where
/// each maximum in [`SPEC_CASES`] comes from: one byte past it, there is no ciphertext to
/// present to `Decrypt` at all.
fn refuses_to_encrypt(
    case: &str,
    public_key: &[u8],
    encryption_algorithm: &str,
    len: usize,
) -> bool {
    let oaep = super::harness::oaep_wrapping_key(case, public_key);
    let mut ciphertext = vec![0u8; oaep.key_size_bytes()];
    oaep.encrypt(
        oaep_algorithm(case, encryption_algorithm),
        &vec![0u8; len],
        &mut ciphertext,
        None,
    )
    .is_err()
}

/// A ciphertext produced outside KMS under the published public key decrypts inside KMS,
/// for every RSA key spec, every algorithm that spec advertises, and plaintexts spanning
/// the lengths each of them can carry.
#[tokio::test]
async fn a_ciphertext_produced_outside_kms_decrypts_under_an_advertised_algorithm() {
    let router = super::harness::router();

    for spec in SPEC_CASES {
        let (key_id, arn, public_key) = encryption_key(&router, spec.key_spec, spec.key_spec).await;
        let origin = text(spec.key_spec, &describe(&router, &arn).await, "Origin");

        for (algorithm, longest) in spec.algorithms {
            // Rows: the shortest plaintext `PlaintextType`'s `len 1..4096` allows, a
            // middling one, and the longest this spec and algorithm can carry.
            for len in [1usize, 32, *longest] {
                let case = format!("{} {algorithm} over {len} bytes", spec.key_spec);
                let bytes = plaintext(&case, len);
                let ciphertext = encrypt_outside_kms(&case, &public_key, algorithm, &bytes);
                assert_eq!(
                    ciphertext.len(),
                    spec.modulus_bytes,
                    "case {case}: an RSA ciphertext occupies exactly one modulus, got {} bytes",
                    ciphertext.len()
                );
                if len >= 8 {
                    assert!(
                        !contains(&ciphertext, &bytes),
                        "case {case}: the ciphertext must not carry the plaintext"
                    );
                }

                let answer = ok(
                    &router,
                    "Decrypt",
                    &json!({"CiphertextBlob": b64(&ciphertext), "KeyId": &arn,
                            "EncryptionAlgorithm": algorithm}),
                )
                .await;
                assert_eq!(
                    unb64(&case, &answer, "Plaintext"),
                    bytes,
                    "case {case}: Decrypt must answer the exact bytes encrypted"
                );
                assert_eq!(
                    answer["KeyId"],
                    json!(arn),
                    "case {case}: Decrypt names the key ARN, got {answer}"
                );
                assert_eq!(
                    answer["EncryptionAlgorithm"],
                    json!(algorithm),
                    "case {case}: Decrypt echoes the algorithm it opened under, got {answer}"
                );
                assert!(
                    answer["KeyMaterialId"].is_null(),
                    "case {case}: KeyMaterialId belongs to a symmetric encryption key alone, \
                     got {answer}"
                );
                assert_eq!(
                    answer["KeyOrigin"],
                    json!(origin),
                    "case {case}: Decrypt reports the origin DescribeKey reports, got {answer}"
                );
            }

            // One byte past the published maximum there is no such ciphertext: the OAEP
            // encoding itself cannot carry it.
            let case = format!("{} {algorithm} past its maximum", spec.key_spec);
            assert!(
                refuses_to_encrypt(&case, &public_key, algorithm, longest + 1),
                "case {case}: {} bytes must not encode under {algorithm}",
                longest + 1
            );
        }

        // The answer names the key ARN whatever identifier the request used, so a caller
        // that holds an alias or a bare key id still learns which key opened it.
        let case = format!("{} by each identifier", spec.key_spec);
        let alias = format!("alias/outside-{}", spec.key_spec.to_lowercase());
        create_alias(&router, &case, &alias, &key_id).await;
        let bytes = plaintext(&case, 24);
        let ciphertext = encrypt_outside_kms(&case, &public_key, "RSAES_OAEP_SHA_256", &bytes);
        for identifier in [key_id.clone(), arn.clone(), alias.clone()] {
            let answer = ok(
                &router,
                "Decrypt",
                &json!({"CiphertextBlob": b64(&ciphertext), "KeyId": &identifier,
                        "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            )
            .await;
            assert_eq!(
                unb64(&case, &answer, "Plaintext"),
                bytes,
                "case {case} ({identifier}): Decrypt must answer the exact bytes encrypted"
            );
            assert_eq!(
                answer["KeyId"],
                json!(arn),
                "case {case} ({identifier}): Decrypt answers the resolved key ARN, got {answer}"
            );
        }
    }
}

/// The declared algorithm is a cryptographic parameter: the same bytes presented under
/// the key's OTHER advertised algorithm do not open, and the failed attempt consumes
/// nothing.
#[tokio::test]
async fn the_declared_algorithm_decides_whether_the_ciphertext_opens() {
    let router = super::harness::router();

    for spec in SPEC_CASES {
        let (_, arn, public_key) = encryption_key(&router, spec.key_spec, spec.key_spec).await;

        // Rows: which advertised algorithm produced the ciphertext — the other one is
        // then the mis-declaration.
        for (produced_under, _) in spec.algorithms {
            let declared = RSA_ENCRYPTION_ALGORITHMS
                .iter()
                .find(|algorithm| *algorithm != produced_under)
                .expect("the key advertises two algorithms");
            let case = format!("{} produced under {produced_under}, declared {declared}", spec.key_spec);
            let bytes = plaintext(&case, 48);
            let ciphertext = encrypt_outside_kms(&case, &public_key, produced_under, &bytes);

            let (status, code, message) = refused(
                &router,
                REGION,
                "Decrypt",
                &json!({"CiphertextBlob": b64(&ciphertext), "KeyId": &arn,
                        "EncryptionAlgorithm": declared}),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidCiphertextException"),
                "case {case}: expected the ciphertext refusal, got {status} {code:?}"
            );
            assert!(
                message.is_empty(),
                "case {case}: AWS words this refusal with no message at all, got {message:?}"
            );

            // The control, after the refusal: the SAME bytes under the algorithm they
            // were produced with still open, so the failed attempt consumed nothing and
            // the key is unchanged.
            let answer = ok(
                &router,
                "Decrypt",
                &json!({"CiphertextBlob": b64(&ciphertext), "KeyId": &arn,
                        "EncryptionAlgorithm": produced_under}),
            )
            .await;
            assert_eq!(
                unb64(&case, &answer, "Plaintext"),
                bytes,
                "case {case}: the ciphertext still opens under its own algorithm"
            );
        }

        // And the key itself is exactly as it was: the same spec, the same usable state,
        // and the same published public half the ciphertexts were produced from.
        let case = format!("{} after the refusals", spec.key_spec);
        let metadata = describe(&router, &arn).await;
        assert_eq!(
            (
                text(&case, &metadata, "KeySpec"),
                text(&case, &metadata, "KeyState")
            ),
            (spec.key_spec.to_string(), "Enabled".to_string()),
            "case {case}: a refused Decrypt changes nothing about the key, got {metadata}"
        );
        let published = ok(&router, "GetPublicKey", &json!({"KeyId": &arn})).await;
        assert_eq!(
            unb64(&case, &published, "PublicKey"),
            public_key,
            "case {case}: the published public half is the same on every read"
        );
    }
}

/// What `Decrypt` refuses for an asymmetric key, and why each refusal differs.
#[tokio::test]
async fn decrypt_refuses_what_an_asymmetric_key_cannot_open() {
    let router = super::harness::router();

    let (_, arn, public_key) = encryption_key(&router, "the key", "RSA_2048").await;
    let (_, other_arn, _) = encryption_key(&router, "another key", "RSA_2048").await;
    let signing = create_key(
        &router,
        &json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let signing_arn = text("a signing key", &signing, "Arn");
    let sm2 = create_key(
        &router,
        &json!({"KeySpec": "SM2", "KeyUsage": "ENCRYPT_DECRYPT"}),
    )
    .await;
    let sm2_arn = text("an SM2 key", &sm2, "Arn");

    let bytes = plaintext("the ciphertext", 16);
    let ciphertext = encrypt_outside_kms("the ciphertext", &public_key, "RSAES_OAEP_SHA_256", &bytes);

    // Rows: WHY the request cannot be served — the ciphertext is another key's, is no
    // ciphertext at all, or is the wrong length; the algorithm is not one the key spec
    // advertises (including the symmetric default an absent member means); the key's
    // usage is not encryption.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a ciphertext for another key of the same spec",
            json!({"CiphertextBlob": b64(&ciphertext), "KeyId": &other_arn,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "InvalidCiphertextException",
        ),
        (
            "zero bytes of the right length",
            json!({"CiphertextBlob": b64(&[0u8; 256]), "KeyId": &arn,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "InvalidCiphertextException",
        ),
        (
            "one byte more than the modulus",
            json!({"CiphertextBlob": b64(&[7u8; 257]), "KeyId": &arn,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "InvalidCiphertextException",
        ),
        (
            "a single byte",
            json!({"CiphertextBlob": b64(b"x"), "KeyId": &arn,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "InvalidCiphertextException",
        ),
        (
            "an algorithm the key spec does not advertise",
            json!({"CiphertextBlob": b64(&ciphertext), "KeyId": &arn,
                   "EncryptionAlgorithm": "SM2PKE"}),
            "InvalidKeyUsageException",
        ),
        (
            "no algorithm at all, which means the symmetric default",
            json!({"CiphertextBlob": b64(&ciphertext), "KeyId": &arn}),
            "InvalidKeyUsageException",
        ),
        (
            "a key whose usage is signing",
            json!({"CiphertextBlob": b64(&ciphertext), "KeyId": &signing_arn,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "InvalidKeyUsageException",
        ),
    ];
    for (case, body, expected) in rows {
        let (status, code, message) = refused(&router, REGION, "Decrypt", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected {expected}, got {status} {code:?} {message:?}"
        );
        if expected == "InvalidKeyUsageException" {
            assert!(
                !message.is_empty(),
                "case {case:?}: this refusal says what was incompatible, got {message:?}"
            );
        }
    }

    // An SM2 key pair is a real KMS key whose SM2PKE scheme no library here performs, so
    // the call is reported as not implemented rather than answered with a plaintext
    // nothing computed.
    let (status, code) = failure(
        &router,
        "Decrypt",
        &json!({"CiphertextBlob": b64(&[9u8; 97]), "KeyId": &sm2_arn,
                "EncryptionAlgorithm": "SM2PKE"}),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (501, "NotImplemented"),
        "case an SM2 ciphertext: expected the not-implemented signal, got {status} {code:?}"
    );

    // The key state is judged on this path too: a disabled key opens nothing, and the
    // ciphertext opens again once it is enabled.
    let answer = ok(&router, "DisableKey", &json!({"KeyId": &arn})).await;
    assert!(
        answer.is_null() || answer.as_object().is_some_and(|members| members.is_empty()),
        "case disabled: DisableKey answers an empty 200, got {answer}"
    );
    let (status, code, _) = refused(
        &router,
        REGION,
        "Decrypt",
        &json!({"CiphertextBlob": b64(&ciphertext), "KeyId": &arn,
                "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "DisabledException"),
        "case disabled: a disabled key decrypts nothing, got {status} {code:?}"
    );
    ok(&router, "EnableKey", &json!({"KeyId": &arn})).await;
    let answer = ok(
        &router,
        "Decrypt",
        &json!({"CiphertextBlob": b64(&ciphertext), "KeyId": &arn,
                "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
    )
    .await;
    assert_eq!(
        unb64("re-enabled", &answer, "Plaintext"),
        bytes,
        "case re-enabled: the ciphertext opens again once the key is enabled"
    );
}

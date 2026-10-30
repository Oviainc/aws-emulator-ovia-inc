// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `Encrypt` with an ASYMMETRIC KMS key, driven over the wire
//! through the shared [`super::harness`].
//!
//! The rule under test: for a KMS key whose usage is encryption and decryption, `Encrypt`
//! seals the plaintext under one of the `EncryptionAlgorithms` that key's spec advertises
//! — the AES-GCM envelope for a symmetric encryption key, and RSAES-OAEP under the public
//! half of the key pair for an asymmetric one, where "the maximum size of the data that
//! you can encrypt varies with the type of KMS key and the encryption algorithm that you
//! choose" (`smithy_query(query="doc",shape="Encrypt")`). The ciphertext of an asymmetric
//! key occupies exactly one modulus, carries no encryption context (the scheme binds
//! none), and is opened by `Decrypt` naming that key and the same algorithm — by nothing
//! else.
//!
//! Every assertion is about that rule over TABLES whose rows differ along what it
//! generalises over: the RSA `KeySpec` (so the modulus), the algorithm (so the digest),
//! the plaintext length from the shortest the model allows to the documented ceiling and
//! one byte past it, the identifier form the request names the key by, and \u2014 for the
//! refusals \u2014 which rule the request breaks.

use serde_json::{json, Value};

use super::harness::{
    b64, call, contains, create_alias, create_key, describe, error_code, error_message, failure,
    ok, refused, text, unb64, REGION,
};

/// One row of the key-spec catalog: an RSA `KeySpec` an asymmetric encryption KMS key can
/// have, the size of its modulus in bytes (the length of every ciphertext it produces),
/// and the longest plaintext each `EncryptionAlgorithmSpec` it advertises can carry.
struct SpecCase {
    key_spec: &'static str,
    modulus_bytes: usize,
    algorithms: &'static [(&'static str, usize)],
}

/// Every RSA `KeySpec` of `smithy_query(query="shape",shape="KeySpec")`, with the maximum
/// plaintext size table of `smithy_query(query="doc",shape="Encrypt")`: "RSA_2048
/// RSAES_OAEP_SHA_1: 214 bytes RSAES_OAEP_SHA_256: 190 bytes RSA_3072 RSAES_OAEP_SHA_1:
/// 342 bytes RSAES_OAEP_SHA_256: 318 bytes RSA_4096 RSAES_OAEP_SHA_1: 470 bytes
/// RSAES_OAEP_SHA_256: 446 bytes".
///
/// The emulator derives each ceiling from the modulus and the digest rather than from a
/// table, so these published numbers are what that derivation is measured against.
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

/// Create an asymmetric encryption KMS key of `key_spec` and report its key id and ARN.
async fn encryption_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_spec: &str,
) -> (String, String) {
    let metadata = create_key(
        router,
        &json!({"KeySpec": key_spec, "KeyUsage": "ENCRYPT_DECRYPT"}),
    )
    .await;
    (
        text(case, &metadata, "KeyId"),
        text(case, &metadata, "Arn"),
    )
}

/// `len` bytes from the emulator's own entropy, so no two rows encrypt the same plaintext
/// and no plaintext is a literal written here.
fn plaintext(case: &str, len: usize) -> Vec<u8> {
    service_com_amazonaws_kms_trentservice::state::random_bytes(len)
        .unwrap_or_else(|| panic!("case {case}: the platform produces entropy"))
}

/// `Encrypt` under an asymmetric key produces a ciphertext of one modulus that `Decrypt`
/// opens back to the plaintext — for every RSA key spec, every algorithm that spec
/// advertises, and plaintexts spanning the lengths each of them can carry.
#[tokio::test]
async fn encrypt_under_an_asymmetric_key_round_trips_through_decrypt() {
    let router = super::harness::router();

    for spec in SPEC_CASES {
        let (key_id, arn) = encryption_key(&router, spec.key_spec, spec.key_spec).await;
        // The algorithms the KEY advertises, read off the key rather than trusted from
        // the table above.
        let described = describe(&router, &arn).await;
        let advertised: Vec<String> = described["EncryptionAlgorithms"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let alias = format!("alias/encrypt-{}", spec.key_spec.to_lowercase());
        create_alias(&router, spec.key_spec, &alias, &key_id).await;

        for (algorithm, longest) in spec.algorithms {
            assert!(
                advertised.iter().any(|value| value == *algorithm),
                "case {}: the key must advertise {algorithm}, got {advertised:?}",
                spec.key_spec
            );
            // Rows: the shortest plaintext `PlaintextType`'s `len 1..4096` allows, a
            // middling one, and the longest this spec and algorithm can carry; each named
            // by a different identifier form, since the key is one resource whichever
            // name reaches it.
            for (len, identifier) in [
                (1usize, key_id.clone()),
                (32, arn.clone()),
                (*longest, alias.clone()),
            ] {
                let case = format!("{} {algorithm} over {len} bytes", spec.key_spec);
                let bytes = plaintext(&case, len);
                let sealed = ok(
                    &router,
                    "Encrypt",
                    &json!({
                        "KeyId": identifier,
                        "Plaintext": b64(&bytes),
                        "EncryptionAlgorithm": algorithm,
                    }),
                )
                .await;
                assert_eq!(
                    sealed["KeyId"],
                    json!(arn),
                    "case {case}: Encrypt must answer the key's ARN, got {sealed}"
                );
                assert_eq!(
                    sealed["EncryptionAlgorithm"],
                    json!(algorithm),
                    "case {case}: Encrypt must report the algorithm it used, got {sealed}"
                );
                let ciphertext = unb64(&case, &sealed, "CiphertextBlob");
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

                // The ciphertext opens back, under the key the request named and the
                // algorithm it was sealed with.
                let opened = ok(
                    &router,
                    "Decrypt",
                    &json!({
                        "KeyId": arn,
                        "CiphertextBlob": text(&case, &sealed, "CiphertextBlob"),
                        "EncryptionAlgorithm": algorithm,
                    }),
                )
                .await;
                assert_eq!(
                    unb64(&case, &opened, "Plaintext"),
                    bytes,
                    "case {case}: Decrypt must answer exactly the plaintext"
                );
                assert_eq!(
                    opened["KeyId"],
                    json!(arn),
                    "case {case}: Decrypt must report the key that opened it, got {opened}"
                );
                // "This field is present only when the operation uses a symmetric
                // encryption KMS key" (`smithy_query(query="doc",shape="DecryptResponse")`).
                assert!(
                    opened["KeyMaterialId"].is_null(),
                    "case {case}: an asymmetric answer carries no KeyMaterialId, got {opened}"
                );
            }
        }
    }
}

/// The ciphertext is bound to the key and the algorithm: the same plaintext encrypted
/// twice gives two different ciphertexts, neither opens under the key's OTHER advertised
/// algorithm, and neither opens under another key of the same spec.
#[tokio::test]
async fn an_asymmetric_ciphertext_opens_only_under_the_key_and_algorithm_that_made_it() {
    let router = super::harness::router();
    let spec = &SPEC_CASES[0];
    let (_, arn) = encryption_key(&router, "under test", spec.key_spec).await;
    let (_, other_arn) = encryption_key(&router, "another key", spec.key_spec).await;
    let bytes = plaintext("binding", 48);

    let seal = |key: String, algorithm: &'static str| {
        let router = &router;
        let bytes = bytes.clone();
        async move {
            let sealed = ok(
                router,
                "Encrypt",
                &json!({
                    "KeyId": key,
                    "Plaintext": b64(&bytes),
                    "EncryptionAlgorithm": algorithm,
                }),
            )
            .await;
            text("binding", &sealed, "CiphertextBlob")
        }
    };

    let first = seal(arn.clone(), "RSAES_OAEP_SHA_256").await;
    let second = seal(arn.clone(), "RSAES_OAEP_SHA_256").await;
    assert_ne!(
        first, second,
        "case repeated: RSAES-OAEP is randomised, so one plaintext must give two \
         different ciphertexts"
    );

    // Rows: the case, the request that must be refused, and the code it must carry.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "declared under the key's other advertised algorithm",
            json!({"KeyId": &arn, "CiphertextBlob": &first,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_1"}),
            "InvalidCiphertextException",
        ),
        (
            "presented to another key of the same spec",
            json!({"KeyId": &other_arn, "CiphertextBlob": &first,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "InvalidCiphertextException",
        ),
        (
            // An asymmetric ciphertext is no envelope, so a request that names no key
            // says nothing about which key pair should open it.
            "presented with no KeyId at all",
            json!({"CiphertextBlob": &first, "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "InvalidCiphertextException",
        ),
    ];

    for (case, request, expected_code) in rows {
        let (status, code) = failure(&router, "Decrypt", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, expected_code,
            "case {case}: expected {expected_code}, got {code}"
        );
    }

    // The ciphertext itself is unharmed by the refusals above: it still opens.
    let opened = ok(
        &router,
        "Decrypt",
        &json!({"KeyId": &arn, "CiphertextBlob": &first,
                "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
    )
    .await;
    assert_eq!(
        unb64("binding", &opened, "Plaintext"),
        bytes,
        "case after the refusals: the ciphertext must still open to its plaintext"
    );
}

/// A plaintext past the ceiling the key spec and the algorithm give is refused, and
/// nothing is answered — while the ceiling itself is served.
///
/// The rows are every published ceiling of [`SPEC_CASES`], each tested AT the bound and
/// one byte past it.
#[tokio::test]
async fn a_plaintext_past_the_modulus_derived_ceiling_is_refused() {
    let router = super::harness::router();

    for spec in SPEC_CASES {
        let (_, arn) = encryption_key(&router, spec.key_spec, spec.key_spec).await;
        for (algorithm, ceiling) in spec.algorithms {
            let case = format!("{} {algorithm}", spec.key_spec);
            let at_the_bound = ok(
                &router,
                "Encrypt",
                &json!({
                    "KeyId": &arn,
                    "Plaintext": b64(&plaintext(&case, *ceiling)),
                    "EncryptionAlgorithm": algorithm,
                }),
            )
            .await;
            assert!(
                at_the_bound["CiphertextBlob"].is_string(),
                "case {case} at {ceiling} bytes: the ceiling itself must be served, got \
                 {at_the_bound}"
            );

            let (status, code, message) = refused(
                &router,
                REGION,
                "Encrypt",
                &json!({
                    "KeyId": &arn,
                    "Plaintext": b64(&plaintext(&case, ceiling + 1)),
                    "EncryptionAlgorithm": algorithm,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case} one past {ceiling}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "ValidationException",
                "case {case} one past {ceiling}: expected ValidationException, got {code}"
            );
            assert_eq!(
                message,
                format!(
                    "Algorithm {algorithm} and key spec {} cannot encrypt data larger than \
                     {ceiling} bytes.",
                    spec.key_spec
                ),
                "case {case} one past {ceiling}: expected the ceiling to be named, got \
                 {message:?}"
            );
        }
    }
}

/// The request's other members are judged against the key's spec: an algorithm the spec
/// does not advertise, an omitted algorithm (which means the symmetric default), and an
/// encryption context the scheme cannot bind are each refused — with the code and the
/// sentence the service words that refusal class with.
#[tokio::test]
async fn an_asymmetric_encrypt_is_refused_outside_what_its_spec_supports() {
    let router = super::harness::router();
    let (_, rsa) = encryption_key(&router, "rsa", "RSA_2048").await;
    let symmetric = create_key(&router, &json!({"Description": "symmetric"})).await;
    let symmetric_id = text("setup", &symmetric, "KeyId");
    let signing = create_key(
        &router,
        &json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let signing_id = text("setup", &signing, "KeyId");
    let signing_arn = text("setup", &signing, "Arn");
    let payload = b64(&plaintext("refusals", 16));

    // Rows: the case, the request, the code the refusal must carry, and its sentence —
    // which names the pair in conflict (the algorithm, and the key spec where that is the
    // other half) and nothing from the request's own values.
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "no EncryptionAlgorithm, so the symmetric default the spec does not advertise",
            json!({"KeyId": &rsa, "Plaintext": &payload}),
            "InvalidKeyUsageException",
            "Algorithm SYMMETRIC_DEFAULT is incompatible with key spec RSA_2048.".to_string(),
        ),
        (
            "the symmetric default named outright",
            json!({"KeyId": &rsa, "Plaintext": &payload,
                   "EncryptionAlgorithm": "SYMMETRIC_DEFAULT"}),
            "InvalidKeyUsageException",
            "Algorithm SYMMETRIC_DEFAULT is incompatible with key spec RSA_2048.".to_string(),
        ),
        (
            "an algorithm of another asymmetric family",
            json!({"KeyId": &rsa, "Plaintext": &payload, "EncryptionAlgorithm": "SM2PKE"}),
            "InvalidKeyUsageException",
            "Algorithm SM2PKE is incompatible with key spec RSA_2048.".to_string(),
        ),
        (
            // "An encryption context is valid only for cryptographic operations with a
            // symmetric encryption KMS key"
            // (`smithy_query(query="doc",shape="EncryptRequest",member="EncryptionContext")`).
            "an encryption context the scheme cannot bind",
            json!({"KeyId": &rsa, "Plaintext": &payload,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256",
                   "EncryptionContext": {"tenant": "acme"}}),
            "InvalidKeyUsageException",
            "Algorithm RSAES_OAEP_SHA_256 does not support an encryption context.".to_string(),
        ),
        (
            "an OAEP algorithm against a symmetric encryption key",
            json!({"KeyId": &symmetric_id, "Plaintext": &payload,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "InvalidKeyUsageException",
            "Algorithm RSAES_OAEP_SHA_256 is incompatible with key spec SYMMETRIC_DEFAULT."
                .to_string(),
        ),
        (
            // A key whose usage is signing advertises no encryption algorithm at all, so
            // the refusal is about the KEY and names it, as every other key-usage
            // refusal in this service does.
            "an RSA key whose usage is signing",
            json!({"KeyId": &signing_id, "Plaintext": &payload,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "InvalidKeyUsageException",
            format!("{signing_arn} key usage is SIGN_VERIFY which is not valid for Encrypt."),
        ),
    ];

    for (case, request, expected_code, expected_message) in rows {
        let (status, payload) = call(&router, REGION, "Encrypt", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            expected_code,
            "case {case}: expected {expected_code}, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            expected_message,
            "case {case}: expected {expected_message:?}, got {payload}"
        );
        assert!(
            payload["CiphertextBlob"].is_null(),
            "case {case}: a refused Encrypt answers no ciphertext, got {payload}"
        );
    }

    // An empty encryption context is the same as none at all, so it is not what makes a
    // request refusable: the asymmetric call carrying one is served.
    let sealed = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": &rsa, "Plaintext": &payload,
                "EncryptionAlgorithm": "RSAES_OAEP_SHA_256",
                "EncryptionContext": {}}),
    )
    .await;
    assert!(
        sealed["CiphertextBlob"].is_string(),
        "case an empty encryption context: expected a ciphertext, got {sealed}"
    );
}

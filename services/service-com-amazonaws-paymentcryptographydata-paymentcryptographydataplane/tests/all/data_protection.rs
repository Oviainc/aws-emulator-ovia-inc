// SPDX-License-Identifier: CC0-1.0

//! The symmetric data-protection loop of the Payment Cryptography data plane, end to
//! end over the wire: `EncryptData`, `DecryptData`, `GenerateMac`, `VerifyMac`,
//! `GenerateCardValidationData`, `VerifyCardValidationData`.
//!
//! Every call goes through the real `MultiServiceRouter`, and the keys these cases use
//! are minted by the CONTROL plane's own `CreateKey` / `CreateAlias` on the SAME
//! engine — so what is asserted here is what a signed SDK request observes against a
//! running emulator, over the key material that service stored.
//!
//! The rule under test: each of these operations answers from the ONE persisted key
//! the identifier resolves to — the `KeyArn` is that key's canonical ARN whatever
//! identifier the request named, the `KeyCheckValue` is the one the control plane
//! computed from that key's own secret, and the ciphertext, MAC and card
//! verification value are the published computation over that key's material, so a
//! decrypt inverts an encrypt and a verify recomputes a generate. Each case is one
//! row of a table spanning the class the rule generalises over — every modelled
//! `EncryptionMode`, every symmetric `KeyAlgorithm`, every `MacAlgorithm` with a
//! computation, both published bounds of `PrimaryAccountNumber` and every
//! `ValidationDataLength` — rather than the single row the issue named.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_paymentcryptographydata_paymentcryptographydataplane::ops::types::{
    ENCRYPTION_MODE_VALUES, MAC_ALGORITHM_VALUES, VERIFICATION_FAILED_REASON_VALUES,
};
use service_com_amazonaws_paymentcryptographydata_paymentcryptographydataplane::EmulatedService as DataPlane;
use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::EmulatedService as ControlPlane;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The `X-Amz-Target` prefix of the control plane's `awsJson1_0` surface, from that
/// crate's OPERATIONS.md wire-protocol section.
const CONTROL_TARGET_PREFIX: &str = "PaymentCryptographyControlPlane";

/// One emulator: the production router with BOTH Payment Cryptography services
/// mounted on one shared engine, exactly as `build_router` mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator core's business, not this service's.
    router.set_request_authentication(false);
    router.register_service(ControlPlane::new(Arc::clone(&engine)));
    router.register_service(DataPlane::new(Arc::clone(&engine)));
    router
}

/// The SigV4 credential scope both services are routed by.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/payment-cryptography/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Call one CONTROL plane operation (`awsJson1_0`, `POST /` with `X-Amz-Target`).
async fn control(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("authorization", authorization())
        .header("host", format!("controlplane.payment-cryptography.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{CONTROL_TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    dispatch(router, request, operation).await
}

/// Call one DATA plane operation (`restJson1`, `POST` to the operation's own URI).
async fn data(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", authorization())
        .header("host", format!("dataplane.payment-cryptography.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    dispatch(router, request, uri).await
}

/// Dispatch and read back the status and the JSON body.
async fn dispatch(
    router: &MultiServiceRouter,
    request: http::Request<Bytes>,
    what: &str,
) -> (u16, Value) {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{what} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Succeed-or-explain for a data plane call.
async fn ok_data(router: &MultiServiceRouter, uri: &str, body: &Value) -> Value {
    let (status, parsed) = data(router, uri, body).await;
    assert_eq!(
        status, 200,
        "case {uri} {body}: expected 200, got {status} {parsed}"
    );
    parsed
}

/// `__type` as the error envelope carries it, with any `shapeId#` prefix dropped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A string member that must be present.
fn string<'a>(value: &'a Value, member: &str) -> &'a str {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string {member} in {value}"))
}

/// Percent-encode the reserved characters a key ARN carries, so it fits the single
/// URI label `EncryptData` and `DecryptData` bind it to.
fn encoded(identifier: &str) -> String {
    identifier.replace(':', "%3A").replace('/', "%2F")
}

/// One key this suite works with: the identifiers it answers to, and the check value
/// the control plane computed for it.
#[derive(Debug, Clone)]
struct Key {
    arn: String,
    key_check_value: String,
}

/// Mint a key through the control plane's own `CreateKey` and read its ARN and check
/// value back out of the response.
async fn create_key(
    router: &MultiServiceRouter,
    key_algorithm: &str,
    key_usage: &str,
    modes: Value,
) -> Key {
    let body = json!({
        "KeyAttributes": {
            "KeyUsage": key_usage,
            "KeyClass": "SYMMETRIC_KEY",
            "KeyAlgorithm": key_algorithm,
            "KeyModesOfUse": modes,
        },
        "Exportable": true,
    });
    let (status, parsed) = control(router, "CreateKey", &body).await;
    assert_eq!(
        status, 200,
        "CreateKey {key_algorithm}/{key_usage}: expected 200, got {status} {parsed}"
    );
    let key = parsed.get("Key").unwrap_or_else(|| {
        panic!("CreateKey {key_algorithm}/{key_usage} answered no Key: {parsed}")
    });
    Key {
        arn: string(key, "KeyArn").to_string(),
        key_check_value: string(key, "KeyCheckValue").to_string(),
    }
}

/// Modes of use for a data encryption key, and for a key that generates and verifies.
fn encrypt_decrypt() -> Value {
    json!({"Encrypt": true, "Decrypt": true})
}
fn generate_verify() -> Value {
    json!({"Generate": true, "Verify": true})
}

/// The `KeyUsage` values this suite creates keys under, from the control plane's
/// modelled `KeyUsage`: the symmetric data encryption key, the ISO 9797 and HMAC MAC
/// keys, and the card verification key.
const USAGE_DATA_ENCRYPTION: &str = "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY";
const USAGE_ISO_9797_3_MAC: &str = "TR31_M3_ISO_9797_3_MAC_KEY";
const USAGE_CMAC: &str = "TR31_M6_ISO_9797_5_CMAC_KEY";
const USAGE_HMAC: &str = "TR31_M7_HMAC_KEY";
const USAGE_CARD_VERIFICATION: &str = "TR31_C0_CARD_VERIFICATION_KEY";

/// Every symmetric `KeyAlgorithm` that names a block cipher, with the block length it
/// works in. Source: the control plane's `SymmetricKeyAlgorithm`, less the HMAC
/// algorithms, which name a hash rather than a cipher.
fn block_cipher_algorithms() -> Vec<(&'static str, usize)> {
    vec![
        ("TDES_2KEY", 8),
        ("TDES_3KEY", 8),
        ("AES_128", 16),
        ("AES_192", 16),
        ("AES_256", 16),
    ]
}

/// A plaintext of `bytes` bytes, as the uppercase hex the model's pattern admits.
fn hex_data(bytes: usize) -> String {
    (0..bytes)
        .map(|index| format!("{:02X}", (index * 7 + 3) % 256))
        .collect()
}

/// The `EncryptData` / `DecryptData` request body for a symmetric call.
fn symmetric_body(member: &str, value: &str, mode: &str, iv: Option<&str>) -> Value {
    let mut attributes = json!({"Mode": mode});
    if let (Some(iv), Value::Object(target)) = (iv, &mut attributes) {
        target.insert("InitializationVector".to_string(), json!(iv));
    }
    json!({member: value, "EncryptionAttributes": {"Symmetric": attributes}})
}

/// The `DecryptData` body, which names its attributes member differently.
fn decrypt_body(cipher_text: &str, mode: &str, iv: Option<&str>) -> Value {
    let mut attributes = json!({"Mode": mode});
    if let (Some(iv), Value::Object(target)) = (iv, &mut attributes) {
        target.insert("InitializationVector".to_string(), json!(iv));
    }
    json!({"CipherText": cipher_text, "DecryptionAttributes": {"Symmetric": attributes}})
}

/// `POST /keys/{KeyIdentifier}/encrypt` for `identifier`.
fn encrypt_uri(identifier: &str) -> String {
    format!("/keys/{}/encrypt", encoded(identifier))
}

/// `POST /keys/{KeyIdentifier}/decrypt` for `identifier`.
fn decrypt_uri(identifier: &str) -> String {
    format!("/keys/{}/decrypt", encoded(identifier))
}

#[tokio::test]
async fn every_modelled_mode_round_trips_under_every_block_cipher_key() {
    let router = emulator();
    // The table spans the whole modelled `EncryptionMode` catalog: a value the model
    // gains later fails this assertion rather than going untested.
    let modes: Vec<&str> = ENCRYPTION_MODE_VALUES.to_vec();
    assert_eq!(
        modes.iter().copied().collect::<BTreeSet<_>>(),
        ["CBC", "CFB", "CFB1", "CFB128", "CFB64", "CFB8", "ECB", "OFB"]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "the mode table must span the modelled EncryptionMode catalog, which is {modes:?}"
    );

    for (key_algorithm, block_len) in block_cipher_algorithms() {
        let key = create_key(
            &router,
            key_algorithm,
            USAGE_DATA_ENCRYPTION,
            encrypt_decrypt(),
        )
        .await;
        for mode in &modes {
            // One block and three blocks: the data length rule EncryptData documents
            // is a multiple of the cipher's block size either way.
            for blocks in [1usize, 3] {
                let plain_text = hex_data(block_len * blocks);
                let case = format!("{key_algorithm}/{mode}/{blocks} blocks");
                let (status, parsed) = data(
                    &router,
                    &encrypt_uri(&key.arn),
                    &symmetric_body("PlainText", &plain_text, mode, None),
                )
                .await;
                // A CFB segment wider than the cipher's own block names no mode for
                // that cipher, and is the one combination refused.
                if *mode == "CFB128" && block_len == 8 {
                    assert_eq!(
                        (status, error_code(&parsed).as_str()),
                        (400, "ValidationException"),
                        "case {case}: a 128-bit segment over a 64-bit block must be \
                         refused, got {status} {parsed}"
                    );
                    continue;
                }
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200, got {status} {parsed}"
                );
                let cipher_text = string(&parsed, "CipherText").to_string();
                assert_eq!(
                    string(&parsed, "KeyArn"),
                    key.arn,
                    "case {case}: the answer must carry the canonical key ARN"
                );
                assert_eq!(
                    string(&parsed, "KeyCheckValue"),
                    key.key_check_value,
                    "case {case}: the answer must carry the key's own check value"
                );
                assert!(
                    cipher_text.len() == plain_text.len()
                        && cipher_text.chars().all(|c| c.is_ascii_hexdigit()),
                    "case {case}: CipherText {cipher_text} must be {} hex characters",
                    plain_text.len()
                );
                assert_ne!(
                    cipher_text, plain_text,
                    "case {case}: the ciphertext must not be the plaintext echoed back"
                );

                let decrypted = ok_data(
                    &router,
                    &decrypt_uri(&key.arn),
                    &decrypt_body(&cipher_text, mode, None),
                )
                .await;
                assert_eq!(
                    string(&decrypted, "PlainText"),
                    plain_text,
                    "case {case}: decrypt must return the plaintext encrypt was given"
                );
                assert_eq!(
                    string(&decrypted, "KeyArn"),
                    key.arn,
                    "case {case}: decrypt must carry the canonical key ARN"
                );
                assert_eq!(
                    string(&decrypted, "KeyCheckValue"),
                    key.key_check_value,
                    "case {case}: decrypt must carry the key's own check value"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_initialization_vector_is_what_the_mode_starts_from() {
    let router = emulator();
    // Every chaining mode starts from the IV, and ECB starts from nothing, so an IV
    // changes the ciphertext of each of the former and of none of the latter.
    for (key_algorithm, block_len) in block_cipher_algorithms() {
        let key = create_key(
            &router,
            key_algorithm,
            USAGE_DATA_ENCRYPTION,
            encrypt_decrypt(),
        )
        .await;
        let plain_text = hex_data(block_len * 2);
        let iv = hex_data(block_len);
        for mode in ["CBC", "CFB", "CFB8", "OFB", "ECB"] {
            let case = format!("{key_algorithm}/{mode}");
            let zero_iv = ok_data(
                &router,
                &encrypt_uri(&key.arn),
                &symmetric_body("PlainText", &plain_text, mode, None),
            )
            .await;
            let with_iv = ok_data(
                &router,
                &encrypt_uri(&key.arn),
                &symmetric_body("PlainText", &plain_text, mode, Some(&iv)),
            )
            .await;
            let (zero, nonzero) = (
                string(&zero_iv, "CipherText").to_string(),
                string(&with_iv, "CipherText").to_string(),
            );
            if mode == "ECB" {
                assert_eq!(
                    zero, nonzero,
                    "case {case}: ECB chains nothing, so an IV cannot change its output"
                );
                continue;
            }
            assert_ne!(
                zero, nonzero,
                "case {case}: the IV must change what the mode starts from"
            );
            let round_tripped = ok_data(
                &router,
                &decrypt_uri(&key.arn),
                &decrypt_body(&nonzero, mode, Some(&iv)),
            )
            .await;
            assert_eq!(
                string(&round_tripped, "PlainText"),
                plain_text,
                "case {case}: decrypt under the same IV must return the plaintext"
            );
            let under_default = ok_data(
                &router,
                &decrypt_uri(&key.arn),
                &decrypt_body(&nonzero, mode, None),
            )
            .await;
            assert_ne!(
                string(&under_default, "PlainText"),
                plain_text,
                "case {case}: decrypt under the default IV must not return the plaintext"
            );
        }
    }
}

#[tokio::test]
async fn a_ciphertext_is_bound_to_the_key_and_the_mode_it_was_made_under() {
    let router = emulator();
    let first = create_key(&router, "AES_128", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    let second = create_key(&router, "AES_128", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    assert_ne!(
        first.arn, second.arn,
        "two creates must mint two keys, got {} twice",
        first.arn
    );
    let plain_text = hex_data(32);
    let encrypted = ok_data(
        &router,
        &encrypt_uri(&first.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    let cipher_text = string(&encrypted, "CipherText").to_string();

    // The same ciphertext under the OTHER key, and under another mode of the same
    // key: each is a different plaintext, and neither is the one that was encrypted.
    for (case, identifier, mode) in [
        ("another key", second.arn.as_str(), "CBC"),
        ("another mode", first.arn.as_str(), "ECB"),
    ] {
        let decrypted = ok_data(
            &router,
            &decrypt_uri(identifier),
            &decrypt_body(&cipher_text, mode, None),
        )
        .await;
        assert_ne!(
            string(&decrypted, "PlainText"),
            plain_text,
            "case {case}: must not answer the plaintext the ciphertext was made from"
        );
    }

    // And the same key, mode and plaintext answer the same ciphertext on a later
    // call: the key is persisted, not redrawn per request.
    let again = ok_data(
        &router,
        &encrypt_uri(&first.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    assert_eq!(
        string(&again, "CipherText"),
        cipher_text,
        "a second call under the same key must answer the same ciphertext"
    );
    assert_eq!(
        string(&again, "KeyCheckValue"),
        first.key_check_value,
        "a second call must answer the same key check value"
    );
}

#[tokio::test]
async fn an_alias_resolves_to_the_key_it_is_bound_to() {
    let router = emulator();
    let key = create_key(&router, "AES_256", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    let alias = "alias/data-plane-suite";
    let (status, parsed) = control(
        &router,
        "CreateAlias",
        &json!({"AliasName": alias, "KeyArn": key.arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateAlias: expected 200, got {status} {parsed}"
    );

    let plain_text = hex_data(16);
    // Both identifier forms the modelled `KeyArnOrKeyAliasType` admits: the key ARN,
    // and the bare alias name.
    let by_arn = ok_data(
        &router,
        &encrypt_uri(&key.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    let by_alias = ok_data(
        &router,
        &encrypt_uri(alias),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    assert_eq!(
        string(&by_alias, "KeyArn"),
        key.arn,
        "an alias must answer the canonical key ARN, never the alias itself"
    );
    assert_eq!(
        string(&by_alias, "CipherText"),
        string(&by_arn, "CipherText"),
        "an alias and its key ARN must reach the same key material"
    );
    assert_eq!(
        string(&by_alias, "KeyCheckValue"),
        key.key_check_value,
        "an alias must answer the key's own check value"
    );

    // And through the body-bound identifier of a MAC call, with a key of its own.
    let mac_key = create_key(&router, "TDES_2KEY", USAGE_CMAC, generate_verify()).await;
    let mac_alias = "alias/data-plane-mac";
    let (status, parsed) = control(
        &router,
        "CreateAlias",
        &json!({"AliasName": mac_alias, "KeyArn": mac_key.arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateAlias for the MAC key: expected 200, got {status} {parsed}"
    );
    let by_alias = ok_data(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": mac_alias,
            "MessageData": hex_data(8),
            "GenerationAttributes": {"Algorithm": "CMAC"},
        }),
    )
    .await;
    assert_eq!(
        string(&by_alias, "KeyArn"),
        mac_key.arn,
        "GenerateMac must answer the canonical key ARN for an alias"
    );
}

#[tokio::test]
async fn an_identifier_that_names_no_key_is_not_found() {
    let router = emulator();
    let key = create_key(&router, "AES_128", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    let (account, id) = {
        let tail = key.arn.rsplit_once(":key/").expect("a key ARN");
        let account = tail
            .0
            .rsplit_once(':')
            .expect("the account segment")
            .1
            .to_string();
        (account, tail.1.to_string())
    };
    // Every form the modelled `KeyArnOrKeyAliasType` admits, each naming no key: an
    // id nothing minted, this key's id in another region, an alias ARN and a bare
    // alias that were never created.
    let unknown = vec![
        (
            "an id nothing minted",
            format!("arn:aws:payment-cryptography:{REGION}:{account}:key/{}", "a".repeat(16)),
        ),
        (
            "this key in another region",
            format!("arn:aws:payment-cryptography:eu-west-1:{account}:key/{id}"),
        ),
        (
            "an alias ARN that was never created",
            format!("arn:aws:payment-cryptography:{REGION}:{account}:alias/absent"),
        ),
        ("a bare alias that was never created", "alias/absent".to_string()),
    ];
    for (case, identifier) in unknown {
        let (status, parsed) = data(
            &router,
            &encrypt_uri(&identifier),
            &symmetric_body("PlainText", &hex_data(16), "CBC", None),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case} ({identifier}): expected a 404 ResourceNotFoundException, \
             got {status} {parsed}"
        );
        assert_eq!(
            string(&parsed, "ResourceId"),
            identifier,
            "case {case}: the refusal must name the resource the request asked for"
        );
    }

    // The same refusal from each of the operations that take the identifier in the
    // body, so no operation invents a key of its own.
    let absent = "alias/absent";
    let body_bound = vec![
        (
            "/mac/generate",
            json!({
                "KeyIdentifier": absent,
                "MessageData": hex_data(8),
                "GenerationAttributes": {"Algorithm": "CMAC"},
            }),
        ),
        (
            "/mac/verify",
            json!({
                "KeyIdentifier": absent,
                "MessageData": hex_data(8),
                "Mac": "0011223344556677",
                "VerificationAttributes": {"Algorithm": "CMAC"},
            }),
        ),
        (
            "/cardvalidationdata/generate",
            json!({
                "KeyIdentifier": absent,
                "PrimaryAccountNumber": "4111111111111111",
                "GenerationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
            }),
        ),
        (
            "/cardvalidationdata/verify",
            json!({
                "KeyIdentifier": absent,
                "PrimaryAccountNumber": "4111111111111111",
                "VerificationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
                "ValidationData": "123",
            }),
        ),
    ];
    for (uri, body) in body_bound {
        let (status, parsed) = data(&router, uri, &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (404, "ResourceNotFoundException"),
            "case {uri}: expected a 404 ResourceNotFoundException, got {status} {parsed}"
        );
    }
}

/// One row of the MAC table: the MAC algorithm, the key algorithm that computes it,
/// and the key usage such a key is created under.
struct MacRow {
    algorithm: &'static str,
    key_algorithm: &'static str,
    key_usage: &'static str,
}

/// Every modelled `MacAlgorithm` this emulator computes, each paired with a key
/// algorithm that can compute it. Source: `MAC_ALGORITHM_VALUES`, which the case below
/// checks this table against, so an algorithm the model gains later is noticed.
fn mac_table() -> Vec<MacRow> {
    let mut rows = Vec::new();
    for key_algorithm in ["TDES_2KEY", "TDES_3KEY"] {
        for algorithm in ["ISO9797_ALGORITHM1", "ISO9797_ALGORITHM3", "CMAC"] {
            rows.push(MacRow {
                algorithm,
                key_algorithm,
                key_usage: USAGE_ISO_9797_3_MAC,
            });
        }
    }
    for key_algorithm in ["AES_128", "AES_192", "AES_256"] {
        rows.push(MacRow {
            algorithm: "CMAC",
            key_algorithm,
            key_usage: USAGE_CMAC,
        });
    }
    for key_algorithm in [
        "HMAC_SHA224",
        "HMAC_SHA256",
        "HMAC_SHA384",
        "HMAC_SHA512",
    ] {
        // The explicit algorithm, and `HMAC`, which names whichever hash the key
        // selects.
        for algorithm in [key_algorithm, "HMAC"] {
            rows.push(MacRow {
                algorithm,
                key_algorithm,
                key_usage: USAGE_HMAC,
            });
        }
    }
    rows
}

#[tokio::test]
async fn a_mac_verifies_only_against_what_generated_it() {
    let router = emulator();
    let message = hex_data(16);
    let tampered = {
        // One nibble of the message flipped: the same length, one bit of content
        // different.
        let mut flipped = message.clone();
        flipped.replace_range(0..1, if message.starts_with('0') { "1" } else { "0" });
        flipped
    };
    let computed: BTreeSet<&str> = mac_table().iter().map(|row| row.algorithm).collect();
    let uncomputed: Vec<&&str> = MAC_ALGORITHM_VALUES
        .iter()
        .filter(|algorithm| !computed.contains(**algorithm))
        .collect();
    assert_eq!(
        uncomputed,
        vec![&"AS2805_4_1"],
        "every modelled MacAlgorithm but AS2805_4_1 must have a row, missing {uncomputed:?}"
    );

    for row in mac_table() {
        let case = format!("{}/{}", row.key_algorithm, row.algorithm);
        let key = create_key(
            &router,
            row.key_algorithm,
            row.key_usage,
            generate_verify(),
        )
        .await;
        let generate = |message: String, mac_length: Option<i32>| {
            let mut body = json!({
                "KeyIdentifier": key.arn,
                "MessageData": message,
                "GenerationAttributes": {"Algorithm": row.algorithm},
            });
            if let (Some(length), Value::Object(target)) = (mac_length, &mut body) {
                target.insert("MacLength".to_string(), json!(length));
            }
            body
        };

        let generated = ok_data(&router, "/mac/generate", &generate(message.clone(), None)).await;
        let mac = string(&generated, "Mac").to_string();
        assert!(
            (4..=128).contains(&mac.len()) && mac.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case}: Mac {mac} must be 4..128 hex characters"
        );
        assert_eq!(
            string(&generated, "KeyArn"),
            key.arn,
            "case {case}: GenerateMac must answer the canonical key ARN"
        );
        assert_eq!(
            string(&generated, "KeyCheckValue"),
            key.key_check_value,
            "case {case}: GenerateMac must answer the key's own check value"
        );
        let again = ok_data(&router, "/mac/generate", &generate(message.clone(), None)).await;
        assert_eq!(
            string(&again, "Mac"),
            mac,
            "case {case}: the MAC must be a function of key, message and algorithm"
        );
        let other_message =
            ok_data(&router, "/mac/generate", &generate(tampered.clone(), None)).await;
        assert_ne!(
            string(&other_message, "Mac"),
            mac,
            "case {case}: a different message must not answer the same MAC"
        );

        let verify = |message: &str, mac: &str, mac_length: Option<i32>| {
            let mut body = json!({
                "KeyIdentifier": key.arn,
                "MessageData": message,
                "Mac": mac,
                "VerificationAttributes": {"Algorithm": row.algorithm},
            });
            if let (Some(length), Value::Object(target)) = (mac_length, &mut body) {
                target.insert("MacLength".to_string(), json!(length));
            }
            body
        };
        let verified = ok_data(&router, "/mac/verify", &verify(&message, &mac, None)).await;
        assert_eq!(
            string(&verified, "KeyArn"),
            key.arn,
            "case {case}: VerifyMac must answer the canonical key ARN"
        );
        assert_eq!(
            string(&verified, "KeyCheckValue"),
            key.key_check_value,
            "case {case}: VerifyMac must answer the key's own check value"
        );
        // A MAC that is lowercase hex is the same MAC.
        ok_data(
            &router,
            "/mac/verify",
            &verify(&message, &mac.to_lowercase(), None),
        )
        .await;

        // Each way a verification can fail, each the modelled
        // VerificationFailedException with the INVALID_MAC reason.
        let same_length_other_mac = {
            let mut other = mac.clone();
            other.replace_range(0..1, if mac.starts_with('0') { "1" } else { "0" });
            other
        };
        for (why, message_data, presented) in [
            ("a tampered message", tampered.as_str(), mac.as_str()),
            ("a tampered MAC", message.as_str(), same_length_other_mac.as_str()),
        ] {
            let (status, parsed) = data(
                &router,
                "/mac/verify",
                &verify(message_data, presented, None),
            )
            .await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "VerificationFailedException"),
                "case {case} with {why}: expected a 400 VerificationFailedException, \
                 got {status} {parsed}"
            );
            assert_eq!(
                string(&parsed, "Reason"),
                "INVALID_MAC",
                "case {case} with {why}: the refusal must name the reason"
            );
            assert!(
                VERIFICATION_FAILED_REASON_VALUES.contains(&string(&parsed, "Reason")),
                "case {case} with {why}: the reason must be a modelled one"
            );
            assert!(
                !string(&parsed, "Message").is_empty(),
                "case {case} with {why}: the refusal must carry a message"
            );
        }

        // `MacLength` cuts the MAC it asks for, and the cut MAC verifies under the
        // same length and under no other.
        let full_length = i32::try_from(mac.len()).expect("a MAC length fits an i32");
        for mac_length in [4, 8, full_length.min(32)] {
            let truncated =
                ok_data(&router, "/mac/generate", &generate(message.clone(), Some(mac_length))).await;
            let short = string(&truncated, "Mac").to_string();
            assert_eq!(
                i32::try_from(short.len()).unwrap_or_default(),
                mac_length,
                "case {case}: MacLength {mac_length} must cut the MAC to that many \
                 hexadecimal characters, got {short}"
            );
            assert!(
                mac.starts_with(&short),
                "case {case}: the cut MAC must be the leading characters of {mac}, got {short}"
            );
            ok_data(
                &router,
                "/mac/verify",
                &verify(&message, &short, Some(mac_length)),
            )
            .await;
        }
        // A length longer than the MAC the algorithm computes names no MAC.
        if full_length < 32 {
            let (status, parsed) = data(
                &router,
                "/mac/generate",
                &generate(message.clone(), Some(32)),
            )
            .await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ValidationException"),
                "case {case}: a MacLength longer than the MAC must be refused, \
                 got {status} {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn a_mac_algorithm_the_key_cannot_compute_is_refused() {
    let router = emulator();
    // The algorithm has to be one the KEY can compute: an HMAC needs the HMAC key
    // whose own algorithm selects that hash, and the Triple DES CBC-MACs need a
    // Triple DES key.
    let rows = vec![
        ("AES_128", USAGE_CMAC, "HMAC_SHA256"),
        ("AES_128", USAGE_CMAC, "ISO9797_ALGORITHM1"),
        ("AES_128", USAGE_CMAC, "ISO9797_ALGORITHM3"),
        ("HMAC_SHA256", USAGE_HMAC, "HMAC_SHA512"),
        ("HMAC_SHA256", USAGE_HMAC, "CMAC"),
        ("TDES_2KEY", USAGE_ISO_9797_3_MAC, "HMAC"),
    ];
    for (key_algorithm, key_usage, algorithm) in rows {
        let key = create_key(&router, key_algorithm, key_usage, generate_verify()).await;
        let (status, parsed) = data(
            &router,
            "/mac/generate",
            &json!({
                "KeyIdentifier": key.arn,
                "MessageData": hex_data(8),
                "GenerationAttributes": {"Algorithm": algorithm},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {key_algorithm}/{algorithm}: expected a 400 ValidationException, \
             got {status} {parsed}"
        );
    }
}

#[tokio::test]
async fn a_key_whose_modes_of_use_exclude_the_operation_is_refused() {
    let router = emulator();
    // Each operation's own documentation names the mode of use it needs: Encrypt,
    // Decrypt, Generate, Verify. A key created without it cannot perform it.
    let encrypt_only = create_key(
        &router,
        "AES_128",
        USAGE_DATA_ENCRYPTION,
        json!({"Encrypt": true}),
    )
    .await;
    let plain_text = hex_data(16);
    let encrypted = ok_data(
        &router,
        &encrypt_uri(&encrypt_only.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    let (status, parsed) = data(
        &router,
        &decrypt_uri(&encrypt_only.arn),
        &decrypt_body(string(&encrypted, "CipherText"), "CBC", None),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "a key without Decrypt must not decrypt, got {status} {parsed}"
    );

    let generate_only = create_key(
        &router,
        "TDES_2KEY",
        USAGE_ISO_9797_3_MAC,
        json!({"Generate": true}),
    )
    .await;
    let generated = ok_data(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": generate_only.arn,
            "MessageData": plain_text,
            "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM3"},
        }),
    )
    .await;
    let (status, parsed) = data(
        &router,
        "/mac/verify",
        &json!({
            "KeyIdentifier": generate_only.arn,
            "MessageData": plain_text,
            "Mac": string(&generated, "Mac"),
            "VerificationAttributes": {"Algorithm": "ISO9797_ALGORITHM3"},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "a key without Verify must not verify, got {status} {parsed}"
    );

    // And a key with NoRestrictions carries every mode.
    let unrestricted = create_key(
        &router,
        "AES_128",
        USAGE_DATA_ENCRYPTION,
        json!({"NoRestrictions": true}),
    )
    .await;
    let encrypted = ok_data(
        &router,
        &encrypt_uri(&unrestricted.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    let decrypted = ok_data(
        &router,
        &decrypt_uri(&unrestricted.arn),
        &decrypt_body(string(&encrypted, "CipherText"), "CBC", None),
    )
    .await;
    assert_eq!(
        string(&decrypted, "PlainText"),
        plain_text,
        "a key with NoRestrictions must encrypt and decrypt"
    );
}

#[tokio::test]
async fn card_validation_data_verifies_only_against_the_card_it_was_generated_for() {
    let router = emulator();
    // The published bounds of `PrimaryAccountNumber` (12..19 digits) and one length
    // between them, under each Triple DES card verification key algorithm, for both
    // attribute sets the computation is defined for, at every modelled
    // `ValidationDataLength` and at the default.
    for key_algorithm in ["TDES_2KEY", "TDES_3KEY"] {
        let key = create_key(
            &router,
            key_algorithm,
            USAGE_CARD_VERIFICATION,
            generate_verify(),
        )
        .await;
        for pan in ["411111111111", "4111111111111111", "4111111111111111111"] {
            for attributes in [
                json!({"CardVerificationValue1": {"CardExpiryDate": "0123", "ServiceCode": "101"}}),
                json!({"CardVerificationValue2": {"CardExpiryDate": "0123"}}),
            ] {
                for length in [None, Some(3), Some(4), Some(5)] {
                    let case =
                        format!("{key_algorithm}/{}/{attributes}/{length:?}", pan.len());
                    let mut body = json!({
                        "KeyIdentifier": key.arn,
                        "PrimaryAccountNumber": pan,
                        "GenerationAttributes": attributes.clone(),
                    });
                    if let (Some(length), Value::Object(target)) = (length, &mut body) {
                        target.insert("ValidationDataLength".to_string(), json!(length));
                    }
                    let generated =
                        ok_data(&router, "/cardvalidationdata/generate", &body).await;
                    let validation_data = string(&generated, "ValidationData").to_string();
                    // "The default value is 3" when the request names no length.
                    let expected_length = length.unwrap_or(3);
                    assert_eq!(
                        i32::try_from(validation_data.len()).unwrap_or_default(),
                        expected_length,
                        "case {case}: ValidationData {validation_data} must be \
                         {expected_length} digits"
                    );
                    assert!(
                        validation_data.bytes().all(|b| b.is_ascii_digit()),
                        "case {case}: ValidationData {validation_data} must be digits"
                    );
                    assert_eq!(
                        string(&generated, "KeyArn"),
                        key.arn,
                        "case {case}: the answer must carry the canonical key ARN"
                    );
                    assert_eq!(
                        string(&generated, "KeyCheckValue"),
                        key.key_check_value,
                        "case {case}: the answer must carry the key's own check value"
                    );

                    let verify = |pan: &str, value: &str, attributes: &Value| {
                        json!({
                            "KeyIdentifier": key.arn,
                            "PrimaryAccountNumber": pan,
                            "VerificationAttributes": attributes.clone(),
                            "ValidationData": value,
                        })
                    };
                    let verified = ok_data(
                        &router,
                        "/cardvalidationdata/verify",
                        &verify(pan, &validation_data, &attributes),
                    )
                    .await;
                    assert_eq!(
                        string(&verified, "KeyCheckValue"),
                        key.key_check_value,
                        "case {case}: verify must carry the key's own check value"
                    );

                    // Each way the verification fails: another value of the same
                    // length, another account number, another expiry date. The two
                    // that change the CARD DATA are judged against what the key
                    // computes for that data, so a legitimate collision on a short
                    // value is a pass rather than a flake.
                    let other_value: String = validation_data
                        .chars()
                        .map(|digit| if digit == '0' { '1' } else { '0' })
                        .collect();
                    let other_pan = format!("5{}", &pan[1..]);
                    let other_attributes = if attributes.get("CardVerificationValue1").is_some() {
                        json!({"CardVerificationValue1": {"CardExpiryDate": "0124", "ServiceCode": "101"}})
                    } else {
                        json!({"CardVerificationValue2": {"CardExpiryDate": "0124"}})
                    };
                    let for_other_pan =
                        generated_validation_data(&router, &key.arn, &other_pan, &attributes, length)
                            .await;
                    let for_other_expiry = generated_validation_data(
                        &router,
                        &key.arn,
                        pan,
                        &other_attributes,
                        length,
                    )
                    .await;
                    for (why, body, presented, computed) in [
                        (
                            "another value",
                            verify(pan, &other_value, &attributes),
                            other_value.as_str(),
                            validation_data.as_str(),
                        ),
                        (
                            "another account number",
                            verify(&other_pan, &validation_data, &attributes),
                            validation_data.as_str(),
                            for_other_pan.as_str(),
                        ),
                        (
                            "another expiry date",
                            verify(pan, &validation_data, &other_attributes),
                            validation_data.as_str(),
                            for_other_expiry.as_str(),
                        ),
                    ] {
                        let (status, parsed) =
                            data(&router, "/cardvalidationdata/verify", &body).await;
                        if presented == computed {
                            assert_eq!(
                                status, 200,
                                "case {case} with {why}: the key computes {computed} for \
                                 this card data too, so it must verify, got {status} {parsed}"
                            );
                            continue;
                        }
                        assert_eq!(
                            (status, error_code(&parsed).as_str()),
                            (400, "VerificationFailedException"),
                            "case {case} with {why}: expected a 400 \
                             VerificationFailedException for {presented} against \
                             {computed}, got {status} {parsed}"
                        );
                        assert_eq!(
                            string(&parsed, "Reason"),
                            "INVALID_VALIDATION_DATA",
                            "case {case} with {why}: the refusal must name the reason"
                        );
                    }
                }
            }
        }
    }
}

/// What the key computes for this card data, read back through
/// `GenerateCardValidationData`.
///
/// A case that changes the card data compares against THIS rather than assuming two
/// different cards never produce the same value: a three-digit value collides about
/// once in a thousand, and the rule under test is that verification follows the
/// computation.
async fn generated_validation_data(
    router: &MultiServiceRouter,
    key_arn: &str,
    pan: &str,
    attributes: &Value,
    length: Option<i32>,
) -> String {
    let mut body = json!({
        "KeyIdentifier": key_arn,
        "PrimaryAccountNumber": pan,
        "GenerationAttributes": attributes.clone(),
    });
    if let (Some(length), Value::Object(target)) = (length, &mut body) {
        target.insert("ValidationDataLength".to_string(), json!(length));
    }
    let generated = ok_data(router, "/cardvalidationdata/generate", &body).await;
    string(&generated, "ValidationData").to_string()
}

#[tokio::test]
async fn card_data_operations_need_a_triple_des_card_verification_key() {
    let router = emulator();
    // "a CVK (Card Verification Key) encryption key is required"
    // (`GenerateCardValidationData`): a key of another usage, and a card
    // verification key of an algorithm the computation is not defined for, are each
    // refused rather than run under the wrong key.
    let rows = vec![
        ("another key usage", "TDES_2KEY", USAGE_DATA_ENCRYPTION),
        ("an AES card verification key", "AES_128", USAGE_CARD_VERIFICATION),
    ];
    for (case, key_algorithm, key_usage) in rows {
        let key = create_key(
            &router,
            key_algorithm,
            key_usage,
            json!({"Encrypt": true, "Decrypt": true, "Generate": true, "Verify": true}),
        )
        .await;
        let (status, parsed) = data(
            &router,
            "/cardvalidationdata/generate",
            &json!({
                "KeyIdentifier": key.arn,
                "PrimaryAccountNumber": "4111111111111111",
                "GenerationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case}: expected a 400 ValidationException, got {status} {parsed}"
        );
    }
}

#[tokio::test]
async fn one_key_reports_one_check_value_to_every_operation() {
    let router = emulator();
    // The key check value is a property of the key, so every operation that can use a
    // key reports the value the control plane's `GetKey` reports for it.
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_CARD_VERIFICATION,
        json!({"Encrypt": true, "Decrypt": true, "Generate": true, "Verify": true}),
    )
    .await;
    let (status, parsed) = control(&router, "GetKey", &json!({"KeyIdentifier": key.arn})).await;
    assert_eq!(status, 200, "GetKey: expected 200, got {status} {parsed}");
    let reported = string(
        parsed.get("Key").expect("GetKey answers a Key"),
        "KeyCheckValue",
    )
    .to_string();
    assert_eq!(
        reported, key.key_check_value,
        "GetKey must report the check value CreateKey answered"
    );

    let message = hex_data(16);
    let encrypted = ok_data(
        &router,
        &encrypt_uri(&key.arn),
        &symmetric_body("PlainText", &message, "CBC", None),
    )
    .await;
    let decrypted = ok_data(
        &router,
        &decrypt_uri(&key.arn),
        &decrypt_body(string(&encrypted, "CipherText"), "CBC", None),
    )
    .await;
    let mac = ok_data(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": key.arn,
            "MessageData": message,
            "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
        }),
    )
    .await;
    let verified_mac = ok_data(
        &router,
        "/mac/verify",
        &json!({
            "KeyIdentifier": key.arn,
            "MessageData": message,
            "Mac": string(&mac, "Mac"),
            "VerificationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
        }),
    )
    .await;
    let card = ok_data(
        &router,
        "/cardvalidationdata/generate",
        &json!({
            "KeyIdentifier": key.arn,
            "PrimaryAccountNumber": "4111111111111111",
            "GenerationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
        }),
    )
    .await;
    let verified_card = ok_data(
        &router,
        "/cardvalidationdata/verify",
        &json!({
            "KeyIdentifier": key.arn,
            "PrimaryAccountNumber": "4111111111111111",
            "VerificationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
            "ValidationData": string(&card, "ValidationData"),
        }),
    )
    .await;
    for (operation, answered) in [
        ("EncryptData", &encrypted),
        ("DecryptData", &decrypted),
        ("GenerateMac", &mac),
        ("VerifyMac", &verified_mac),
        ("GenerateCardValidationData", &card),
        ("VerifyCardValidationData", &verified_card),
    ] {
        assert_eq!(
            string(answered, "KeyCheckValue"),
            reported,
            "case {operation}: must report the key's one check value"
        );
        assert_eq!(
            string(answered, "KeyArn"),
            key.arn,
            "case {operation}: must report the key's canonical ARN"
        );
    }
}

#[tokio::test]
async fn a_key_the_control_plane_has_scheduled_for_deletion_cannot_be_used() {
    let router = emulator();
    // The key the data plane reads is the LIVE control-plane record, not a copy: a key
    // scheduled for deletion there cannot be used here, and restoring it there makes
    // it usable again.
    let key = create_key(&router, "AES_128", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    let plain_text = hex_data(16);
    let encrypted = ok_data(
        &router,
        &encrypt_uri(&key.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    let cipher_text = string(&encrypted, "CipherText").to_string();

    let (status, parsed) = control(
        &router,
        "DeleteKey",
        &json!({"KeyIdentifier": key.arn, "DeleteKeyInDays": 3}),
    )
    .await;
    assert_eq!(status, 200, "DeleteKey: expected 200, got {status} {parsed}");
    let (status, parsed) = data(
        &router,
        &encrypt_uri(&key.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "a key waiting to be deleted must not encrypt, got {status} {parsed}"
    );

    let (status, parsed) = control(&router, "RestoreKey", &json!({"KeyIdentifier": key.arn})).await;
    assert_eq!(status, 200, "RestoreKey: expected 200, got {status} {parsed}");
    let decrypted = ok_data(
        &router,
        &decrypt_uri(&key.arn),
        &decrypt_body(&cipher_text, "CBC", None),
    )
    .await;
    assert_eq!(
        string(&decrypted, "PlainText"),
        plain_text,
        "a restored key must run the same computation it ran before"
    );
}

#[tokio::test]
async fn an_attribute_set_with_no_computation_here_still_says_so() {
    let router = emulator();
    // The attribute sets this slice does not compute answer 501 — "nobody has written
    // it yet" — rather than a value no documented algorithm produced. Each is the
    // record of what the next slice of this service has to implement.
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_ISO_9797_3_MAC,
        json!({"Generate": true, "Verify": true, "DeriveKey": true}),
    )
    .await;
    let (status, parsed) = data(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": key.arn,
            "MessageData": hex_data(8),
            "GenerationAttributes": {"Algorithm": "AS2805_4_1"},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (501, "NotImplemented"),
        "an algorithm with no computation here must say so, got {status} {parsed}"
    );

    let (status, parsed) = data(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": key.arn,
            "MessageData": hex_data(8),
            "GenerationAttributes": {
                "DukptCmac": {"KeySerialNumber": "FFFF9876543210E00008", "DukptKeyVariant": "BIDIRECTIONAL"},
            },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (501, "NotImplemented"),
        "a DUKPT derivation must say so, got {status} {parsed}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! What `GetParametersForImport` hands a caller that is about to import key material,
//! driven end-to-end over the wire through the shared [`super::harness`].
//!
//! The rule under test: for a KMS key whose key material comes from outside KMS, the
//! operation REPORTS a freshly generated RSA wrapping key pair — the public half of it,
//! as X.509 `SubjectPublicKeyInfo` DER, of the size the requested `WrappingKeySpec`
//! names — together with the import token that names that one pair and the instant,
//! 24 hours ahead, at which both stop being usable; it names the key by its ARN, it
//! mints one independent pair per call, it leaves the key exactly as it was, and an
//! identifier it cannot resolve to such a key in a compatible key state is refused with
//! no pair minted at all.
//!
//! Each test asserts that rule over a TABLE: the rows differ in the wrapping key spec
//! and wrapping algorithm asked for, in the kind of key the material is destined for,
//! and in HOW an identifier fails to name an importable key. The round trip is the
//! proof that the reported public key is the public half of a pair this service really
//! kept: key material wrapped under it is material `ImportKeyMaterial` opens, and only
//! the token issued WITH that pair opens it.
//!
//! [`super::error_semantics`] holds the neighbouring cases for the same operation — the
//! origin gate that refuses a key whose key material KMS generated, the wrap-and-import
//! round trip, and the imports a token cannot serve. What is only here: the modulus size
//! each wrapping key spec names, the independence of one call's pair from another's, the
//! pairs the store is left holding, and a reissue for a key whose material was deleted.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_kms_trentservice::state::service_state;

use super::harness::{
    alias_arn, b64, create_alias, create_key, describe, oaep_wrapping_key, ok, refused,
    router_with_engine, text, unb64, wrap, REGION,
};

/// The Region a row names when the row is about an ARN pinned to another endpoint.
const OTHER_REGION: &str = "ap-south-1";

/// How many wrapping key pairs the service is holding — what a call that mints one adds
/// to, and what a refused call must leave alone.
fn held_pairs(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let state = state.lock();
    state.import_parameters.len()
}

/// Key material of `len` bytes that depends only on `seed`, so the same row produces
/// the same material twice and two rows produce different material.
fn material(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|index| (index as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// A request for import parameters for `key_id`.
fn request(key_id: &str, wrapping_algorithm: &str, wrapping_key_spec: &str) -> Value {
    json!({
        "KeyId": key_id,
        "WrappingAlgorithm": wrapping_algorithm,
        "WrappingKeySpec": wrapping_key_spec,
    })
}

/// Every call mints its OWN wrapping key pair: the public key is an RSA key of the size
/// the `WrappingKeySpec` names, the token is new, and a token opens only the material
/// wrapped under the public key it was issued with.
#[tokio::test]
async fn each_call_mints_its_own_wrapping_key_pair_and_token() {
    let (router, engine) = router_with_engine();
    let metadata = create_key(
        &router,
        &json!({"Origin": "EXTERNAL", "KeySpec": "SYMMETRIC_DEFAULT"}),
    )
    .await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");

    // Rows: the `WrappingKeySpec` asked for, the RSA modulus in bytes that spec names,
    // and the wrapping algorithm the caller says it will use. The specs are the RSA
    // values of `smithy_query(query="shape",shape="WrappingKeySpec")` (RSA_2048,
    // RSA_3072, RSA_4096 — 2048, 3072 and 4096 bits); the algorithms are the RSAES-OAEP
    // values of `smithy_query(query="shape",shape="AlgorithmSpec")`, which wrap the key
    // material directly under the public key.
    let rows = [
        ("RSA_2048", 256usize, "RSAES_OAEP_SHA_256"),
        ("RSA_3072", 384, "RSAES_OAEP_SHA_1"),
        ("RSA_4096", 512, "RSAES_OAEP_SHA_256"),
    ];
    let held_before = held_pairs(&engine);

    // What each row was answered: the case, the public key, its token, and the algorithm
    // the pair was issued for.
    let mut issued: Vec<(String, Vec<u8>, String, &str)> = Vec::new();
    for (wrapping_key_spec, modulus_len, wrapping_algorithm) in rows {
        let case = format!("{wrapping_key_spec} with {wrapping_algorithm}");
        let answer = ok(
            &router,
            "GetParametersForImport",
            &request(&key_id, wrapping_algorithm, wrapping_key_spec),
        )
        .await;

        // "The Amazon Resource Name (key ARN) of the KMS key to use in a subsequent
        // ImportKeyMaterial request. This is the same KMS key specified in the
        // GetParametersForImport request."
        assert_eq!(
            answer["KeyId"].as_str().unwrap_or_default(),
            key_arn,
            "case {case:?}: the answer names the key by its ARN, got {answer}"
        );

        // "The public key (or 'wrapping key') of an RSA key pair that KMS generates":
        // it parses as an RSA public key and carries the modulus the spec names.
        let public_key = unb64(&case, &answer, "PublicKey");
        let size = oaep_wrapping_key(&case, &public_key).key_size_bytes();
        assert_eq!(
            size, modulus_len,
            "case {case:?}: expected a {modulus_len}-byte RSA modulus, got {size}"
        );

        // "The public key and its import token are permanently linked": a pair of its
        // own per call, so neither half repeats.
        let token = text(&case, &answer, "ImportToken");
        assert!(
            issued
                .iter()
                .all(|(_, held_key, held_token, _)| *held_key != public_key
                    && *held_token != token),
            "case {case:?}: expected a wrapping key pair and token of its own, got one \
             already issued"
        );

        // "Each public key and import token set is valid for 24 hours. The expiration
        // date and time appear in the ParametersValidTo field."
        let valid_to = answer["ParametersValidTo"].as_f64().unwrap_or_default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64();
        let twenty_four_hours = 24.0 * 60.0 * 60.0;
        assert!(
            (valid_to - (now + twenty_four_hours)).abs() < 60.0,
            "case {case:?}: expected ParametersValidTo 24 hours ahead of {now}, got \
             {valid_to}"
        );

        // The call reports parameters; the key still waits for its key material.
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["KeyState"].as_str().unwrap_or_default(),
            "PendingImport",
            "case {case:?}: the key still waits for its key material, got {described}"
        );

        issued.push((case, public_key, token, wrapping_algorithm));
    }
    let held = held_pairs(&engine);
    assert_eq!(
        held,
        held_before + rows.len(),
        "case every row: each answered call keeps the pair it handed out, expected {} \
         held, got {held}",
        held_before + rows.len()
    );

    // Each token names ONE pair: material wrapped under another call's public key does
    // not open under it, so the private half really is the one handed out with it.
    let bytes = material(32, 0x11);
    for (case, public_key, _token, wrapping_algorithm) in &issued {
        for (other_case, _other_key, other_token, _other_algorithm) in &issued {
            if other_case == case {
                continue;
            }
            let crossed = format!("{case} wrapped, {other_case} token");
            let wrapped = wrap(&crossed, public_key, wrapping_algorithm, &bytes);
            let (status, code, _message) = refused(
                &router,
                REGION,
                "ImportKeyMaterial",
                &json!({
                    "KeyId": key_id,
                    "ImportToken": other_token,
                    "EncryptedKeyMaterial": b64(&wrapped),
                    "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {crossed:?}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "InvalidCiphertextException",
                "case {crossed:?}: material wrapped under another pair must not open, got \
                 {code:?}"
            );
            assert_eq!(
                describe(&router, &key_id).await["KeyState"]
                    .as_str()
                    .unwrap_or_default(),
                "PendingImport",
                "case {crossed:?}: a refused import leaves the key waiting"
            );
        }
    }

    // And the pairing that holds: the token issued WITH the public key the material was
    // wrapped under imports it.
    let (case, public_key, token, wrapping_algorithm) = &issued[0];
    let wrapped = wrap(case, public_key, wrapping_algorithm, &bytes);
    let imported = ok(
        &router,
        "ImportKeyMaterial",
        &json!({
            "KeyId": key_id,
            "ImportToken": token,
            "EncryptedKeyMaterial": b64(&wrapped),
            "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
        }),
    )
    .await;
    assert_eq!(
        imported["KeyId"].as_str().unwrap_or_default(),
        key_arn,
        "case {case:?}: the import names the key by its ARN, got {imported}"
    );
    assert_eq!(
        describe(&router, &key_id).await["KeyState"]
            .as_str()
            .unwrap_or_default(),
        "Enabled",
        "case {case:?}: a key that received its key material is usable"
    );
}

/// An identifier that names no importable key in a compatible key state is refused, and
/// no wrapping key pair is minted for it.
#[tokio::test]
async fn an_identifier_that_names_no_importable_key_is_refused_and_mints_nothing() {
    let (router, engine) = router_with_engine();
    let account = engine::emulator_account_id();

    // A key created with no key material, reachable by an alias as well as by its own
    // identifiers, and a second one whose deletion is then scheduled.
    let importable = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let importable_id = text("setup", &importable, "KeyId");
    create_alias(&router, "setup", "alias/importable", &importable_id).await;
    let doomed = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let doomed_id = text("setup", &doomed, "KeyId");
    let doomed_arn = text("setup", &doomed, "Arn");
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": doomed_id, "PendingWindowInDays": 7}),
    )
    .await;
    let missing_id = "00000000-0000-0000-0000-000000000000";

    // Rows: the identifier, and the refusal it meets. They differ in HOW the identifier
    // fails to name an importable key: it names an alias rather than a key (in both
    // forms KMS accepts for one), it is an ARN that is no KMS key ARN, it is pinned to
    // another Region's endpoint, it names no key of this endpoint at all, or it names a
    // key whose key state refuses the call.
    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "an alias name",
            "alias/importable".to_string(),
            "InvalidArnException",
            "The specified ARN is not valid: alias/importable.".to_string(),
        ),
        (
            "an alias ARN",
            alias_arn(REGION, "alias/importable"),
            "InvalidArnException",
            format!(
                "The specified ARN is not valid: {}.",
                alias_arn(REGION, "alias/importable")
            ),
        ),
        (
            "an ARN that is no KMS key ARN",
            format!("arn:aws:kms:{REGION}:{account}:thing/abc"),
            "InvalidArnException",
            format!("The specified ARN is not valid: arn:aws:kms:{REGION}:{account}:thing/abc."),
        ),
        (
            // KMS answers a key ARN pinned to another Region by naming the ARN's OWN
            // Region: recorded on the wire as HTTP 400 `NotFoundException: Invalid arn
            // <that Region>` by `kms-key-description-update-and-cancel-deletion#task#5`
            // of the `simparity-wire-aws-v1` corpus, which is the recording the
            // service-wide message builder for a key lookup that found nothing is read
            // from.
            "a key ARN of another Region",
            format!("arn:aws:kms:{OTHER_REGION}:{account}:key/{importable_id}"),
            "NotFoundException",
            format!("Invalid arn {OTHER_REGION}"),
        ),
        (
            "a key id nothing holds",
            missing_id.to_string(),
            "NotFoundException",
            format!("Key 'arn:aws:kms:{REGION}:{account}:key/{missing_id}' does not exist"),
        ),
        (
            "a key ARN nothing holds",
            format!("arn:aws:kms:{REGION}:{account}:key/{missing_id}"),
            "NotFoundException",
            format!("Key 'arn:aws:kms:{REGION}:{account}:key/{missing_id}' does not exist"),
        ),
        (
            "a key awaiting deletion",
            doomed_id.clone(),
            "KMSInvalidStateException",
            format!("{doomed_arn} is pending deletion."),
        ),
    ];
    let held_before = held_pairs(&engine);
    for (case, identifier, expected_code, expected_message) in rows {
        let (status, code, message) = refused(
            &router,
            REGION,
            "GetParametersForImport",
            &request(&identifier, "RSAES_OAEP_SHA_256", "RSA_2048"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );
        let held = held_pairs(&engine);
        assert_eq!(
            held, held_before,
            "case {case:?}: a refused call mints no wrapping key pair, expected \
             {held_before} held, got {held}"
        );
    }

    // The key the alias rows named is untouched by them: it still waits for its key
    // material, and the same call made with its own key id is answered.
    let described = describe(&router, &importable_id).await;
    assert_eq!(
        described["KeyState"].as_str().unwrap_or_default(),
        "PendingImport",
        "case the importable key: the refused calls leave it waiting, got {described}"
    );
    let answer = ok(
        &router,
        "GetParametersForImport",
        &request(&importable_id, "RSAES_OAEP_SHA_256", "RSA_2048"),
    )
    .await;
    assert_eq!(
        answer["KeyId"].as_str().unwrap_or_default(),
        text("the importable key", &importable, "Arn"),
        "case the importable key: the answer names the key by its ARN, got {answer}"
    );
}

/// Parameters can be issued again for a key whose imported key material was deleted,
/// and the new pair is the one that import must use — "you can also use
/// GetParametersForImport to get a public key and import token to reimport the original
/// key material into a KMS key whose key material expired or was deleted".
#[tokio::test]
async fn parameters_are_issued_again_for_a_key_whose_material_was_deleted() {
    let (router, engine) = router_with_engine();

    // Rows: the key the material is destined for and the number of bytes that key's
    // material has, with the wrapping key spec and algorithm the import uses. They
    // differ in the key spec, in whether the key is multi-Region, and in the wrapping
    // the material is carried under. A SYMMETRIC_DEFAULT key is "a 256-bit AES-GCM key"
    // and an HMAC_384 key a 384-bit HMAC key
    // (`smithy_query(query="shape",shape="KeySpec")`).
    let rows: Vec<(&str, Value, usize, &str, &str, bool)> = vec![
        (
            "a symmetric encryption key",
            json!({"Origin": "EXTERNAL", "KeySpec": "SYMMETRIC_DEFAULT"}),
            32,
            "RSA_2048",
            "RSAES_OAEP_SHA_256",
            true,
        ),
        (
            "a multi-Region symmetric encryption key",
            json!({"Origin": "EXTERNAL", "MultiRegion": true}),
            32,
            "RSA_3072",
            "RSAES_OAEP_SHA_1",
            true,
        ),
        (
            "an HMAC key",
            json!({"Origin": "EXTERNAL", "KeySpec": "HMAC_384",
                   "KeyUsage": "GENERATE_VERIFY_MAC"}),
            48,
            "RSA_2048",
            "RSAES_OAEP_SHA_1",
            false,
        ),
    ];
    for (
        index,
        (case, create, material_len, wrapping_key_spec, wrapping_algorithm, identifies_material),
    ) in rows.into_iter().enumerate()
    {
        let metadata = create_key(&router, &create).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");
        assert_eq!(
            metadata["KeyState"].as_str().unwrap_or_default(),
            "PendingImport",
            "case {case:?}: a key created with no key material waits for it, got {metadata}"
        );
        let bytes = material(material_len, index as u8);
        let held_before = held_pairs(&engine);

        // The first import, under the first pair.
        let first = ok(
            &router,
            "GetParametersForImport",
            &request(&key_id, wrapping_algorithm, wrapping_key_spec),
        )
        .await;
        let first_public_key = unb64(case, &first, "PublicKey");
        let imported = ok(
            &router,
            "ImportKeyMaterial",
            &json!({
                "KeyId": key_id,
                "ImportToken": text(case, &first, "ImportToken"),
                "EncryptedKeyMaterial": b64(&wrap(case, &first_public_key, wrapping_algorithm,
                                                 &bytes)),
                "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
            }),
        )
        .await;
        // A key material identifier "is present for symmetric encryption keys"
        // (`smithy_query(query="shape",shape="KeyMetadata")` -> `CurrentKeyMaterialId`),
        // so the HMAC row is answered without one.
        let key_material_id = imported["KeyMaterialId"].as_str().map(str::to_string);
        assert_eq!(
            key_material_id.is_some(),
            identifies_material,
            "case {case:?}: expected KeyMaterialId present: {identifies_material}, got \
             {imported}"
        );

        // Deleting the material leaves the key waiting for material again, which is the
        // state this operation exists for.
        ok(
            &router,
            "DeleteImportedKeyMaterial",
            &json!({"KeyId": key_id}),
        )
        .await;
        assert_eq!(
            describe(&router, &key_id).await["KeyState"]
                .as_str()
                .unwrap_or_default(),
            "PendingImport",
            "case {case:?}: a key whose material was deleted waits for material again"
        );

        // The second call answers a DIFFERENT pair for the same key, and the material
        // the key held before is reimported under it — the same bytes, so the same key
        // material identifier.
        let second = ok(
            &router,
            "GetParametersForImport",
            &request(&key_arn, wrapping_algorithm, wrapping_key_spec),
        )
        .await;
        assert_eq!(
            second["KeyId"].as_str().unwrap_or_default(),
            key_arn,
            "case {case:?}: the answer names the key by its ARN, got {second}"
        );
        let second_public_key = unb64(case, &second, "PublicKey");
        assert_ne!(
            second_public_key, first_public_key,
            "case {case:?}: a second call mints a wrapping key pair of its own"
        );
        let reimported = ok(
            &router,
            "ImportKeyMaterial",
            &json!({
                "KeyId": key_id,
                "ImportToken": text(case, &second, "ImportToken"),
                "EncryptedKeyMaterial": b64(&wrap(case, &second_public_key, wrapping_algorithm,
                                                  &bytes)),
                "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
            }),
        )
        .await;
        assert_eq!(
            reimported["KeyMaterialId"].as_str().map(str::to_string),
            key_material_id,
            "case {case:?}: the same key material carries the same identifier, got \
             {reimported}"
        );
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["KeyState"].as_str().unwrap_or_default(),
            "Enabled",
            "case {case:?}: the reimported key is usable again, got {described}"
        );
        assert_eq!(
            described["Origin"].as_str().unwrap_or_default(),
            "EXTERNAL",
            "case {case:?}: the key material still comes from outside KMS, got {described}"
        );

        // Both pairs were kept while they were handed out: the store holds one record
        // per answered call.
        let held = held_pairs(&engine);
        assert_eq!(
            held,
            held_before + 2,
            "case {case:?}: expected the row's two answered calls to leave {} held \
             wrapping key pairs, got {held}",
            held_before + 2
        );
    }
}

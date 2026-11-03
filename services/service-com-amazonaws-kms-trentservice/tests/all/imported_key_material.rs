// SPDX-License-Identifier: CC0-1.0

//! What identifies the key material a caller IMPORTS, and what the refusals on the
//! import and decrypt paths say, driven end-to-end over the wire through the shared
//! [`super::harness`].
//!
//! Three rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over:
//!
//! 1. Key material is identified by the material itself, so material a KMS key is
//!    already associated with is not NEW material: an import that states its material is
//!    `NEW_KEY_MATERIAL` is refused for material the key holds, holds pending, or still
//!    retains — and material that really is new waits in `PENDING_ROTATION` beside the
//!    version the key goes on encrypting under, until `RotateKeyOnDemand` makes it
//!    current. The rows differ in the wrapping algorithm and in which material is
//!    presented.
//! 2. The key material of a key of an ASYMMETRIC spec is the PRIVATE KEY of a key pair
//!    of that spec, as PKCS#8 DER: such a private key is imported and makes the key
//!    usable, while a private key of another spec, or bytes that are no private key at
//!    all, are refused. The rows differ in the key spec and in what the bytes are.
//! 3. Each refusal on these paths carries the wording AWS words that class with: an
//!    import token not bound to the key, and a ciphertext that does not open, carry NO
//!    message at all; a `KeyId` that did not seal the ciphertext carries one fixed
//!    sentence that names no key. The rows differ in how each refusal is provoked.

use serde_json::{json, Value};

use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
use aws_lc_rs::signature::{
    EcdsaKeyPair, Ed25519KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING, ECDSA_P384_SHA384_ASN1_SIGNING,
};
use service_com_amazonaws_kms_trentservice::state::{random_bytes, KeyMaterial, MaterialKind};
use sm2::pkcs8::EncodePrivateKey;

use super::harness::{
    b64, call, create_alias, create_key, describe, error_code, error_message, ok, refused,
    rotate_on_demand, router, text, unb64, wrap, REGION,
};

/// How many bytes of key material a symmetric encryption KMS key takes, which is what
/// `MaterialKind::Aes256` holds.
const SYMMETRIC_MATERIAL_LEN: usize = 32;

/// The `KeyMaterialId` KMS derives from symmetric key material: asked of the store that
/// derives it, so a row proves the key ended up holding exactly the bytes that were
/// wrapped rather than re-stating a literal.
fn symmetric_material_id(case: &str, material: &[u8]) -> String {
    KeyMaterial::from_bytes(MaterialKind::Aes256, material)
        .unwrap_or_else(|| panic!("case {case}: the bytes are symmetric key material"))
        .id()
        .unwrap_or_else(|| panic!("case {case}: symmetric key material carries an id"))
}

/// Ask for import parameters for `key_id` and report the wrapping public key and the
/// import token they are bound to.
async fn parameters_for(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_id: &str,
    wrapping_algorithm: &str,
) -> (Vec<u8>, String) {
    let parameters = ok(
        router,
        "GetParametersForImport",
        &json!({
            "KeyId": key_id,
            "WrappingAlgorithm": wrapping_algorithm,
            "WrappingKeySpec": "RSA_2048",
        }),
    )
    .await;
    (
        unb64(case, &parameters, "PublicKey"),
        text(case, &parameters, "ImportToken"),
    )
}

/// An `ImportKeyMaterial` request body for an already wrapped blob.
fn import_request(key_id: &str, import_token: &str, wrapped: &[u8], import_type: Option<&str>) -> Value {
    let mut body = json!({
        "KeyId": key_id,
        "ImportToken": import_token,
        "EncryptedKeyMaterial": b64(wrapped),
        "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
    });
    if let Some(import_type) = import_type {
        body["ImportType"] = json!(import_type);
    }
    body
}

/// The key materials `ListKeyRotations` reports for a key, including the first one and
/// anything waiting for a rotation.
async fn key_materials(
    router: &smithy_server_runtime::MultiServiceRouter,
    key_id: &str,
) -> Vec<Value> {
    let listed = ok(
        router,
        "ListKeyRotations",
        &json!({"KeyId": key_id, "IncludeKeyMaterial": "ALL_KEY_MATERIAL"}),
    )
    .await;
    listed["Rotations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `KeyMaterialId`s the listing reports in `state`.
fn in_state(materials: &[Value], state: &str) -> Vec<String> {
    materials
        .iter()
        .filter(|entry| entry["KeyMaterialState"].as_str() == Some(state))
        .map(|entry| {
            entry["KeyMaterialId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Rule 1: material the key is already associated with is not new material, and material
/// that is waits for the rotation it was imported for.
#[tokio::test]
async fn material_a_key_already_holds_is_not_new_key_material() {
    let router = router();

    // Rows: the wrapping algorithm the caller uses. They differ in BOTH halves of the
    // wrapping — which digest the RSA step uses, and whether the material travels under
    // the RSA key directly or under an ephemeral AES key — and nothing about the rule
    // depends on which was used.
    for wrapping_algorithm in [
        "RSAES_OAEP_SHA_256",
        "RSAES_OAEP_SHA_1",
        "RSA_AES_KEY_WRAP_SHA_256",
        "RSA_AES_KEY_WRAP_SHA_1",
    ] {
        let case = wrapping_algorithm;
        let metadata = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
        let key_id = text(case, &metadata, "KeyId");

        // The FIRST material: a key with none is not rotating, so this import makes the
        // key usable and the material current at once.
        let first = random_bytes(SYMMETRIC_MATERIAL_LEN).expect("entropy");
        let first_id = symmetric_material_id(case, &first);
        let (public_key, token) = parameters_for(&router, case, &key_id, wrapping_algorithm).await;
        let wrapped = wrap(case, &public_key, wrapping_algorithm, &first);
        let imported = ok(
            &router,
            "ImportKeyMaterial",
            &import_request(&key_id, &token, &wrapped, None),
        )
        .await;
        assert_eq!(
            imported["KeyMaterialId"].as_str().unwrap_or_default(),
            first_id,
            "case {case:?}: the key must hold exactly the material that was wrapped, got \
             {imported}"
        );
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["CurrentKeyMaterialId"].as_str().unwrap_or_default(),
            first_id,
            "case {case:?}: the first material is the current one, got {described}"
        );

        // A ciphertext sealed under that material, kept to prove a later rotation does
        // not lose it.
        let plaintext = b"imported material round trip";
        let sealed = ok(
            &router,
            "Encrypt",
            &json!({"KeyId": key_id, "Plaintext": b64(plaintext)}),
        )
        .await;
        let blob = text(case, &sealed, "CiphertextBlob");

        // NEW material for a key that already has material: the import succeeds ONCE.
        let next = random_bytes(SYMMETRIC_MATERIAL_LEN).expect("entropy");
        let next_id = symmetric_material_id(case, &next);
        let (public_key, token) = parameters_for(&router, case, &key_id, wrapping_algorithm).await;
        let wrapped = wrap(case, &public_key, wrapping_algorithm, &next);
        let request = import_request(&key_id, &token, &wrapped, Some("NEW_KEY_MATERIAL"));
        let imported = ok(&router, "ImportKeyMaterial", &request).await;
        assert_eq!(
            imported["KeyMaterialId"].as_str().unwrap_or_default(),
            next_id,
            "case {case:?}: the answer names the material that was imported, got {imported}"
        );

        // The identical pair presented again: the material is the key's own by now, so it
        // is not the new material the request says it is.
        let (status, code, message) = refused(&router, REGION, "ImportKeyMaterial", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "IncorrectKeyMaterialException"),
            "case {case:?}: a replayed import is not a second one, got {status} {code:?}"
        );
        assert_eq!(
            message,
            "expected new key material but the key material being imported is already \
             associated with the KMS key",
            "case {case:?}: unexpected refusal wording, got {message:?}"
        );

        // Nothing about the key's own material changed: the new material is WAITING for a
        // rotation, which is a state of its own, and the key still encrypts under the
        // material it had.
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["CurrentKeyMaterialId"].as_str().unwrap_or_default(),
            first_id,
            "case {case:?}: material imported for a rotation is not current until the \
             rotation happens, got {described}"
        );
        let materials = key_materials(&router, &key_id).await;
        assert_eq!(
            in_state(&materials, "PENDING_ROTATION"),
            vec![next_id.clone()],
            "case {case:?}: exactly the newly imported material waits for the rotation, \
             got {materials:?}"
        );
        assert_eq!(
            in_state(&materials, "CURRENT"),
            vec![first_id.clone()],
            "case {case:?}: the material the key encrypts under is still the first one, \
             got {materials:?}"
        );

        // The rotation the material was imported for makes it current, retains the
        // version it replaced, and is recorded as the on-demand rotation it was.
        rotate_on_demand(&router, case, &key_id).await;
        let materials = key_materials(&router, &key_id).await;
        assert_eq!(
            in_state(&materials, "CURRENT"),
            vec![next_id.clone()],
            "case {case:?}: the rotation makes the imported material current, got \
             {materials:?}"
        );
        assert_eq!(
            in_state(&materials, "NON_CURRENT"),
            vec![first_id.clone()],
            "case {case:?}: the material it replaced is retained, got {materials:?}"
        );
        assert!(
            in_state(&materials, "PENDING_ROTATION").is_empty(),
            "case {case:?}: nothing waits for a rotation that has happened, got \
             {materials:?}"
        );
        let rotation = materials
            .iter()
            .find(|entry| entry["KeyMaterialId"].as_str() == Some(next_id.as_str()))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            rotation["RotationType"].as_str().unwrap_or_default(),
            "ON_DEMAND",
            "case {case:?}: the rotation is the on-demand one that was asked for, got \
             {rotation}"
        );
        // The material the rotation made current came from a caller, so the entry carries
        // the imported-material members, filled from what that import stated.
        assert_eq!(
            rotation["ImportState"].as_str().unwrap_or_default(),
            "IMPORTED",
            "case {case:?}: the rotation installed material a caller imported, got \
             {rotation}"
        );
        assert_eq!(
            rotation["ExpirationModel"].as_str().unwrap_or_default(),
            "KEY_MATERIAL_DOES_NOT_EXPIRE",
            "case {case:?}: the entry reports what the import stated, got {rotation}"
        );

        // "KMS retains all key material": the ciphertext sealed before the rotation still
        // opens, under the version that sealed it.
        let opened = ok(&router, "Decrypt", &json!({"CiphertextBlob": blob})).await;
        assert_eq!(
            unb64(case, &opened, "Plaintext"),
            plaintext,
            "case {case:?}: a ciphertext sealed before the rotation must still open, got \
             {opened}"
        );
        assert_eq!(
            opened["KeyMaterialId"].as_str().unwrap_or_default(),
            first_id,
            "case {case:?}: the answer names the material that opened it, got {opened}"
        );

        // Rows of the same rule, now that the key has three versions to be presented
        // with: the material it encrypts under, and the material it retains. Neither is
        // new material.
        for (presented, material) in [("the current material", &next), ("a retained material", &first)] {
            let (public_key, token) =
                parameters_for(&router, case, &key_id, wrapping_algorithm).await;
            let wrapped = wrap(case, &public_key, wrapping_algorithm, material);
            let (status, code, message) = refused(
                &router,
                REGION,
                "ImportKeyMaterial",
                &import_request(&key_id, &token, &wrapped, Some("NEW_KEY_MATERIAL")),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "IncorrectKeyMaterialException"),
                "case {case:?} / {presented}: expected the new-material precondition to \
                 refuse it, got {status} {code:?} {message:?}"
            );
        }

        // The control: the same material, imported as the EXISTING material it now is,
        // is served — it is only the claim that the material is NEW that fails.
        let (public_key, token) = parameters_for(&router, case, &key_id, wrapping_algorithm).await;
        let wrapped = wrap(case, &public_key, wrapping_algorithm, &next);
        let reimported = ok(
            &router,
            "ImportKeyMaterial",
            &import_request(&key_id, &token, &wrapped, Some("EXISTING_KEY_MATERIAL")),
        )
        .await;
        assert_eq!(
            reimported["KeyMaterialId"].as_str().unwrap_or_default(),
            next_id,
            "case {case:?}: re-importing the existing material keeps its identity, got \
             {reimported}"
        );

        // And material that is only WAITING for a rotation can be taken back: it "is not
        // permanently associated with the KMS key", so deleting it by name leaves the
        // material the key encrypts under untouched and the key usable.
        let waiting = random_bytes(SYMMETRIC_MATERIAL_LEN).expect("entropy");
        let waiting_id = symmetric_material_id(case, &waiting);
        let (public_key, token) = parameters_for(&router, case, &key_id, wrapping_algorithm).await;
        let wrapped = wrap(case, &public_key, wrapping_algorithm, &waiting);
        ok(
            &router,
            "ImportKeyMaterial",
            &import_request(&key_id, &token, &wrapped, Some("NEW_KEY_MATERIAL")),
        )
        .await;
        let deleted = ok(
            &router,
            "DeleteImportedKeyMaterial",
            &json!({"KeyId": key_id, "KeyMaterialId": waiting_id}),
        )
        .await;
        assert_eq!(
            deleted["KeyMaterialId"].as_str().unwrap_or_default(),
            waiting_id,
            "case {case:?}: the delete names the material it deleted, got {deleted}"
        );
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["KeyState"].as_str().unwrap_or_default(),
            "Enabled",
            "case {case:?}: deleting material that was only waiting leaves the key \
             usable, got {described}"
        );
        assert_eq!(
            described["CurrentKeyMaterialId"].as_str().unwrap_or_default(),
            next_id,
            "case {case:?}: it leaves the material the key encrypts under, got {described}"
        );
        let materials = key_materials(&router, &key_id).await;
        assert!(
            in_state(&materials, "PENDING_ROTATION").is_empty(),
            "case {case:?}: nothing waits for a rotation any more, got {materials:?}"
        );
    }
}

/// Rule 1, continued: material whose expiry has passed is material KMS has already
/// taken away, so an import of NEW material into such a key has no rotation to wait for
/// — it becomes the material the key encrypts under, and the key is usable again.
#[tokio::test]
async fn new_material_for_a_key_whose_material_expired_becomes_current() {
    let router = router();
    let metadata = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let key_id = text("setup", &metadata, "KeyId");

    // The first material, stated to expire a couple of seconds out: short enough that
    // the clock makes the deletion due DURING the test rather than being waited out, and
    // far enough ahead that the import itself still lands inside the window on a loaded
    // machine — `ValidTo` is refused unless it is in the future when the handler reads
    // it, so a sub-second margin races the request it travels in.
    let expiring = random_bytes(SYMMETRIC_MATERIAL_LEN).expect("entropy");
    let (public_key, token) =
        parameters_for(&router, "setup", &key_id, "RSAES_OAEP_SHA_256").await;
    let wrapped = wrap("setup", &public_key, "RSAES_OAEP_SHA_256", &expiring);
    let valid_to = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
        .trunc()
        + 3.0;
    ok(
        &router,
        "ImportKeyMaterial",
        &json!({
            "KeyId": key_id,
            "ImportToken": token,
            "EncryptedKeyMaterial": b64(&wrapped),
            "ExpirationModel": "KEY_MATERIAL_EXPIRES",
            "ValidTo": valid_to,
        }),
    )
    .await;

    // The expiry is a deletion the CLOCK makes due, so once it passes the key reports
    // that it is waiting for material again. The wait clears the whole margin above.
    tokio::time::sleep(std::time::Duration::from_millis(3_500)).await;
    let described = describe(&router, &key_id).await;
    assert_eq!(
        described["KeyState"].as_str().unwrap_or_default(),
        "PendingImport",
        "case expired material: a key whose material expired waits for material, got \
         {described}"
    );

    // NEW material for that key is not material for a rotation: there is no version to
    // rotate from, so it becomes the key's own material at once.
    let replacement = random_bytes(SYMMETRIC_MATERIAL_LEN).expect("entropy");
    let replacement_id = symmetric_material_id("expired material", &replacement);
    let (public_key, token) =
        parameters_for(&router, "expired material", &key_id, "RSAES_OAEP_SHA_256").await;
    let wrapped = wrap(
        "expired material",
        &public_key,
        "RSAES_OAEP_SHA_256",
        &replacement,
    );
    let imported = ok(
        &router,
        "ImportKeyMaterial",
        &import_request(&key_id, &token, &wrapped, Some("NEW_KEY_MATERIAL")),
    )
    .await;
    assert_eq!(
        imported["KeyMaterialId"].as_str().unwrap_or_default(),
        replacement_id,
        "case expired material: the answer names the material that was imported, got \
         {imported}"
    );
    let described = describe(&router, &key_id).await;
    assert_eq!(
        described["KeyState"].as_str().unwrap_or_default(),
        "Enabled",
        "case expired material: the key is usable again, got {described}"
    );
    assert_eq!(
        described["CurrentKeyMaterialId"].as_str().unwrap_or_default(),
        replacement_id,
        "case expired material: the new material is the current one, got {described}"
    );
    let materials = key_materials(&router, &key_id).await;
    assert!(
        in_state(&materials, "PENDING_ROTATION").is_empty(),
        "case expired material: nothing is waiting for a rotation, got {materials:?}"
    );
}

/// Rule 1, continued: the material-identity rule is about the MATERIAL, so it holds for
/// every kind of key material — including the kinds KMS reports no `KeyMaterialId` for.
#[tokio::test]
async fn new_material_is_judged_by_the_material_for_every_kind_of_key() {
    let router = router();

    // Rows: the kind of key material the key holds. They span the kinds whose bytes this
    // store holds and that report NO KeyMaterialId (an HMAC key and an asymmetric one),
    // which is exactly where an identity read off the reported id would stop working.
    let rows: Vec<(&str, Value, Vec<u8>)> = vec![
        (
            "an HMAC key",
            json!({"Origin": "EXTERNAL", "KeySpec": "HMAC_256",
                   "KeyUsage": "GENERATE_VERIFY_MAC"}),
            random_bytes(32).expect("entropy"),
        ),
        (
            "an asymmetric key",
            json!({"Origin": "EXTERNAL", "KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
            private_key_of("an asymmetric key", "RSA_2048"),
        ),
    ];
    for (case, create, material) in rows {
        let metadata = create_key(&router, &create).await;
        let key_id = text(case, &metadata, "KeyId");

        // The first import: the key had no material, so this material is new.
        let (public_key, token) =
            parameters_for(&router, case, &key_id, "RSA_AES_KEY_WRAP_SHA_256").await;
        let wrapped = wrap(case, &public_key, "RSA_AES_KEY_WRAP_SHA_256", &material);
        ok(
            &router,
            "ImportKeyMaterial",
            &import_request(&key_id, &token, &wrapped, None),
        )
        .await;

        // The same material, now presented as NEW material: the key is associated with
        // it, whether or not any id for it is ever reported.
        let (public_key, token) =
            parameters_for(&router, case, &key_id, "RSA_AES_KEY_WRAP_SHA_256").await;
        let wrapped = wrap(case, &public_key, "RSA_AES_KEY_WRAP_SHA_256", &material);
        let (status, code, message) = refused(
            &router,
            REGION,
            "ImportKeyMaterial",
            &import_request(&key_id, &token, &wrapped, Some("NEW_KEY_MATERIAL")),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "IncorrectKeyMaterialException"),
            "case {case:?}: expected the new-material precondition to refuse it, got \
             {status} {code:?}"
        );
        assert_eq!(
            message,
            "expected new key material but the key material being imported is already \
             associated with the KMS key",
            "case {case:?}: unexpected refusal wording, got {message:?}"
        );

        // The control: the same material, imported as the EXISTING material it is — the
        // documented way to restore a key whose material was deleted or expired — is
        // served, and the key is usable again.
        let (public_key, token) =
            parameters_for(&router, case, &key_id, "RSA_AES_KEY_WRAP_SHA_256").await;
        let wrapped = wrap(case, &public_key, "RSA_AES_KEY_WRAP_SHA_256", &material);
        ok(
            &router,
            "ImportKeyMaterial",
            &import_request(&key_id, &token, &wrapped, Some("EXISTING_KEY_MATERIAL")),
        )
        .await;
        assert_eq!(
            describe(&router, &key_id).await["KeyState"]
                .as_str()
                .unwrap_or_default(),
            "Enabled",
            "case {case:?}: re-importing the key's own material leaves it usable"
        );
    }
}

/// A freshly generated private key of the key pair `key_spec` names, as PKCS#8 DER —
/// what a caller importing asymmetric key material wraps. Each key pair comes from the
/// library that implements its algorithm, not from bytes written here.
fn private_key_of(case: &str, key_spec: &str) -> Vec<u8> {
    match key_spec {
        "RSA_2048" | "RSA_3072" => {
            let key_size = if key_spec == "RSA_2048" {
                aws_lc_rs::rsa::KeySize::Rsa2048
            } else {
                aws_lc_rs::rsa::KeySize::Rsa3072
            };
            let private = aws_lc_rs::rsa::PrivateDecryptingKey::generate(key_size)
                .unwrap_or_else(|error| panic!("case {case}: an RSA key pair generates: {error}"));
            let der: Pkcs8V1Der<'static> = AsDer::as_der(&private)
                .unwrap_or_else(|error| panic!("case {case}: the private key encodes: {error}"));
            der.as_ref().to_vec()
        }
        "ECC_NIST_P256" | "ECC_NIST_P384" => {
            let algorithm = if key_spec == "ECC_NIST_P256" {
                &ECDSA_P256_SHA256_ASN1_SIGNING
            } else {
                &ECDSA_P384_SHA384_ASN1_SIGNING
            };
            let pair = EcdsaKeyPair::generate(algorithm)
                .unwrap_or_else(|error| panic!("case {case}: an EC key pair generates: {error}"));
            pair.to_pkcs8v1()
                .unwrap_or_else(|error| panic!("case {case}: the private key encodes: {error}"))
                .as_ref()
                .to_vec()
        }
        "ECC_NIST_EDWARDS25519" => Ed25519KeyPair::generate()
            .unwrap_or_else(|error| panic!("case {case}: an Ed25519 key pair generates: {error}"))
            .to_pkcs8v1()
            .unwrap_or_else(|error| panic!("case {case}: the private key encodes: {error}"))
            .as_ref()
            .to_vec(),
        "SM2" => {
            let scalar = random_bytes(32).expect("entropy");
            let secret = sm2::SecretKey::from_slice(&scalar)
                .unwrap_or_else(|error| panic!("case {case}: the scalar is an SM2 key: {error}"));
            secret
                .to_pkcs8_der()
                .unwrap_or_else(|error| panic!("case {case}: the private key encodes: {error}"))
                .as_bytes()
                .to_vec()
        }
        other => panic!("case {case}: {other} is not a key spec this test generates"),
    }
}

/// Rule 2: the key material of an asymmetric KMS key is the private key of a key pair of
/// its own spec.
#[tokio::test]
async fn an_asymmetric_key_takes_the_private_key_of_a_key_pair_of_its_spec() {
    let router = router();

    // Rows: the key spec of the KMS key. They span the asymmetric families — RSA at two
    // modulus sizes, two NIST curves, Edwards25519 and SM2 — and each row imports a
    // private key of its OWN spec, which must be served, and one of ANOTHER spec, which
    // must not. The wrapping is `RSA_AES_KEY_WRAP_SHA_256`, the only one long material
    // fits in: "supported for wrapping RSA and ECC key material".
    let rows = [
        ("RSA_2048", "SIGN_VERIFY", "ECC_NIST_P256"),
        ("RSA_3072", "ENCRYPT_DECRYPT", "RSA_2048"),
        ("ECC_NIST_P256", "SIGN_VERIFY", "ECC_NIST_P384"),
        ("ECC_NIST_P384", "KEY_AGREEMENT", "ECC_NIST_P256"),
        ("ECC_NIST_EDWARDS25519", "SIGN_VERIFY", "ECC_NIST_P256"),
        ("SM2", "SIGN_VERIFY", "ECC_NIST_EDWARDS25519"),
    ];
    for (key_spec, key_usage, other_spec) in rows {
        let case = key_spec;
        let metadata = create_key(
            &router,
            &json!({"Origin": "EXTERNAL", "KeySpec": key_spec, "KeyUsage": key_usage}),
        )
        .await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");
        assert_eq!(
            metadata["KeyState"].as_str().unwrap_or_default(),
            "PendingImport",
            "case {case:?}: a key created with no material waits for it, got {metadata}"
        );

        // Bytes that are not material of this key: a private key of another spec, and
        // bytes that are no private key at all.
        for (presented, bytes) in [
            ("a private key of another spec", private_key_of(case, other_spec)),
            ("bytes that are no private key", random_bytes(1218).expect("entropy")),
        ] {
            let (public_key, token) =
                parameters_for(&router, case, &key_id, "RSA_AES_KEY_WRAP_SHA_256").await;
            let wrapped = wrap(case, &public_key, "RSA_AES_KEY_WRAP_SHA_256", &bytes);
            let (status, code, _message) = refused(
                &router,
                REGION,
                "ImportKeyMaterial",
                &import_request(&key_id, &token, &wrapped, None),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "IncorrectKeyMaterialException"),
                "case {case:?} / {presented}: expected the material to be refused, got \
                 {status} {code:?}"
            );
            assert_eq!(
                describe(&router, &key_id).await["KeyState"]
                    .as_str()
                    .unwrap_or_default(),
                "PendingImport",
                "case {case:?} / {presented}: a refused import leaves the key waiting"
            );
        }

        // The private key of a key pair of this key's own spec IS its material.
        let private_key = private_key_of(case, key_spec);
        let (public_key, token) =
            parameters_for(&router, case, &key_id, "RSA_AES_KEY_WRAP_SHA_256").await;
        let wrapped = wrap(case, &public_key, "RSA_AES_KEY_WRAP_SHA_256", &private_key);
        let imported = ok(
            &router,
            "ImportKeyMaterial",
            &import_request(&key_id, &token, &wrapped, None),
        )
        .await;
        assert_eq!(
            imported["KeyId"].as_str().unwrap_or_default(),
            key_arn,
            "case {case:?}: the answer names the key by its ARN, got {imported}"
        );

        // Read the key back: it has material now, so it is usable, and it still reports
        // the spec and origin it was created with. `CurrentKeyMaterialId` stays absent
        // because it "is present for symmetric encryption keys".
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["KeyState"].as_str().unwrap_or_default(),
            "Enabled",
            "case {case:?}: a key with key material is usable, got {described}"
        );
        assert_eq!(
            described["Origin"].as_str().unwrap_or_default(),
            "EXTERNAL",
            "case {case:?}: importing material does not change the origin, got {described}"
        );
        assert_eq!(
            described["KeySpec"].as_str().unwrap_or_default(),
            key_spec,
            "case {case:?}: importing material does not change the key spec, got {described}"
        );
        assert!(
            described["CurrentKeyMaterialId"].is_null(),
            "case {case:?}: an asymmetric key reports no CurrentKeyMaterialId, got \
             {described}"
        );
    }
}

/// Rule 3: the wording of each refusal class on the import and decrypt paths.
#[tokio::test]
async fn each_refusal_carries_the_wording_aws_words_that_class_with() {
    let router = router();

    // An import token is bound to ONE key: a token that is not bound to the key the
    // request names is refused with no message at all, whichever way it fails to be.
    let first = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let first_id = text("setup", &first, "KeyId");
    let second = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let second_id = text("setup", &second, "KeyId");
    let second_arn = text("setup", &second, "Arn");
    let (public_key, token) = parameters_for(&router, "setup", &first_id, "RSAES_OAEP_SHA_256").await;
    let material = random_bytes(SYMMETRIC_MATERIAL_LEN).expect("entropy");
    let wrapped = wrap("setup", &public_key, "RSAES_OAEP_SHA_256", &material);

    // Rows: how the token fails to be bound to the key the request names — a token this
    // service never minted, the shortest and a longer one, and a token minted for ANOTHER
    // key.
    let token_rows: Vec<(&str, Value)> = vec![
        (
            "a token this service never minted",
            import_request(&second_id, &b64(&[0u8; 32]), &wrapped, None),
        ),
        (
            "the shortest token the model admits",
            import_request(&second_id, &b64(&[0u8; 1]), &wrapped, None),
        ),
        (
            "a token minted for another key",
            import_request(&second_id, &token, &wrapped, None),
        ),
    ];
    for (case, body) in token_rows {
        let (status, code, message) = refused(&router, REGION, "ImportKeyMaterial", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidImportTokenException"),
            "case {case:?}: expected the import-token refusal, got {status} {code:?}"
        );
        assert!(
            message.is_empty(),
            "case {case:?}: AWS words this refusal with no message at all, got {message:?}"
        );
        assert!(
            !message.contains(&second_arn) && !message.contains(&second_id),
            "case {case:?}: the refusal names no key, got {message:?}"
        );
    }

    // A ciphertext that does not open under the key and encryption context given is
    // refused with no message either, whichever part of the envelope disagrees.
    let key = create_key(&router, &json!({"Description": "decrypt refusals"})).await;
    let key_id = text("setup", &key, "KeyId");
    let context = json!({"tenant": "acme", "env": "sandbox", "batch": "0043"});
    let sealed = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": key_id, "Plaintext": b64(b"sealed"), "EncryptionContext": context}),
    )
    .await;
    let blob = unb64("setup", &sealed, "CiphertextBlob");
    let mut flipped = blob.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0x01;

    // Rows: WHAT about the presented ciphertext does not match what sealed it — the bytes
    // themselves, their length, or any one difference in the encryption context, whose
    // ORDER is not one of them.
    let ciphertext_rows: Vec<(&str, Value)> = vec![
        ("bytes that are not a ciphertext", json!({"CiphertextBlob": b64(&[0u8; 64])})),
        ("a truncated envelope", json!({"CiphertextBlob": b64(&blob[..blob.len() / 2])})),
        ("one byte of the envelope flipped", json!({"CiphertextBlob": b64(&flipped)})),
        (
            "a context value changed",
            json!({"CiphertextBlob": b64(&blob),
                   "EncryptionContext": {"env": "sandbox", "tenant": "acme", "batch": "0044"}}),
        ),
        (
            "a context pair dropped",
            json!({"CiphertextBlob": b64(&blob),
                   "EncryptionContext": {"tenant": "acme", "env": "sandbox"}}),
        ),
        (
            "a context pair added",
            json!({"CiphertextBlob": b64(&blob),
                   "EncryptionContext": {"tenant": "acme", "env": "sandbox", "batch": "0043",
                                         "zone": "west"}}),
        ),
        ("no context at all", json!({"CiphertextBlob": b64(&blob)})),
    ];
    for (case, body) in ciphertext_rows {
        let (status, code, message) = refused(&router, REGION, "Decrypt", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidCiphertextException"),
            "case {case:?}: expected the ciphertext refusal, got {status} {code:?}"
        );
        assert!(
            message.is_empty(),
            "case {case:?}: AWS words this refusal with no message at all, got {message:?}"
        );
    }

    // The control the rows are measured against: the SAME pairs in another order are the
    // same encryption context, so the ciphertext opens.
    let opened = ok(
        &router,
        "Decrypt",
        &json!({"CiphertextBlob": b64(&blob),
                "EncryptionContext": {"batch": "0043", "tenant": "acme", "env": "sandbox"}}),
    )
    .await;
    assert_eq!(
        unb64("reordered context", &opened, "Plaintext"),
        b"sealed",
        "case reordered context: an encryption context is a map, not a sequence, got \
         {opened}"
    );

    // A `KeyId` that names an existing key other than the one that sealed the ciphertext
    // is one fixed sentence that names NEITHER key — not the key the caller presented,
    // and not the key it would disclose.
    let other = create_key(&router, &json!({"Description": "did not seal it"})).await;
    let other_id = text("setup", &other, "KeyId");
    let other_arn = text("setup", &other, "Arn");
    let key_arn = text("setup", &key, "Arn");
    create_alias(&router, "setup", "alias/did-not-seal-it", &other_id).await;

    // Rows: how the request names that other key — by key id, by key ARN, and by an
    // alias of it.
    for (case, identifier) in [
        ("by key id", other_id.clone()),
        ("by key ARN", other_arn.clone()),
        ("by alias", "alias/did-not-seal-it".to_string()),
    ] {
        let (status, code, message) = refused(
            &router,
            REGION,
            "Decrypt",
            &json!({"CiphertextBlob": b64(&blob), "KeyId": identifier,
                    "EncryptionContext": context}),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "IncorrectKeyException"),
            "case {case:?}: expected the wrong-key refusal, got {status} {code:?}"
        );
        assert_eq!(
            message,
            "The key ID in the request does not identify a CMK that can perform this \
             operation.",
            "case {case:?}: unexpected refusal wording, got {message:?}"
        );
        assert!(
            !message.contains(&key_arn) && !message.contains(&other_arn),
            "case {case:?}: the refusal names neither key, got {message:?}"
        );
    }

    // The control: the key that DID seal the ciphertext decrypts it, and is the key the
    // answer names.
    let opened = ok(
        &router,
        "Decrypt",
        &json!({"CiphertextBlob": b64(&blob), "KeyId": key_id, "EncryptionContext": context}),
    )
    .await;
    assert_eq!(
        opened["KeyId"].as_str().unwrap_or_default(),
        key_arn,
        "case the sealing key: the answer names the key that opened it, got {opened}"
    );

    // And a `KeyId` that names NO key is a different failure from naming the wrong one:
    // the resource is missing, not incorrect.
    let (status, payload) = call(
        &router,
        REGION,
        "Decrypt",
        &json!({"CiphertextBlob": b64(&blob), "KeyId": "00000000-0000-0000-0000-000000000000",
                "EncryptionContext": context}),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload).as_str()),
        (400, "NotFoundException"),
        "case a key that does not exist: expected the missing-resource refusal, got \
         {status} {payload}"
    );
    assert!(
        !error_message(&payload).is_empty(),
        "case a key that does not exist: this refusal does say what was not found, got \
         {payload}"
    );
}

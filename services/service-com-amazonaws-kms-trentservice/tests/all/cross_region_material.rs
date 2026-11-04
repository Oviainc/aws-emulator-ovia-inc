// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Region boundary of KMS key MATERIAL, driven over the wire
//! through the shared [`super::harness`].
//!
//! The rule under test: an operation that OPENS a ciphertext can use only key material
//! of the Region its endpoint serves, so a request naming material this endpoint holds
//! no key for — the key the ciphertext envelope names, or a `KeyId` that is an ARN of
//! another Region — is refused on AUTHORIZATION (`AccessDeniedException`, HTTP 400),
//! naming the caller and the action and nothing about the resource beyond its being the
//! one the ciphertext is associated with, and never reported as a lookup miss: whether
//! that key exists elsewhere is exactly what this endpoint cannot tell the caller.
//!
//! The boundary is about MATERIAL and not about routing, which is what the last two
//! tests pin: a ciphertext still opens at the endpoint whose key sealed it, a key of
//! this Region stays usable, an identifier that simply names no key of THIS Region is
//! still `NotFoundException`, and a replica key of a multi-Region set opens its
//! primary's ciphertext, because the keys of a set share their material.
//!
//! The rows span the three ways a request can point at another Region's material (the
//! ciphertext alone, the ciphertext with a local key named, the ciphertext with the
//! foreign key named), the pair of operations that open a ciphertext, and the direction
//! the two Regions are used in.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    b64, call, describe_in, error_code, error_message, ok_in, replicate_into, router, text, REGION,
};

/// The Region whose endpoint mints the key material in these cases.
const HOME: &str = REGION;

/// The Region whose endpoint is handed that material and must refuse it.
const ELSEWHERE: &str = "us-west-2";

/// The whole of the message KMS words this refusal with: the authorization-failure
/// sentence every AWS service words, naming the principal it refused and the action it
/// refused — and, of the resource, only that it is the one the CIPHERTEXT is associated
/// with, since each of the three reasons the request can be refused for would disclose
/// something this endpoint cannot answer for.
///
/// `action` is the IAM action the refused call performs on the ciphertext's key: the
/// operation's own name for `Decrypt`, and the SOURCE half for `ReEncrypt`, which opens
/// a ciphertext under `kms:ReEncryptFrom`.
fn foreign_material_message(action: &str) -> String {
    format!(
        "User: arn:aws:iam::{account}:user/emulator is not authorized to perform: \
         kms:{action} on the resource associated with this ciphertext because the \
         resource does not exist in this Region, no resource-based policies allow \
         access, or a resource-based policy explicitly denies access",
        account = engine::emulator_account_id()
    )
}

/// The IAM action `operation` performs on the key that sealed the ciphertext it was
/// given — see [`foreign_material_message`].
fn ciphertext_action(operation: &str) -> &'static str {
    if operation == "ReEncrypt" {
        "ReEncryptFrom"
    } else {
        "Decrypt"
    }
}

/// The key id a key ARN names: its resource part after `key/`.
fn key_id_of(key_arn: &str) -> String {
    key_arn
        .rsplit('/')
        .next()
        .expect("a key ARN names its key id")
        .to_string()
}

/// A key in `region` and a ciphertext it sealed, reported as `(key ARN, ciphertext)`.
async fn sealed_in(
    router: &MultiServiceRouter,
    region: &str,
    plaintext: &[u8],
) -> (String, String) {
    let metadata = ok_in(
        router,
        region,
        "CreateKey",
        &json!({"Description": format!("material of {region}")}),
    )
    .await["KeyMetadata"]
        .clone();
    let key_arn = text(region, &metadata, "Arn");
    let sealed = ok_in(
        router,
        region,
        "Encrypt",
        &json!({"KeyId": key_arn, "Plaintext": b64(plaintext)}),
    )
    .await;
    (key_arn, text(region, &sealed, "CiphertextBlob"))
}

/// Material of another Region is an authorization refusal at this endpoint, however the
/// request points at it — and the same refusal in either direction between two Regions.
#[tokio::test]
async fn key_material_of_another_region_is_refused_on_authorization() {
    let router = router();
    let plaintext = b"single-region-material-must-not-travel";

    // Rows: the Region that minted the material, and the Region handed it.
    for (home, elsewhere) in [(HOME, ELSEWHERE), (ELSEWHERE, HOME)] {
        let (foreign_arn, foreign_blob) = sealed_in(&router, home, plaintext).await;
        let (local_arn, _local_blob) = sealed_in(&router, elsewhere, plaintext).await;
        let foreign_id = key_id_of(&foreign_arn);

        // Rows: a request at `elsewhere`'s endpoint that points at `home`'s material.
        let cases = vec![
            (
                "Decrypt naming the foreign key by its ARN",
                "Decrypt",
                json!({"CiphertextBlob": foreign_blob, "KeyId": foreign_arn}),
            ),
            (
                "Decrypt naming a key of this Region",
                "Decrypt",
                json!({"CiphertextBlob": foreign_blob, "KeyId": local_arn}),
            ),
            (
                "Decrypt naming no key at all",
                "Decrypt",
                json!({"CiphertextBlob": foreign_blob}),
            ),
            (
                "Decrypt naming the foreign key by its bare key id",
                "Decrypt",
                json!({"CiphertextBlob": foreign_blob, "KeyId": foreign_id}),
            ),
            (
                "ReEncrypt of foreign bytes into a key of this Region",
                "ReEncrypt",
                json!({"CiphertextBlob": foreign_blob, "DestinationKeyId": local_arn}),
            ),
            (
                "ReEncrypt naming the foreign key as its source",
                "ReEncrypt",
                json!({"CiphertextBlob": foreign_blob, "SourceKeyId": foreign_arn,
                       "DestinationKeyId": local_arn}),
            ),
        ];
        for (described, operation, body) in cases {
            let case = format!("{described} at {elsewhere}");
            let (status, payload) = call(&router, elsewhere, operation, &body).await;
            let code = error_code(&payload);
            let message = error_message(&payload);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                code, "AccessDeniedException",
                "case {case:?}: material of another Region is an authorization refusal, \
                 got {code:?} in {payload}"
            );
            assert_eq!(
                message,
                foreign_material_message(ciphertext_action(operation)),
                "case {case:?}: expected the refusal KMS words for material it cannot \
                 authorize, got {message:?}"
            );
        }

        // A foreign key ARN named beside a ciphertext this endpoint CAN open is the same
        // refusal: the key named is material of another Region either way.
        let local_sealed = ok_in(
            &router,
            elsewhere,
            "Encrypt",
            &json!({"KeyId": local_arn, "Plaintext": b64(plaintext)}),
        )
        .await;
        let local_blob = text("local", &local_sealed, "CiphertextBlob");
        let case = format!("a foreign KeyId beside this Region's own ciphertext at {elsewhere}");
        let (status, payload) = call(
            &router,
            elsewhere,
            "Decrypt",
            &json!({"CiphertextBlob": local_blob, "KeyId": foreign_arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "AccessDeniedException",
            "case {case:?}: a key ARN of another Region is an authorization refusal, got \
             {payload}"
        );

        // The positive controls: the ciphertext still opens where its key lives, and the
        // key of this Region is untouched and still usable.
        let opened = ok_in(
            &router,
            home,
            "Decrypt",
            &json!({"CiphertextBlob": foreign_blob, "KeyId": foreign_arn}),
        )
        .await;
        assert_eq!(
            opened["Plaintext"],
            json!(b64(plaintext)),
            "case {home}: the ciphertext opens at the endpoint whose key sealed it, got \
             {opened}"
        );
        let local = describe_in(&router, elsewhere, &local_arn).await;
        assert_eq!(
            local["KeyState"],
            json!("Enabled"),
            "case {elsewhere}: the refused calls leave this Region's key enabled, got \
             {local}"
        );
        let reopened = ok_in(
            &router,
            elsewhere,
            "Decrypt",
            &json!({"CiphertextBlob": local_blob, "KeyId": local_arn}),
        )
        .await;
        assert_eq!(
            reopened["Plaintext"],
            json!(b64(plaintext)),
            "case {elsewhere}: this Region's key still opens its own ciphertext, got \
             {reopened}"
        );
    }
}

/// An identifier that names no key of THIS Region, and is not an ARN of another one, is
/// still reported as a resource that does not exist: the authorization refusal is about
/// material of another Region, not about every failed lookup.
#[tokio::test]
async fn an_identifier_naming_no_key_of_this_region_is_still_not_found() {
    let router = router();
    let (key_arn, blob) = sealed_in(&router, HOME, b"local material").await;

    // Rows: identifiers of this Region's endpoint that resolve to no key.
    let absent = [
        ("an unknown key id", "00000000-0000-0000-0000-000000000000".to_string()),
        (
            "an unknown key ARN of this Region",
            format!("{}/{}", key_arn.rsplit_once('/').expect("a key ARN").0, "0".repeat(8)),
        ),
        ("an alias nothing created", "alias/not-created".to_string()),
    ];
    for (described, identifier) in absent {
        let case = format!("Decrypt naming {described}");
        let (status, payload) = call(
            &router,
            HOME,
            "Decrypt",
            &json!({"CiphertextBlob": blob, "KeyId": identifier}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "NotFoundException",
            "case {case:?}: an identifier of this Region that names no key is a lookup \
             miss, got {payload}"
        );
    }
}

/// The material the keys of a multi-Region set SHARE crosses Regions, which is the
/// control that separates key-material isolation from a blanket cross-Region refusal: a
/// replica opens the ciphertext its primary key sealed, at its own endpoint.
#[tokio::test]
async fn a_multi_region_replica_opens_its_primarys_ciphertext() {
    let router = router();
    let plaintext = b"shared multi-region material".to_vec();

    let metadata = ok_in(
        &router,
        HOME,
        "CreateKey",
        &json!({"MultiRegion": true, "Description": "shared material"}),
    )
    .await["KeyMetadata"]
        .clone();
    let key_id = text("setup", &metadata, "KeyId");
    let primary_arn = text("setup", &metadata, "Arn");

    // Rows: the Regions the set is replicated into, so the control holds for more than
    // one replica of one set.
    let replicas = ["us-west-2", "eu-west-1"];
    replicate_into(&router, "setup", &key_id, &replicas).await;

    let sealed = ok_in(
        &router,
        HOME,
        "Encrypt",
        &json!({"KeyId": primary_arn, "Plaintext": b64(&plaintext),
                "EncryptionContext": {"purpose": "replication"}}),
    )
    .await;
    let blob = text("setup", &sealed, "CiphertextBlob");

    for replica_region in replicas {
        let replica_arn = text(
            replica_region,
            &describe_in(&router, replica_region, &key_id).await,
            "Arn",
        );
        let opened = ok_in(
            &router,
            replica_region,
            "Decrypt",
            &json!({"CiphertextBlob": blob, "KeyId": replica_arn,
                    "EncryptionContext": {"purpose": "replication"}}),
        )
        .await;
        assert_eq!(
            opened["Plaintext"],
            json!(b64(&plaintext)),
            "case {replica_region}: a replica opens its primary's ciphertext, got {opened}"
        );
        assert_eq!(
            opened["KeyId"],
            json!(replica_arn),
            "case {replica_region}: the answer names the key of the Region that served \
             it, got {opened}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning what `UpdateEncryptionKey` writes: the customer managed KMS
//! key goes in force at exactly the (`resourceType`, `scanType`) scope the request names,
//! so every modelled scope holds its own key at the same time and a later update replaces
//! only its own scope's key — while a request the modelled input constraints refuse puts
//! nothing in force anywhere.
//!
//! The sibling `encryption_key` suite walks one scope per store; this one populates the
//! WHOLE scope space in a single store, which is what proves the scope key is the pair and
//! not one half of it.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.

use serde_json::{json, Value};

// The signed-request harness and the encryption-key request helpers this crate's suites
// share, rather than a second copy of either.
use crate::encryption_key::{
    assert_no_customer_key, get_encryption_key, update_encryption_key, RESOURCE_TYPES, SCAN_TYPES,
};
use crate::filters_and_enablement::{router, send_in};

/// A distinct well-formed `KmsKeyArn` per index, cycling the partitions and the two key-id
/// forms the modelled pattern allows, so a table over indices also spans the value forms.
/// Source: `smithy_query(query="operation", operation="UpdateEncryptionKey")` — `kmsKeyId`
/// pattern
/// `^arn:aws(-(us-gov|cn))?:kms:([a-z0-9][-.a-z0-9]{0,62})?:[0-9]{12}?:key/((<uuid>)|(mrk-[0-9a-zA-Z]{32}))$`.
fn key_arn(index: usize) -> String {
    let lower = format!("{index:08x}");
    let upper = format!("{index:08X}");
    match index % 4 {
        0 => format!("arn:aws:kms:us-east-1:111122223333:key/{lower}-12ab-34cd-56ef-1234567890ab"),
        1 => format!(
            "arn:aws:kms:eu-west-2:111122223333:key/mrk-{lower}0123456789abcdef01234567"
        ),
        2 => format!(
            "arn:aws-us-gov:kms:us-gov-west-1:444455556666:key/{upper}-ABCD-EF01-2345-6789ABCDEF01"
        ),
        _ => format!(
            "arn:aws-cn:kms:cn-north-1:777788889999:key/{lower}-aaaa-bbbb-cccc-dddddddddddd"
        ),
    }
}

/// Every modelled scope, in model order: `ResourceType` × `ScanType`.
fn every_scope() -> Vec<(&'static str, &'static str)> {
    RESOURCE_TYPES
        .iter()
        .flat_map(|resource_type| {
            SCAN_TYPES
                .iter()
                .map(move |scan_type| (*resource_type, *scan_type))
        })
        .collect()
}

/// Read every modelled scope and assert each reports the key `expected` gives for it.
async fn assert_every_scope_reports(
    router: &smithy_server_runtime::MultiServiceRouter,
    round: &str,
    expected: &[(&str, &str, String)],
) {
    for (resource_type, scan_type, kms_key_id) in expected {
        let case = format!("{round}: {resource_type}/{scan_type}");
        let (status, error_type, body) = get_encryption_key(router, resource_type, scan_type).await;
        assert_eq!(
            status, 200,
            "case {case}: GetEncryptionKey: expected 200, got {status} / {body} ({error_type:?})"
        );
        assert_eq!(
            body["kmsKeyId"], *kms_key_id,
            "case {case}: must report its own scope's key {kms_key_id}, got {body}"
        );
    }
}

/// With a key configured for EVERY modelled scope in ONE account, each scope reports the
/// key written for it — no scope shadows another — and a second pass that moves every
/// scope to a different key replaces each one in place.
#[tokio::test]
async fn every_modelled_scope_holds_its_own_key_at_once() {
    let router = router();
    let scopes = every_scope();

    // Nothing is in force to begin with, at any scope.
    for (resource_type, scan_type) in &scopes {
        let case = format!("empty store: {resource_type}/{scan_type}");
        assert_no_customer_key(&router, &case, resource_type, scan_type).await;
    }

    // Two rounds: the first puts a distinct key in force at every scope, the second moves
    // every scope to another distinct key. The offset shifts which key form each scope
    // gets, so a scope is tested with a uuid key id in one round and an `mrk-` one in the
    // other.
    for (round, offset) in [("first round", 0usize), ("second round", 1)] {
        let expected: Vec<(&str, &str, String)> = scopes
            .iter()
            .enumerate()
            .map(|(index, (resource_type, scan_type))| {
                (
                    *resource_type,
                    *scan_type,
                    key_arn(index + offset * scopes.len()),
                )
            })
            .collect();

        for (resource_type, scan_type, kms_key_id) in &expected {
            let case = format!("{round}: {resource_type}/{scan_type} -> {kms_key_id}");
            let (status, error_type, body) =
                update_encryption_key(&router, kms_key_id, resource_type, scan_type).await;
            assert_eq!(
                status, 200,
                "case {case}: UpdateEncryptionKey: expected 200, got {status} / {body} \
                 ({error_type:?})"
            );
            assert_eq!(
                body,
                Value::Object(serde_json::Map::new()),
                "case {case}: UpdateEncryptionKey models an empty output, got {body}"
            );
        }

        assert_every_scope_reports(&router, round, &expected).await;
    }
}

/// `UpdateEncryptionKey` is `@idempotent`: repeating the identical request leaves the same
/// single key in force at that scope and touches no other scope.
#[tokio::test]
async fn repeating_an_update_leaves_the_same_scope_state() {
    let router = router();
    // Rows differ along the scope and the key form repeated.
    let rows: [(&str, &str, usize); 3] = [
        ("AWS_EC2_INSTANCE", "NETWORK", 0),
        ("CODE_REPOSITORY", "CODE", 1),
        ("Microsoft.ContainerRegistry/registry/containerImage", "PACKAGE", 2),
    ];

    for (resource_type, scan_type, index) in rows {
        let kms_key_id = key_arn(index);
        for attempt in 1..=3 {
            let case = format!("{resource_type}/{scan_type} attempt {attempt}");
            let (status, error_type, body) =
                update_encryption_key(&router, &kms_key_id, resource_type, scan_type).await;
            assert_eq!(
                status, 200,
                "case {case}: UpdateEncryptionKey: expected 200, got {status} / {body} \
                 ({error_type:?})"
            );
            let (status, _, body) = get_encryption_key(&router, resource_type, scan_type).await;
            assert_eq!(
                status, 200,
                "case {case}: GetEncryptionKey: expected 200, got {status} / {body}"
            );
            assert_eq!(
                body["kmsKeyId"], kms_key_id,
                "case {case}: the repeated key must stay in force, got {body}"
            );
        }
    }

    // Every scope the rows did not name is still encrypted with an Amazon Web Services
    // owned key.
    for (resource_type, scan_type) in every_scope() {
        if rows
            .iter()
            .any(|(row_resource, row_scan, _)| *row_resource == resource_type && *row_scan == scan_type)
        {
            continue;
        }
        let case = format!("untouched scope {resource_type}/{scan_type}");
        assert_no_customer_key(&router, &case, resource_type, scan_type).await;
    }
}

/// A request the modelled input constraints refuse configures NOTHING: the scope it named
/// keeps the key it had, or keeps having none. Rows differ along WHICH constraint the
/// request violates — an absent required member (all three are `required`) and a token
/// outside a modelled enum.
/// Source: `smithy_query(query="operation", operation="UpdateEncryptionKey")` and
/// `smithy_query(query="shape", shape="ResourceType,ScanType")`.
#[tokio::test]
async fn a_refused_update_configures_nothing() {
    let good_key = key_arn(7);
    let resource_type = "AWS_ECR_REPOSITORY";
    let scan_type = "PACKAGE";

    let refused: [(&str, Value); 9] = [
        ("no member at all", json!({})),
        (
            "an absent kmsKeyId",
            json!({ "resourceType": resource_type, "scanType": scan_type }),
        ),
        (
            "an absent resourceType",
            json!({ "kmsKeyId": good_key, "scanType": scan_type }),
        ),
        (
            "an absent scanType",
            json!({ "kmsKeyId": good_key, "resourceType": resource_type }),
        ),
        (
            "a lower-case resourceType token",
            json!({ "kmsKeyId": good_key, "resourceType": "aws_ecr_repository", "scanType": scan_type }),
        ),
        (
            // `EC2` is a `ResourceScanType` token, not a `ResourceType` one.
            "a token from another enum as resourceType",
            json!({ "kmsKeyId": good_key, "resourceType": "EC2", "scanType": scan_type }),
        ),
        (
            "an empty resourceType",
            json!({ "kmsKeyId": good_key, "resourceType": "", "scanType": scan_type }),
        ),
        (
            "a lower-case scanType token",
            json!({ "kmsKeyId": good_key, "resourceType": resource_type, "scanType": "package" }),
        ),
        (
            "an unmodelled scanType token",
            json!({ "kmsKeyId": good_key, "resourceType": resource_type, "scanType": "SBOM" }),
        ),
    ];

    // First against a store where the scope has no customer managed key: it must stay that
    // way. Then against one where a key is in force: the refusal must not disturb it.
    for configured in [false, true] {
        let router = router();
        if configured {
            let (status, _, body) =
                update_encryption_key(&router, &good_key, resource_type, scan_type).await;
            assert_eq!(
                status, 200,
                "putting a key in force first: expected 200, got {status} / {body}"
            );
        }

        for (name, request) in &refused {
            let case = format!("{name} (key in force: {configured})");
            let (status, error_type, body) = send_in(
                &router,
                "PUT",
                "us-east-1",
                "/encryptionkey/update",
                Some(request),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status} / {body}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException, got {error_type:?} / {body}"
            );

            if configured {
                let (status, _, body) = get_encryption_key(&router, resource_type, scan_type).await;
                assert_eq!(
                    status, 200,
                    "case {case}: GetEncryptionKey after a refusal: expected 200, got {status} / \
                     {body}"
                );
                assert_eq!(
                    body["kmsKeyId"], good_key,
                    "case {case}: a refused update must leave the key in force, got {body}"
                );
            } else {
                assert_no_customer_key(&router, &case, resource_type, scan_type).await;
            }
        }
    }
}

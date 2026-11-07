// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Inspector encryption-key surface:
//! `UpdateEncryptionKey` and `GetEncryptionKey`.
//!
//! The rule under test: a customer managed KMS key is in force for exactly the
//! (`resourceType`, `scanType`) pair it was configured for, so `GetEncryptionKey` reports
//! the key last written for that pair and refuses with 404 `ResourceNotFoundException`
//! while that pair is still encrypted with an Amazon Web Services owned key.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The signed-request harness this crate's suites share: `router()` builds a router over
// its own engine, `send_in` sends one restJson1 request with a given method.
use crate::filters_and_enablement::{router, send_in};

/// Every `ResourceType` token, in model order.
/// Source: `smithy_query(query="shape", shape="ResourceType")`.
pub(crate) const RESOURCE_TYPES: [&str; 8] = [
    "AWS_EC2_INSTANCE",
    "AWS_ECR_CONTAINER_IMAGE",
    "AWS_ECR_REPOSITORY",
    "AWS_LAMBDA_FUNCTION",
    "CODE_REPOSITORY",
    "Microsoft.Compute/virtualMachines",
    "Microsoft.ContainerRegistry/registry/containerImage",
    "Microsoft.Web/sites",
];

/// Every `ScanType` token.
/// Source: `smithy_query(query="shape", shape="ScanType")`, which lists them in this
/// order; the generated contract header in `src/ops/get_encryption_key.rs` lists the same
/// three alphabetically.
pub(crate) const SCAN_TYPES: [&str; 3] = ["NETWORK", "PACKAGE", "CODE"];

/// Well-formed `KmsKeyArn` values, one per form the modelled pattern allows: a
/// single-region key id, a multi-region (`mrk-`) key id, upper-case hex, and the
/// `aws-us-gov` and `aws-cn` partitions.
/// Source: `smithy_query(query="shape", shape="KmsKeyArn")` — pattern
/// `^arn:aws(-(us-gov|cn))?:kms:([a-z0-9][-.a-z0-9]{0,62})?:[0-9]{12}?:key/((<uuid>)|(mrk-[0-9a-zA-Z]{32}))$`.
const KEY_ARNS: [&str; 5] = [
    "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
    "arn:aws:kms:eu-west-2:111122223333:key/mrk-0123456789abcdef0123456789abcdef",
    "arn:aws:kms:ap-southeast-1:555566667777:key/ABCDEF01-2345-6789-ABCD-EF0123456789",
    "arn:aws-us-gov:kms:us-gov-west-1:444455556666:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    "arn:aws-cn:kms:cn-north-1:777788889999:key/mrk-ABCDEFabcdef01234567890123456789",
];

/// `GetEncryptionKey` — `GET /encryptionkey/get`, both members bound to the query string.
pub(crate) async fn get_encryption_key(
    router: &MultiServiceRouter,
    resource_type: &str,
    scan_type: &str,
) -> (u16, Option<String>, Value) {
    // `/` is a legal character in a URI query component (RFC 3986 §3.4), so the
    // `Microsoft.*` `ResourceType` tokens go on the wire as they are.
    let uri = format!("/encryptionkey/get?resourceType={resource_type}&scanType={scan_type}");
    send_in(router, "GET", "us-east-1", &uri, None).await
}

/// `UpdateEncryptionKey` — `PUT /encryptionkey/update`, every member in the body.
pub(crate) async fn update_encryption_key(
    router: &MultiServiceRouter,
    kms_key_id: &str,
    resource_type: &str,
    scan_type: &str,
) -> (u16, Option<String>, Value) {
    let body = json!({
        "kmsKeyId": kms_key_id,
        "resourceType": resource_type,
        "scanType": scan_type,
    });
    send_in(
        router,
        "PUT",
        "us-east-1",
        "/encryptionkey/update",
        Some(&body),
    )
    .await
}

/// Assert `GetEncryptionKey` refuses this pair the way it refuses a pair encrypted with an
/// Amazon Web Services owned key.
pub(crate) async fn assert_no_customer_key(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    scan_type: &str,
) {
    let (status, error_type, body) = get_encryption_key(router, resource_type, scan_type).await;
    assert_eq!(
        status, 404,
        "case {case}: GetEncryptionKey with no customer key: expected 404, got {status} / {body}"
    );
    assert_eq!(
        error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case}: expected ResourceNotFoundException, got {error_type:?} / {body}"
    );
    assert_eq!(
        body["message"], "Customer managed KMS key not found",
        "case {case}: unexpected refusal message, got {body}"
    );
}

/// The whole modelled pair space: for every `ResourceType` × `ScanType` combination,
/// `GetEncryptionKey` first reports "no customer managed key", and after
/// `UpdateEncryptionKey` reports exactly the key that was put in force — and keeps
/// reporting it, because a read changes nothing.
#[tokio::test]
async fn get_reports_the_key_update_put_in_force_for_each_modelled_pair() {
    for (index, resource_type) in RESOURCE_TYPES.iter().enumerate() {
        for (offset, scan_type) in SCAN_TYPES.iter().enumerate() {
            // Rows differ along resource type, scan type AND the form of the key ARN.
            let kms_key_id = KEY_ARNS[(index + offset) % KEY_ARNS.len()];
            let case = format!("{resource_type}/{scan_type} -> {kms_key_id}");
            let router = router();

            assert_no_customer_key(&router, &case, resource_type, scan_type).await;

            let (status, error_type, body) =
                update_encryption_key(&router, kms_key_id, resource_type, scan_type).await;
            assert_eq!(
                status, 200,
                "case {case}: UpdateEncryptionKey: expected 200, got {status} / {body} \
                 ({error_type:?})"
            );

            for read in 1..=2 {
                let (status, error_type, body) =
                    get_encryption_key(&router, resource_type, scan_type).await;
                assert_eq!(
                    status, 200,
                    "case {case}: GetEncryptionKey read #{read}: expected 200, got {status} / \
                     {body} ({error_type:?})"
                );
                assert_eq!(
                    body["kmsKeyId"], kms_key_id,
                    "case {case}: GetEncryptionKey read #{read} must report the configured key, \
                     got {body}"
                );
            }
        }
    }
}

/// A configured key is in force for exactly ONE pair: every other pair still reports no
/// customer managed key, so neither the resource type nor the scan type is ignored.
#[tokio::test]
async fn a_configured_key_is_scoped_to_its_pair() {
    // One row per pair that gets a key; the assertions sweep the other 23 pairs.
    let configured: [(&str, &str, &str); 3] = [
        ("AWS_LAMBDA_FUNCTION", "CODE", KEY_ARNS[0]),
        ("AWS_ECR_CONTAINER_IMAGE", "PACKAGE", KEY_ARNS[1]),
        ("Microsoft.Web/sites", "NETWORK", KEY_ARNS[3]),
    ];

    for (resource_type, scan_type, kms_key_id) in configured {
        let router = router();
        let case = format!("only {resource_type}/{scan_type} configured");

        let (status, error_type, body) =
            update_encryption_key(&router, kms_key_id, resource_type, scan_type).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateEncryptionKey: expected 200, got {status} / {body} \
             ({error_type:?})"
        );

        for other_resource in RESOURCE_TYPES {
            for other_scan in SCAN_TYPES {
                let (status, error_type, body) =
                    get_encryption_key(&router, other_resource, other_scan).await;
                if other_resource == resource_type && other_scan == scan_type {
                    assert_eq!(
                        status, 200,
                        "case {case}: the configured pair: expected 200, got {status} / {body}"
                    );
                    assert_eq!(
                        body["kmsKeyId"], kms_key_id,
                        "case {case}: the configured pair must report its key, got {body}"
                    );
                } else {
                    assert_eq!(
                        status, 404,
                        "case {case}: {other_resource}/{other_scan} must still have no customer \
                         key, got {status} / {body} ({error_type:?})"
                    );
                }
            }
        }
    }
}

/// `UpdateEncryptionKey` is `@idempotent`: the key last written is the one in force, and
/// writing the same key again changes nothing.
#[tokio::test]
async fn updating_a_pair_replaces_the_key_in_force() {
    let router = router();
    let resource_type = "CODE_REPOSITORY";
    let scan_type = "CODE";

    // Each row is the key the pair is moved to; the last two rows repeat one key, which
    // must leave the same state behind.
    let sequence = [KEY_ARNS[0], KEY_ARNS[1], KEY_ARNS[4], KEY_ARNS[4]];
    for (step, kms_key_id) in sequence.iter().enumerate() {
        let case = format!("step {step}: {kms_key_id}");
        let (status, error_type, body) =
            update_encryption_key(&router, kms_key_id, resource_type, scan_type).await;
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
            body["kmsKeyId"], *kms_key_id,
            "case {case}: GetEncryptionKey must report the key last written, got {body}"
        );
    }
}

/// A `kmsKeyId` outside the modelled `KmsKeyArn` pattern is refused with 400
/// `ValidationException`, and the refused request configures nothing: the pair keeps the
/// key it had, or keeps having none.
#[tokio::test]
async fn a_malformed_key_arn_is_refused_and_configures_nothing() {
    // Rows differ along WHICH part of the pattern is violated.
    let malformed: [(&str, &str); 7] = [
        ("empty", ""),
        ("not an arn", "not-an-arn"),
        (
            "an alias rather than a key",
            "arn:aws:kms:us-east-1:111122223333:alias/inspector",
        ),
        (
            "a bare key id",
            "1234abcd-12ab-34cd-56ef-1234567890ab",
        ),
        (
            "a truncated key id",
            "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef",
        ),
        (
            "an 11-digit account",
            "arn:aws:kms:us-east-1:11112222333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        ),
        (
            "an unmodelled partition",
            "arn:aws-iso:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        ),
    ];

    // First with no key configured: the pair must stay unconfigured.
    let unconfigured = router();
    for (case, kms_key_id) in malformed {
        let (status, error_type, body) =
            update_encryption_key(&unconfigured, kms_key_id, "AWS_LAMBDA_FUNCTION", "CODE").await;
        assert_eq!(
            status, 400,
            "case {case}: UpdateEncryptionKey: expected 400, got {status} / {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {error_type:?} / {body}"
        );
        assert_no_customer_key(&unconfigured, case, "AWS_LAMBDA_FUNCTION", "CODE").await;
    }

    // Then with a key in force: the refused request must not disturb it.
    let configured = router();
    let in_force = KEY_ARNS[2];
    let (status, _, body) =
        update_encryption_key(&configured, in_force, "AWS_LAMBDA_FUNCTION", "CODE").await;
    assert_eq!(
        status, 200,
        "UpdateEncryptionKey with a well-formed key: expected 200, got {status} / {body}"
    );
    for (case, kms_key_id) in malformed {
        let (status, _, body) =
            update_encryption_key(&configured, kms_key_id, "AWS_LAMBDA_FUNCTION", "CODE").await;
        assert_eq!(
            status, 400,
            "case {case}: UpdateEncryptionKey: expected 400, got {status} / {body}"
        );
        let (status, _, body) =
            get_encryption_key(&configured, "AWS_LAMBDA_FUNCTION", "CODE").await;
        assert_eq!(
            status, 200,
            "case {case}: GetEncryptionKey after a refusal: expected 200, got {status} / {body}"
        );
        assert_eq!(
            body["kmsKeyId"], in_force,
            "case {case}: a refused update must leave the configured key in force, got {body}"
        );
    }
}

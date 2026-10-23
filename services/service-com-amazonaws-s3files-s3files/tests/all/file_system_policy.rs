// SPDX-License-Identifier: CC0-1.0

//! The IAM resource policy of an S3 File System, end to end over HTTP:
//! `PutFileSystemPolicy`, `GetFileSystemPolicy` and `DeleteFileSystemPolicy`, each
//! asserted against what the service PERSISTED rather than against what one request
//! carried.
//!
//! The suite reuses `file_system_lifecycle`'s harness, so every case goes through the
//! same `MultiServiceRouter` the emulator boots: routing, the restJson1 wire shapes, the
//! modelled response statuses and the modelled error envelopes are all exercised.
//!
//! The rules under test, each stated once and then exercised over a SET of documents:
//!
//! 1. A put attaches the document to the named file system VERBATIM, and a later read
//!    reports that stored document together with the file system's canonical id; a
//!    second put REPLACES it rather than merging into it.
//! 2. `fileSystemId` accepts either form the model allows — the bare id or the file
//!    system's ARN — and both name the same policy sub-resource.
//! 3. A `policy` that is not a valid JSON document following IAM policy syntax is
//!    refused with the modelled `ValidationException` (400), having changed nothing.
//! 4. The policy is a sub-resource: it does not exist until a put creates it, a delete
//!    makes it observably gone, and a read of an absent policy — or of a policy on a
//!    file system this service does not hold — is the modelled
//!    `ResourceNotFoundException` (404).
//! 5. `DeleteFileSystemPolicy` is `@idempotent`: repeating it is the same call, so
//!    detaching an already-absent policy still succeeds with the modelled 204.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    call, create_file_system, created_file_system_id, encoded, new_router, Answer, ROLE_ARN,
};

/// The route all three resource-policy operations are bound to:
/// `PUT|GET|DELETE /file-systems/{fileSystemId}/policy`.
fn policy_route(label: &str) -> String {
    format!("/file-systems/{label}/policy")
}

/// `PutFileSystemPolicy`. `policy` is a `String` member of the JSON body carrying the
/// policy document, so the document rides as JSON text inside JSON.
async fn put_policy(router: &MultiServiceRouter, label: &str, policy: &str) -> Answer {
    call(
        router,
        "PUT",
        &policy_route(label),
        Some(json!({ "policy": policy })),
    )
    .await
}

async fn get_policy(router: &MultiServiceRouter, label: &str) -> Answer {
    call(router, "GET", &policy_route(label), None).await
}

async fn delete_policy(router: &MultiServiceRouter, label: &str) -> Answer {
    call(router, "DELETE", &policy_route(label), None).await
}

/// One statement block of the form `policy`'s documentation describes — "the
/// permissions for accessing the file system" — granting `actions` on the file system to
/// a principal.
fn statement(sid: &str, effect: &str, actions: Value) -> Value {
    json!({
        "Sid": sid,
        "Effect": effect,
        "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
        "Action": actions,
        "Resource": "*",
    })
}

/// The documents rules 1 and 2 are asserted over.
///
/// `PutFileSystemPolicyRequest$policy` is a plain `String` the service stores rather than
/// evaluates, so the rows differ along everything the rule generalises over: how the
/// `Statement` element is written (one block or an array), how many blocks it holds,
/// which optional IAM elements are present, how the JSON is formatted, which characters
/// it carries, and how large it is.
fn valid_policy_table() -> Vec<(&'static str, String)> {
    let allow = statement("AllowRead", "Allow", json!(["s3files:GetObject"]));
    let deny = statement("DenyWrite", "Deny", json!("s3files:PutObject"));
    vec![
        (
            "one statement in a Statement array",
            json!({ "Version": "2012-10-17", "Statement": [allow.clone()] }).to_string(),
        ),
        (
            "Statement written as a single block",
            json!({ "Version": "2012-10-17", "Statement": allow.clone() }).to_string(),
        ),
        (
            "no Version element",
            json!({ "Statement": [allow.clone()] }).to_string(),
        ),
        (
            "an Id element and two statements of opposite Effect",
            json!({
                "Version": "2012-10-17",
                "Id": "s3files-policy",
                "Statement": [allow.clone(), deny.clone()],
            })
            .to_string(),
        ),
        (
            "pretty-printed across several lines",
            serde_json::to_string_pretty(
                &json!({ "Version": "2012-10-17", "Statement": [allow.clone(), deny.clone()] }),
            )
            .expect("render a pretty-printed policy"),
        ),
        (
            "a Condition block and a non-ASCII Sid",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "Zugriff-fürs-Dateisystem",
                    "Effect": "Allow",
                    "Principal": { "Service": "lambda.amazonaws.com" },
                    "Action": "s3files:*",
                    "Resource": "*",
                    "Condition": { "Bool": { "aws:SecureTransport": "true" } },
                }],
            })
            .to_string(),
        ),
        (
            "a large document of 64 statements",
            json!({
                "Version": "2012-10-17",
                "Statement": (0..64)
                    .map(|index| statement(
                        &format!("Allow{index}"),
                        "Allow",
                        json!([format!("s3files:Action{index}")]),
                    ))
                    .collect::<Vec<Value>>(),
            })
            .to_string(),
        ),
    ]
}

/// The documents rule 3 is asserted over: each one fails exactly one clause of "the
/// policy must be a valid JSON document that follows IAM policy syntax" — it does not
/// parse, its top-level element is not an object, or its required `Statement` element is
/// missing, empty or not a statement block.
fn invalid_policy_table() -> Vec<(&'static str, String)> {
    vec![
        ("the empty string", String::new()),
        ("not JSON at all", "allow everything".to_string()),
        (
            "truncated JSON",
            "{\"Version\":\"2012-10-17\",\"Statement\":[".to_string(),
        ),
        (
            "a JSON array at the top level",
            json!([statement("AllowRead", "Allow", json!("s3files:*"))]).to_string(),
        ),
        ("a JSON string at the top level", json!("Allow").to_string()),
        ("a JSON number at the top level", "42".to_string()),
        (
            "an object with no Statement element",
            json!({ "Version": "2012-10-17", "Id": "no-statements" }).to_string(),
        ),
        (
            "an empty Statement array",
            json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
        ),
        (
            "a Statement element that is a string",
            json!({ "Version": "2012-10-17", "Statement": "Allow" }).to_string(),
        ),
        (
            "a Statement array holding something other than a block",
            json!({ "Version": "2012-10-17", "Statement": ["Allow"] }).to_string(),
        ),
    ]
}

/// A well-formed `fs-` id (`FileSystemId`'s `^fs-[0-9a-f]{17,40}$`) that no create ever
/// minted, so it passes the boundary validator and reaches the handler's lookup.
const UNKNOWN_FILE_SYSTEM_ID: &str = "fs-0123456789abcdef0";

/// Rule 1: whatever a put was given is persisted and read back verbatim beside the file
/// system's canonical id, and a second put replaces it.
#[tokio::test]
async fn a_put_policy_is_persisted_verbatim_and_read_back() {
    let router = new_router();
    let id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-policy-roundtrip",
        Some("policy-roundtrip"),
    )
    .await;

    let mut previous: Option<String> = None;
    for (case, document) in valid_policy_table() {
        let put = put_policy(&router, &id, &document).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutFileSystemPolicy, got {} / {:?}",
            put.status, put.body
        );

        let read = get_policy(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetFileSystemPolicy, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("policy"),
            document,
            "case {case:?}: the stored document must be reported byte for byte"
        );
        assert_eq!(
            read.text("fileSystemId"),
            id,
            "case {case:?}: the read must name the file system the policy is attached to"
        );

        // "Creates or replaces": the previous document must be gone, not merged with.
        if let Some(earlier) = previous.replace(document.clone()) {
            if earlier != document {
                assert_ne!(
                    read.text("policy"),
                    earlier,
                    "case {case:?}: a put replaces the attached document, it does not keep \
                     the earlier one"
                );
            }
        }

        // A read reports, it does not consume: a second read answers the same thing.
        let again = get_policy(&router, &id).await;
        assert_eq!(
            again.text("policy"),
            document,
            "case {case:?}: reading a policy must not change what a later caller sees"
        );
    }
}

/// Rule 2: the bare id and the file system's ARN name the same policy sub-resource, on
/// each of the three operations.
#[tokio::test]
async fn either_id_form_names_the_same_policy() {
    let router = new_router();
    let created = create_file_system(
        &router,
        json!({ "bucket": "arn:aws:s3:::s3files-policy-id-forms", "roleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateFileSystem, got {} / {:?}",
        created.status, created.body
    );
    let id = created.text("fileSystemId");
    // A non-greedy `@httpLabel` carries the ARN percent-encoded, which is how an SDK
    // puts a value containing `/` and `:` into a path segment.
    let encoded_arn = encoded(&created.text("fileSystemArn"));

    let document = json!({
        "Version": "2012-10-17",
        "Statement": [statement("AllowRead", "Allow", json!("s3files:GetObject"))],
    })
    .to_string();
    // The write is addressed by the ARN; both reads below then have to find it.
    let put = put_policy(&router, &encoded_arn, &document).await;
    assert_eq!(
        put.status, 200,
        "case \"the ARN\": a put addressed by the ARN must be accepted, got {} / {:?}",
        put.status, put.body
    );

    for (case, label) in [("the bare id", id.as_str()), ("the ARN", encoded_arn.as_str())] {
        let read = get_policy(&router, label).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetFileSystemPolicy, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("policy"),
            document,
            "case {case:?}: both id forms must reach the same policy"
        );
        assert_eq!(
            read.text("fileSystemId"),
            id,
            "case {case:?}: the read reports the canonical id whichever form addressed it"
        );
    }

    // And the delete reaches it through the other form.
    let deleted = delete_policy(&router, &id).await;
    assert_eq!(
        deleted.status, 204,
        "case \"the bare id\": expected the modelled 204 from DeleteFileSystemPolicy, got {} / {:?}",
        deleted.status, deleted.body
    );
    let gone = get_policy(&router, &encoded_arn).await;
    assert_eq!(
        gone.status, 404,
        "case \"the ARN\": a deleted policy must be gone whichever form reads it, got {} / {:?}",
        gone.status, gone.body
    );
}

/// Rule 3: a document that is not a valid JSON IAM policy is refused with the modelled
/// `ValidationException` and leaves the attached policy untouched.
#[tokio::test]
async fn a_policy_that_is_not_a_valid_iam_document_is_refused() {
    let router = new_router();
    let id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-policy-validation",
        Some("policy-validation"),
    )
    .await;

    // A document that IS valid, so every refusal below can be shown to have changed
    // nothing.
    let attached = json!({
        "Version": "2012-10-17",
        "Statement": [statement("AllowRead", "Allow", json!("s3files:GetObject"))],
    })
    .to_string();
    let put = put_policy(&router, &id, &attached).await;
    assert_eq!(
        put.status, 200,
        "expected the baseline put to be accepted, got {} / {:?}",
        put.status, put.body
    );

    for (case, document) in invalid_policy_table() {
        let refused = put_policy(&router, &id, &document).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {:?} / {:?}",
            refused.error_type, refused.body
        );

        let read = get_policy(&router, &id).await;
        assert_eq!(
            read.text("policy"),
            attached,
            "case {case:?}: a refused put must leave the attached policy as it was"
        );
    }
}

/// Rule 4: the policy is a sub-resource — absent until a put creates it, gone after a
/// delete, and never there for a file system this service does not hold.
#[tokio::test]
async fn an_absent_policy_is_the_modelled_not_found() {
    let router = new_router();
    let id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-policy-absent",
        Some("policy-absent"),
    )
    .await;

    let document = json!({
        "Version": "2012-10-17",
        "Statement": [statement("AllowRead", "Allow", json!("s3files:GetObject"))],
    })
    .to_string();

    // Each row is a call that must answer the modelled 404, and the reason it must.
    for (case, answer) in [
        (
            "reading a policy no put has created",
            get_policy(&router, &id).await,
        ),
        (
            "reading the policy of an unknown file system",
            get_policy(&router, UNKNOWN_FILE_SYSTEM_ID).await,
        ),
        (
            "putting a policy on an unknown file system",
            put_policy(&router, UNKNOWN_FILE_SYSTEM_ID, &document).await,
        ),
        (
            "deleting the policy of an unknown file system",
            delete_policy(&router, UNKNOWN_FILE_SYSTEM_ID).await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {case:?}: expected the modelled ResourceNotFoundException, got {:?} / {:?}",
            answer.error_type, answer.body
        );
    }

    // Attached, then detached: the same read flips from 200 back to 404.
    assert_eq!(
        put_policy(&router, &id, &document).await.status,
        200,
        "case \"attach\": expected the put to be accepted"
    );
    assert_eq!(
        get_policy(&router, &id).await.status,
        200,
        "case \"attach\": the attached policy must be readable"
    );
    let deleted = delete_policy(&router, &id).await;
    assert_eq!(
        deleted.status, 204,
        "case \"detach\": expected the modelled 204, got {} / {:?}",
        deleted.status, deleted.body
    );
    let after = get_policy(&router, &id).await;
    assert_eq!(
        after.status, 404,
        "case \"detach\": a detached policy must be observably gone, got {} / {:?}",
        after.status, after.body
    );
    // The policy sub-resource has no id of its own, so its 404 carries one constant,
    // request-independent sentence naming what is missing.
    assert_eq!(
        after.text("message"),
        service_com_amazonaws_s3files_s3files::state::POLICY_NOT_FOUND_MESSAGE,
        "case \"detach\": expected the constant policy-not-found message, got {:?}",
        after.body
    );

    // The policy is a sub-resource of the file system, so it is released with it.
    assert_eq!(
        put_policy(&router, &id, &document).await.status,
        200,
        "case \"released with its file system\": expected the put to be accepted"
    );
    let dropped = call(&router, "DELETE", &format!("/file-systems/{id}"), None).await;
    assert_eq!(
        dropped.status, 204,
        "case \"released with its file system\": expected DeleteFileSystem to succeed, got {} / {:?}",
        dropped.status, dropped.body
    );
    let orphan = get_policy(&router, &id).await;
    assert_eq!(
        orphan.status, 404,
        "case \"released with its file system\": a deleted file system holds no policy, got {} / {:?}",
        orphan.status, orphan.body
    );
}

/// Rule 5: `DeleteFileSystemPolicy` is `@idempotent`, so repeating it — or making the
/// first call against a file system that never had a policy — still answers the modelled
/// 204.
#[tokio::test]
async fn deleting_a_policy_is_idempotent() {
    let router = new_router();
    let id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-policy-idempotent",
        Some("policy-idempotent"),
    )
    .await;
    let document = json!({
        "Version": "2012-10-17",
        "Statement": statement("AllowRead", "Allow", json!("s3files:GetObject")),
    })
    .to_string();

    // Rows: how many policies were attached before the repeated delete.
    for (case, puts) in [("never attached", 0), ("attached once", 1), ("replaced", 2)] {
        let id = if puts == 0 {
            created_file_system_id(
                &router,
                "arn:aws:s3:::s3files-policy-idempotent-fresh",
                Some(&format!("policy-idempotent-{case}")),
            )
            .await
        } else {
            id.clone()
        };
        for _ in 0..puts {
            assert_eq!(
                put_policy(&router, &id, &document).await.status,
                200,
                "case {case:?}: expected the put to be accepted"
            );
        }
        for attempt in 1..=3 {
            let deleted = delete_policy(&router, &id).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?} attempt {attempt}: expected the modelled 204, got {} / {:?}",
                deleted.status, deleted.body
            );
        }
        let read = get_policy(&router, &id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: after the deletes no policy is attached, got {} / {:?}",
            read.status, read.body
        );
    }
}

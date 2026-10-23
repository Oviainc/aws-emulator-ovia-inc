// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the vector bucket policy slice: PutVectorBucketPolicy,
//! GetVectorBucketPolicy and DeleteVectorBucketPolicy, served over the real
//! `restJson1` wire through the router production uses.
//!
//! The rule under test: a vector bucket holds AT MOST ONE policy document, which a
//! put stores verbatim (replacing any earlier one) once it is a policy document by
//! the IAM policy grammar, a get reports verbatim, and a delete removes — and a get
//! of a bucket that carries no policy is the modelled 404, not an empty success.
//! Each table row states the case it covers and every assertion reports the actual
//! value.

use serde_json::{json, Value};
use service_com_amazonaws_s3vectors_s3vectors::state::MAX_POLICY_BYTES;
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{call, error_code, expected_bucket_arn, ok, router};

/// A fresh router with one empty vector bucket of this name.
async fn with_bucket(bucket: &str) -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;
    router
}

/// Store `policy` on `bucket`, asserting the put succeeded.
async fn put_policy(router: &MultiServiceRouter, bucket: &str, policy: &str) {
    ok(
        router,
        "PutVectorBucketPolicy",
        &json!({ "vectorBucketName": bucket, "policy": policy }),
    )
    .await;
}

/// The policy GetVectorBucketPolicy reports for `bucket`.
async fn get_policy(router: &MultiServiceRouter, bucket: &str) -> String {
    let body = ok(
        router,
        "GetVectorBucketPolicy",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;
    body["policy"]
        .as_str()
        .unwrap_or_else(|| panic!("GetVectorBucketPolicy must report a policy string, got {body}"))
        .to_string()
}

/// A policy document whose `Sid` is padded so the whole document is `bytes` long.
///
/// Panics if `bytes` is smaller than the unpadded document, which no caller asks for.
fn policy_of_size(bytes: usize) -> String {
    let document = |pad: usize| {
        format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"{}","Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::123456789012:root"}},"Action":"s3vectors:GetVectors","Resource":"*"}}]}}"#,
            "a".repeat(pad)
        )
    };
    let unpadded = document(0).len();
    let policy = document(bytes - unpadded);
    assert_eq!(
        policy.len(),
        bytes,
        "the padded policy must be exactly {bytes} bytes, got {}",
        policy.len()
    );
    policy
}

/// Policy documents a put must accept and a get must report back unchanged.
///
/// Every row is a policy document by the AWS IAM JSON policy grammar (the IAM
/// reference's "Grammar of the IAM JSON policy language": a `Version`/`Id`/
/// `Statement` object, one statement or an array of them, each with an `Effect` of
/// `Allow` or `Deny` and an `Action` or `NotAction`), and the rows differ in the
/// grammar's optional parts, in the form each member takes, and in document size.
fn accepted_policies() -> Vec<(&'static str, String)> {
    vec![
        (
            "statement array with one Allow",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
                    "Action": "s3vectors:GetVectors",
                    "Resource": "arn:aws:s3vectors:us-east-1:123456789012:bucket/b/index/i",
                }],
            })
            .to_string(),
        ),
        (
            "a single statement object rather than an array",
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3vectors:*",
                    "Resource": "*",
                },
            })
            .to_string(),
        ),
        (
            "no Version member",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": ["s3vectors:ListVectors", "s3vectors:GetVectors"],
                }],
            })
            .to_string(),
        ),
        (
            "Id and Sid, two statements of opposite Effect",
            json!({
                "Version": "2012-10-17",
                "Id": "vector-bucket-policy",
                "Statement": [
                    {
                        "Sid": "AllowReads",
                        "Effect": "Allow",
                        "Principal": { "AWS": ["arn:aws:iam::123456789012:root"] },
                        "Action": ["s3vectors:GetVectors", "s3vectors:ListVectors"],
                        "Resource": ["*"],
                    },
                    {
                        "Sid": "DenyWrites",
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "s3vectors:PutVectors",
                        "Resource": "*",
                    },
                ],
            })
            .to_string(),
        ),
        (
            "NotAction in place of Action, with a Condition block",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "NotAction": "s3vectors:GetVectors",
                    "Resource": "*",
                    "Condition": { "StringNotEquals": { "aws:PrincipalAccount": "123456789012" } },
                }],
            })
            .to_string(),
        ),
        (
            "non-ASCII Sid text",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "zugriff-fur-prufung-\u{30c6}\u{30b9}\u{30c8}",
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3vectors:GetVectorBucket",
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
        (
            "pretty-printed document with newlines and indentation",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \
             \"Effect\": \"Allow\",\n      \"Principal\": \"*\",\n      \
             \"Action\": \"s3vectors:QueryVectors\",\n      \"Resource\": \"*\"\n    }\n  ]\n}"
                .to_string(),
        ),
        (
            "a document exactly at the size ceiling",
            policy_of_size(MAX_POLICY_BYTES),
        ),
        (
            "a document one byte under the size ceiling",
            policy_of_size(MAX_POLICY_BYTES - 1),
        ),
    ]
}

/// Every accepted policy document round-trips verbatim through put and get, and the
/// bucket keeps exactly the last one put.
#[tokio::test]
async fn every_accepted_policy_document_round_trips_verbatim() {
    let router = with_bucket("policy-roundtrip").await;

    for (case, policy) in accepted_policies() {
        put_policy(&router, "policy-roundtrip", &policy).await;
        let reported = get_policy(&router, "policy-roundtrip").await;
        assert_eq!(
            reported, policy,
            "case {case:?}: the policy must be reported exactly as it was put, got {reported:?}"
        );
    }
}

/// A put REPLACES the stored policy rather than merging into it: after the second
/// put nothing of the first one is left.
#[tokio::test]
async fn a_second_put_replaces_the_policy_it_does_not_merge() {
    let router = with_bucket("policy-overwrite").await;

    let first = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "FirstOnly",
            "Effect": "Allow",
            "Principal": "*",
            "Action": "s3vectors:GetVectors",
            "Resource": "*",
        }],
    })
    .to_string();
    let second = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "SecondOnly",
            "Effect": "Deny",
            "Principal": "*",
            "Action": "s3vectors:ListVectors",
            "Resource": "*",
        }],
    })
    .to_string();

    put_policy(&router, "policy-overwrite", &first).await;
    put_policy(&router, "policy-overwrite", &second).await;

    let reported = get_policy(&router, "policy-overwrite").await;
    assert_eq!(
        reported, second,
        "the last put policy is the one a get reports, got {reported:?}"
    );
    assert!(
        !reported.contains("FirstOnly"),
        "nothing of the replaced policy may survive, got {reported:?}"
    );

    // And the replacement is the whole document: the statement count does not grow.
    let parsed: Value = serde_json::from_str(&reported).expect("the reported policy is JSON");
    assert_eq!(
        parsed["Statement"].as_array().map(Vec::len),
        Some(1),
        "a replaced policy holds only the new statements, got {reported:?}"
    );
}

/// The policy lifecycle: absent -> put -> get -> delete -> absent, with the get of
/// an absent policy answering the modelled 404 both before the first put and after
/// the delete, and the delete idempotent.
#[tokio::test]
async fn the_policy_lifecycle_runs_from_absent_through_put_to_deleted() {
    let router = with_bucket("policy-lifecycle").await;
    let request = json!({ "vectorBucketName": "policy-lifecycle" });

    for case in ["before any put", "after the delete"] {
        if case == "after the delete" {
            let policy = accepted_policies()[0].1.clone();
            put_policy(&router, "policy-lifecycle", &policy).await;
            let reported = get_policy(&router, "policy-lifecycle").await;
            assert_eq!(
                reported, policy,
                "case {case:?}: the put policy must be readable, got {reported:?}"
            );
            ok(&router, "DeleteVectorBucketPolicy", &request).await;
        }

        let (status, body) = call(&router, "GetVectorBucketPolicy", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "NotFoundException"),
            "case {case:?}: a bucket with no policy has no policy to get, got {status} {body}"
        );
    }

    // DeleteVectorBucketPolicy is `@idempotent`: deleting a policy that is already
    // gone leaves the same state and answers the same way.
    ok(&router, "DeleteVectorBucketPolicy", &request).await;
    let (status, body) = call(&router, "GetVectorBucketPolicy", &request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "NotFoundException"),
        "a repeated delete leaves the policy absent, got {status} {body}"
    );

    // A bucket whose policy was deleted still takes a new one.
    let again = accepted_policies()[2].1.clone();
    put_policy(&router, "policy-lifecycle", &again).await;
    let reported = get_policy(&router, "policy-lifecycle").await;
    assert_eq!(
        reported, again,
        "a policy put after a delete must be readable, got {reported:?}"
    );

    // The policy is a property of the bucket, not of the service: the bucket itself
    // still reads back unchanged.
    let bucket = ok(&router, "GetVectorBucket", &request).await;
    assert_eq!(
        bucket["vectorBucket"]["vectorBucketName"].as_str(),
        Some("policy-lifecycle"),
        "the bucket must still read back, got {bucket}"
    );
}

/// Documents that are not policy documents by the IAM grammar are refused as
/// invalid input and NOT stored: a get still answers 404 afterwards.
///
/// The rows walk the grammar's requirements — well-formed JSON, a top-level object,
/// a string `Version`/`Id`, a `Statement` of at least one statement object, an
/// `Effect` of exactly `Allow` or `Deny`, and an `Action` or `NotAction` — plus the
/// size ceiling, tested one byte past it.
#[tokio::test]
async fn documents_outside_the_policy_grammar_are_refused_and_not_stored() {
    let router = with_bucket("policy-refusals").await;

    let cases: Vec<(&str, String)> = vec![
        ("not JSON at all", "this is not a policy".to_string()),
        ("truncated JSON", "{\"Statement\":".to_string()),
        ("empty document", String::new()),
        ("trailing content after the object", "{} {}".to_string()),
        ("a JSON array at the top level", "[]".to_string()),
        ("a JSON string at the top level", "\"Allow\"".to_string()),
        ("a JSON number at the top level", "7".to_string()),
        ("JSON null", "null".to_string()),
        (
            "no Statement member",
            json!({ "Version": "2012-10-17" }).to_string(),
        ),
        (
            "an empty Statement array",
            json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
        ),
        (
            "a statement that is not an object",
            json!({ "Statement": ["s3vectors:*"] }).to_string(),
        ),
        (
            "a statement with no Effect",
            json!({ "Statement": [{ "Action": "s3vectors:*", "Resource": "*" }] }).to_string(),
        ),
        (
            "an Effect outside Allow and Deny",
            json!({ "Statement": [{ "Effect": "allow", "Action": "s3vectors:*" }] }).to_string(),
        ),
        (
            "an Effect that is not a string",
            json!({ "Statement": [{ "Effect": true, "Action": "s3vectors:*" }] }).to_string(),
        ),
        (
            "a statement with neither Action nor NotAction",
            json!({ "Statement": [{ "Effect": "Allow", "Resource": "*" }] }).to_string(),
        ),
        (
            "a Version that is not a string",
            json!({ "Version": 2012, "Statement": [{ "Effect": "Allow", "Action": "s3vectors:*" }] })
                .to_string(),
        ),
        (
            "an Id that is not a string",
            json!({ "Id": [], "Statement": [{ "Effect": "Allow", "Action": "s3vectors:*" }] })
                .to_string(),
        ),
        (
            "one byte past the size ceiling",
            policy_of_size(MAX_POLICY_BYTES + 1),
        ),
    ];

    for (case, policy) in cases {
        let (status, body) = call(
            &router,
            "PutVectorBucketPolicy",
            &json!({ "vectorBucketName": "policy-refusals", "policy": policy }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a document outside the policy grammar must be refused, \
             got {status} {body}"
        );

        let (status, body) = call(
            &router,
            "GetVectorBucketPolicy",
            &json!({ "vectorBucketName": "policy-refusals" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "NotFoundException"),
            "case {case:?}: a refused put must store nothing, got {status} {body}"
        );
    }
}

/// Each bucket carries its own policy, and a bucket's policy goes with the bucket.
#[tokio::test]
async fn a_policy_belongs_to_exactly_one_bucket() {
    let router = router();
    for bucket in ["policy-bucket-one", "policy-bucket-two", "policy-bucket-gone"] {
        ok(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
    }

    let policies = accepted_policies();
    let (_, one) = policies[0].clone();
    let (_, two) = policies[3].clone();
    put_policy(&router, "policy-bucket-one", &one).await;
    put_policy(&router, "policy-bucket-two", &two).await;
    put_policy(&router, "policy-bucket-gone", &one).await;

    for (case, bucket, expected) in [
        ("first bucket", "policy-bucket-one", &one),
        ("second bucket", "policy-bucket-two", &two),
    ] {
        let reported = get_policy(&router, bucket).await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: each bucket reports its own policy, got {reported:?}"
        );
    }

    // Deleting one bucket's policy leaves the other's alone.
    ok(
        &router,
        "DeleteVectorBucketPolicy",
        &json!({ "vectorBucketName": "policy-bucket-one" }),
    )
    .await;
    let reported = get_policy(&router, "policy-bucket-two").await;
    assert_eq!(
        &reported, &two,
        "deleting one bucket's policy must not touch another's, got {reported:?}"
    );

    // And a policy does not outlive its bucket: a bucket re-created under the same
    // name carries no policy.
    ok(
        &router,
        "DeleteVectorBucket",
        &json!({ "vectorBucketName": "policy-bucket-gone" }),
    )
    .await;
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "policy-bucket-gone" }),
    )
    .await;
    let (status, body) = call(
        &router,
        "GetVectorBucketPolicy",
        &json!({ "vectorBucketName": "policy-bucket-gone" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "NotFoundException"),
        "a re-created bucket carries no policy, got {status} {body}"
    );
}

/// The bucket a policy operation acts on may be named by `vectorBucketName` or by
/// `vectorBucketArn`, and the two forms reach the same bucket.
#[tokio::test]
async fn a_policy_operation_takes_either_accepted_bucket_form() {
    let router = with_bucket("policy-arn-form").await;
    let arn = expected_bucket_arn("policy-arn-form");
    let policy = accepted_policies()[0].1.clone();

    // Put by ARN, read back by name.
    ok(
        &router,
        "PutVectorBucketPolicy",
        &json!({ "vectorBucketArn": arn, "policy": policy }),
    )
    .await;
    let reported = get_policy(&router, "policy-arn-form").await;
    assert_eq!(
        reported, policy,
        "a policy put by ARN must be readable by name, got {reported:?}"
    );

    // Read back by ARN too.
    let body = ok(
        &router,
        "GetVectorBucketPolicy",
        &json!({ "vectorBucketArn": arn }),
    )
    .await;
    assert_eq!(
        body["policy"].as_str(),
        Some(policy.as_str()),
        "a get by ARN must report the same policy, got {body}"
    );

    // Delete by ARN removes the same policy.
    ok(
        &router,
        "DeleteVectorBucketPolicy",
        &json!({ "vectorBucketArn": arn }),
    )
    .await;
    let (status, body) = call(
        &router,
        "GetVectorBucketPolicy",
        &json!({ "vectorBucketName": "policy-arn-form" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "NotFoundException"),
        "a delete by ARN removes the policy the name reads, got {status} {body}"
    );
}

/// A policy operation that cannot resolve the bucket it names is refused, and a
/// policy operation on a bucket that does not exist is the modelled 404.
#[tokio::test]
async fn a_policy_operation_refuses_a_bucket_it_cannot_resolve() {
    let router = with_bucket("policy-targets").await;
    let policy = accepted_policies()[0].1.clone();

    // (case, operation, request, status, error code)
    let cases: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "put names no bucket",
            "PutVectorBucketPolicy",
            json!({ "policy": policy }),
            400,
            "ValidationException",
        ),
        (
            "put names a name and an ARN that disagree",
            "PutVectorBucketPolicy",
            json!({ "vectorBucketName": "policy-targets",
                    "vectorBucketArn": expected_bucket_arn("other-bucket"),
                    "policy": policy }),
            400,
            "ValidationException",
        ),
        (
            "put names a bucket that does not exist",
            "PutVectorBucketPolicy",
            json!({ "vectorBucketName": "policy-absent", "policy": policy }),
            404,
            "NotFoundException",
        ),
        (
            "get names no bucket",
            "GetVectorBucketPolicy",
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "get names a bucket that does not exist",
            "GetVectorBucketPolicy",
            json!({ "vectorBucketName": "policy-absent" }),
            404,
            "NotFoundException",
        ),
        (
            "delete names no bucket",
            "DeleteVectorBucketPolicy",
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "delete names an index ARN where a bucket ARN belongs",
            "DeleteVectorBucketPolicy",
            json!({ "vectorBucketArn":
                    format!("{}/index/i", expected_bucket_arn("policy-targets")) }),
            400,
            "ValidationException",
        ),
        (
            "delete names a bucket that does not exist",
            "DeleteVectorBucketPolicy",
            json!({ "vectorBucketName": "policy-absent" }),
            404,
            "NotFoundException",
        ),
    ];

    for (case, operation, request, status, code) in cases {
        let (actual_status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (actual_status, error_code(&body).as_str()),
            (status, code),
            "case {case:?}: {operation} must answer {status} {code}, got {actual_status} {body}"
        );
    }
}

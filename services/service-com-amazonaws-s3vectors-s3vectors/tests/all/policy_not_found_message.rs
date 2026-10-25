// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what the vector-bucket-policy operations report when the
//! POLICY is absent, served over the real `restJson1` wire through the router
//! production uses.
//!
//! The rule under test: a vector bucket holds at most one policy, deleting it is
//! idempotent (a bucket that carries no policy is left alone and answered as a
//! success, however often the delete is repeated), and a read of an absent policy
//! is refused 404 `NotFoundException` whose message is ONE fixed sentence per
//! missing resource kind — carrying no value from the request, so the sentence for
//! a bucket that does not exist and the sentence for a bucket that holds no policy
//! are different and a client can tell the two apart.
//!
//! Every row states the case it covers and every assertion reports the actual
//! value. The rows differ in the bucket name (its length and its characters), in
//! which input form names the bucket (`vectorBucketName`, `vectorBucketArn`, or
//! both), and in how the policy came to be absent (never put, put then deleted,
//! deleted again).

use serde_json::{json, Value};
use service_com_amazonaws_s3vectors_s3vectors::state::{BUCKET_NOT_FOUND, POLICY_NOT_FOUND};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{call, error_code, expected_bucket_arn, ok, router};

/// The `message` member of an error body.
fn message_of(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a refusal must carry a message, got {body}"))
        .to_string()
}

/// How one row names the bucket a policy operation acts on.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// Only `vectorBucketName`.
    Name,
    /// Only `vectorBucketArn`.
    Arn,
    /// Both members, naming the same bucket.
    Both,
}

impl Target {
    /// The request members this form contributes for `bucket`.
    fn request(self, bucket: &str) -> Value {
        match self {
            Self::Name => json!({ "vectorBucketName": bucket }),
            Self::Arn => json!({ "vectorBucketArn": expected_bucket_arn(bucket) }),
            Self::Both => json!({
                "vectorBucketName": bucket,
                "vectorBucketArn": expected_bucket_arn(bucket),
            }),
        }
    }
}

/// A policy document every row stores before deleting it again — a policy document
/// by the IAM JSON policy grammar.
fn a_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
            "Action": "s3vectors:GetVectors",
            "Resource": "*",
        }],
    })
    .to_string()
}

/// A 63-character bucket name, the top of `VectorBucketName`'s modelled length
/// range (`len 3..63`).
fn longest_bucket_name() -> String {
    format!("p{}z", "0".repeat(61))
}

/// The bucket names the table spans: the bottom of the modelled length range, the
/// top of it, a name carrying both punctuation characters the name grammar allows,
/// and a name of the opaque generated form a client tends to use.
fn bucket_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest accepted name", "abc".to_string()),
        ("the longest accepted name", longest_bucket_name()),
        (
            "a name carrying dots and hyphens",
            "policy.msg-bucket.v2".to_string(),
        ),
        (
            "an opaque generated name",
            "s3v-9f41c2ab7de04c11b3e8".to_string(),
        ),
    ]
}

/// Assert a policy read of `bucket` is the modelled 404 carrying the fixed
/// absent-policy sentence, naming `case` on every failure.
async fn assert_policy_absent(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    target: Target,
) {
    let (status, body) = call(router, "GetVectorBucketPolicy", &target.request(bucket)).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "NotFoundException"),
        "case {case:?}: an absent policy must be the modelled 404, got {status} {body}"
    );
    let message = message_of(&body);
    assert_eq!(
        message, POLICY_NOT_FOUND,
        "case {case:?}: an absent policy must report the fixed absent-policy \
         sentence, got {message:?}"
    );
    assert!(
        !message.contains(bucket),
        "case {case:?}: the sentence must carry no value from the request, got {message:?}"
    );
}

/// Across every bucket name and every accepted input form, a policy that was never
/// put, one that was deleted, and one deleted twice all read back as the same fixed
/// refusal, and every delete of an absent policy is a success.
#[tokio::test]
async fn an_absent_policy_reports_one_fixed_sentence_whatever_the_request_names() {
    for (name_case, bucket) in bucket_names() {
        for target in [Target::Name, Target::Arn, Target::Both] {
            let router = router();
            ok(
                &router,
                "CreateVectorBucket",
                &json!({ "vectorBucketName": bucket }),
            )
            .await;
            let request = target.request(&bucket);

            // Never put.
            let case = format!("{name_case} by {target:?}: never put");
            assert_policy_absent(&router, &case, &bucket, target).await;

            // Deleting a policy that is not there is a success that changes nothing.
            ok(&router, "DeleteVectorBucketPolicy", &request).await;
            let case = format!("{name_case} by {target:?}: deleted while absent");
            assert_policy_absent(&router, &case, &bucket, target).await;

            // Put, then deleted: the same sentence, so a policy having existed
            // leaves no trace in the refusal.
            let policy = a_policy();
            ok(
                &router,
                "PutVectorBucketPolicy",
                &json!({ "vectorBucketName": bucket, "policy": policy }),
            )
            .await;
            let body = ok(&router, "GetVectorBucketPolicy", &request).await;
            assert_eq!(
                body["policy"].as_str(),
                Some(policy.as_str()),
                "case {name_case:?} by {target:?}: the put policy must read back, got {body}"
            );
            ok(&router, "DeleteVectorBucketPolicy", &request).await;
            let case = format!("{name_case} by {target:?}: put then deleted");
            assert_policy_absent(&router, &case, &bucket, target).await;

            // And repeating the delete is idempotent, for any number of repeats.
            for repeat in 1..=3 {
                ok(&router, "DeleteVectorBucketPolicy", &request).await;
                let case = format!("{name_case} by {target:?}: delete repeated {repeat} time(s)");
                assert_policy_absent(&router, &case, &bucket, target).await;
            }
        }
    }
}

/// A bucket that does not exist and a bucket that holds no policy are DIFFERENT
/// missing resources: both are 404 `NotFoundException`, and each carries its own
/// kind's sentence, on every policy operation.
#[tokio::test]
async fn a_missing_bucket_and_a_missing_policy_report_different_sentences() {
    assert_ne!(
        BUCKET_NOT_FOUND, POLICY_NOT_FOUND,
        "the two missing-resource kinds must be distinguishable by their sentence"
    );

    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "policy-kinds" }),
    )
    .await;

    // (case, operation, bucket named, extra members, the sentence expected)
    let cases: Vec<(&str, &str, &str, Value, &str)> = vec![
        (
            "get on a bucket that does not exist",
            "GetVectorBucketPolicy",
            "policy-kinds-absent",
            json!({}),
            BUCKET_NOT_FOUND,
        ),
        (
            "delete on a bucket that does not exist",
            "DeleteVectorBucketPolicy",
            "policy-kinds-absent",
            json!({}),
            BUCKET_NOT_FOUND,
        ),
        (
            "put on a bucket that does not exist",
            "PutVectorBucketPolicy",
            "policy-kinds-absent",
            json!({ "policy": a_policy() }),
            BUCKET_NOT_FOUND,
        ),
        (
            "get on a bucket that exists and holds no policy",
            "GetVectorBucketPolicy",
            "policy-kinds",
            json!({}),
            POLICY_NOT_FOUND,
        ),
    ];

    for (case, operation, bucket, extra, expected) in cases {
        let mut request = json!({ "vectorBucketName": bucket });
        for (key, value) in extra.as_object().expect("the extra members are an object") {
            request[key] = value.clone();
        }
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "NotFoundException"),
            "case {case:?}: {operation} must answer the modelled 404, got {status} {body}"
        );
        let message = message_of(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: {operation} must report this kind's sentence, got {message:?}"
        );
    }

    // Deleting the policy of a bucket that exists is a success whether or not the
    // bucket carries one, which is what makes the two kinds visible: only the
    // missing BUCKET is refused.
    ok(
        &router,
        "DeleteVectorBucketPolicy",
        &json!({ "vectorBucketName": "policy-kinds" }),
    )
    .await;
}

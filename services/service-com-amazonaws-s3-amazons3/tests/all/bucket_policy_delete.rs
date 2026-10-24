// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WITHDRAWING a bucket's policy: `DeleteBucketPolicy`.
//!
//! The rule under test: deleting a bucket subresource clears that subresource from the
//! bucket holding it and answers the operation's no-content status (`204`, which
//! `smithy_query(query="operation", operation="DeleteBucketPolicy")` reports as its
//! `@http` code, with `smithy.api#Unit` output so there is nothing to carry), whether or
//! not the subresource was configured — a subresource DELETE is IDEMPOTENT. After it, the
//! subresource's reader reports it as absent again (`GetBucketPolicy` →
//! `NoSuchBucketPolicy`, 404), and a later write may configure it afresh. The status is a
//! property of the operation, never of the bucket, of the document the bucket held, or of
//! how many times the delete is called.
//!
//! The rows below span what the rule generalises over — the bucket (name form, Region),
//! the document the delete removes (statement count, principal form, whitespace, byte
//! size, from the smallest accepted document up to the published 20 KB ceiling), the
//! number of times the delete runs, and whether a policy was there at all. The recorded
//! case that filed this is one row among them.
//!
//! Refusals keep their own status and code rather than collapsing into the 204: a bucket
//! that is not there is `NoSuchBucket` (404) and a request naming an account other than
//! the bucket's owner is `AccessDenied` (403), per
//! `DeleteBucketPolicyRequest$ExpectedBucketOwner` ("the request fails with the HTTP
//! status code 403 Forbidden").
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour.

use service_com_amazonaws_s3_amazons3::state::MAX_BUCKET_POLICY_BYTES;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{
    create_bucket, get_bucket_policy, policy_of_size, put_bucket_policy, router, send, send_bytes,
    Answer,
};

/// `DELETE /<bucket>?policy` — `DeleteBucketPolicy`.
async fn delete_bucket_policy(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "DELETE", &format!("/{bucket}?policy"), &[], "").await
}

/// `DELETE /<bucket>?policy` carrying `x-amz-expected-bucket-owner`.
async fn delete_bucket_policy_as_owner(
    router: &MultiServiceRouter,
    bucket: &str,
    expected_owner: &str,
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{bucket}?policy"),
        &[("x-amz-expected-bucket-owner", expected_owner)],
        "",
    )
    .await
}

/// The policy documents a delete is asserted over: each is attached and then withdrawn.
/// The rows differ in statement count, action and principal form, whitespace and byte
/// size — none of which the rule depends on.
fn documents() -> Vec<(String, String)> {
    let at_ceiling = policy_of_size(MAX_BUCKET_POLICY_BYTES);
    assert_eq!(
        at_ceiling.len(),
        MAX_BUCKET_POLICY_BYTES,
        "case \"a document exactly at the 20 KB ceiling\": the row must be built at the \
         ceiling, got {} bytes",
        at_ceiling.len()
    );
    vec![
        (
            "the smallest accepted document".to_string(),
            "{\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"s3:*\"}]}".to_string(),
        ),
        (
            "three statements, one of them a Deny".to_string(),
            serde_json::json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Principal": "*", "Action": ["s3:GetObject"]},
                    {"Effect": "Allow", "Principal": {"Service": "example.amazonaws.com"},
                     "Action": "s3:PutObject"},
                    {"Effect": "Deny", "Principal": "*", "Action": "s3:DeleteObject"},
                ],
            })
            .to_string(),
        ),
        (
            "a single statement object rather than a list".to_string(),
            serde_json::json!({
                "Statement": {"Effect": "Allow", "Principal": "*", "Action": "s3:GetObject"},
            })
            .to_string(),
        ),
        (
            "a document carrying newlines and tabs".to_string(),
            "{\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\"Principal\": \
             \"*\",\n\t\t\t\"Action\": \"s3:GetObject\"\n\t\t}\n\t]\n}"
                .to_string(),
        ),
        (
            "a document exactly at the 20 KB ceiling".to_string(),
            at_ceiling,
        ),
    ]
}

#[tokio::test]
async fn deleting_a_bucket_policy_answers_204_and_leaves_the_bucket_without_one() {
    let router = router();
    // Several buckets, differing in name length and form; one outside `us-east-1`, so the
    // rows span Regions too. The status is not a property of the bucket.
    let buckets: &[(&str, Option<&str>)] = &[
        ("abc", None),
        ("config-delivery-destination", None),
        ("logs.2026.archive-bucket", Some("eu-west-1")),
    ];
    for (bucket, region) in buckets {
        let created = create_bucket(&router, bucket, *region, None).await;
        assert_eq!(
            created.status, 200,
            "case {bucket:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }

    for (bucket, _) in buckets {
        for (label, document) in &documents() {
            let attached = put_bucket_policy(&router, bucket, document).await;
            assert_eq!(
                attached.status, 204,
                "case ({bucket:?}, {label:?}): the policy must be attached first, got {} \
                 with body {:?}",
                attached.status,
                String::from_utf8_lossy(&attached.body)
            );

            let deleted = delete_bucket_policy(&router, bucket).await;
            assert_eq!(
                deleted.status, 204,
                "case ({bucket:?}, {label:?}): expected HTTP 204, got {} with body {:?}",
                deleted.status,
                String::from_utf8_lossy(&deleted.body)
            );
            assert!(
                deleted.body.is_empty(),
                "case ({bucket:?}, {label:?}): a 204 carries no body, got {:?}",
                String::from_utf8_lossy(&deleted.body)
            );

            // The 204 is a real WRITE: the reader now reports the subresource as absent.
            let read = get_bucket_policy(&router, bucket).await;
            assert_eq!(
                read.status, 404,
                "case ({bucket:?}, {label:?}): GetBucketPolicy after the delete must answer \
                 404, got {} with body {:?}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                read.error_code(),
                "NoSuchBucketPolicy",
                "case ({bucket:?}, {label:?}): expected error code NoSuchBucketPolicy, got {:?}",
                read.error_code()
            );

            // A delete is IDEMPOTENT: repeating it on a bucket that now has no policy is
            // still a bodyless 204, never a 404 and never a 304.
            for repeat in 1..=2 {
                let again = delete_bucket_policy(&router, bucket).await;
                assert_eq!(
                    again.status, 204,
                    "case ({bucket:?}, {label:?}, repeat {repeat}): expected HTTP 204, got {} \
                     with body {:?}",
                    again.status,
                    String::from_utf8_lossy(&again.body)
                );
                assert!(
                    again.body.is_empty(),
                    "case ({bucket:?}, {label:?}, repeat {repeat}): a 204 carries no body, got \
                     {:?}",
                    String::from_utf8_lossy(&again.body)
                );
            }

            // And the subresource can be configured afresh afterwards: the delete cleared
            // the record, it did not retire the bucket's policy slot.
            let reattached = put_bucket_policy(&router, bucket, document).await;
            assert_eq!(
                reattached.status, 204,
                "case ({bucket:?}, {label:?}): a policy must be attachable again after the \
                 delete, got {} with body {:?}",
                reattached.status,
                String::from_utf8_lossy(&reattached.body)
            );
            let reread = get_bucket_policy(&router, bucket).await;
            assert_eq!(
                String::from_utf8_lossy(&reread.body),
                document.as_str(),
                "case ({bucket:?}, {label:?}): the re-attached policy must read back byte for \
                 byte, got {} with body {:?}",
                reread.status,
                String::from_utf8_lossy(&reread.body)
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_policy_a_bucket_never_had_is_still_a_204() {
    let router = router();
    // A fresh bucket has no policy at all, so the FIRST delete is already the
    // "nothing to clear" case the idempotence rule covers.
    let buckets: &[&str] = &["fresh-bucket", "another-fresh-bucket"];
    for bucket in buckets {
        let created = create_bucket(&router, bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {bucket:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let before = get_bucket_policy(&router, bucket).await;
        assert_eq!(
            before.error_code(),
            "NoSuchBucketPolicy",
            "case {bucket:?}: a fresh bucket must carry no policy, got {} with body {:?}",
            before.status,
            String::from_utf8_lossy(&before.body)
        );

        let deleted = delete_bucket_policy(&router, bucket).await;
        assert_eq!(
            deleted.status, 204,
            "case {bucket:?}: expected HTTP 204, got {} with body {:?}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        assert!(
            deleted.body.is_empty(),
            "case {bucket:?}: a 204 carries no body, got {:?}",
            String::from_utf8_lossy(&deleted.body)
        );
    }
}

#[tokio::test]
async fn a_refused_delete_bucket_policy_keeps_its_own_status_and_error_document() {
    let router = router();
    let bucket = "config-delivery-destination";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let policy = r#"{"Statement":[{"Effect":"Allow","Principal":{"Service":"config.amazonaws.com"},"Action":"s3:PutObject"}]}"#;
    let attached = put_bucket_policy(&router, bucket, policy).await;
    assert_eq!(
        attached.status, 204,
        "the policy must be attached first, got {} with body {:?}",
        attached.status,
        String::from_utf8_lossy(&attached.body)
    );

    // A bucket this account does not hold: the delete reports the bucket, not a success.
    for absent in ["no-such-bucket-at-all", "neither-is-this-one"] {
        let answer = delete_bucket_policy(&router, absent).await;
        assert_eq!(
            answer.status, 404,
            "case {absent:?}: expected HTTP 404, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchBucket",
            "case {absent:?}: expected error code NoSuchBucket, got {:?}",
            answer.error_code()
        );
        assert!(
            !answer.body.is_empty(),
            "case {absent:?}: a refusal carries an error document, got an empty body"
        );
    }

    // An expected owner that is not the account serving the bucket: 403 AccessDenied. The
    // rows span a well-formed foreign account id and one of a different length.
    for mismatched in ["123456789012", "0", "99999999999999"] {
        let answer = delete_bucket_policy_as_owner(&router, bucket, mismatched).await;
        assert_eq!(
            answer.status, 403,
            "case {mismatched:?}: expected HTTP 403, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {mismatched:?}: expected error code AccessDenied, got {:?}",
            answer.error_code()
        );
    }

    // None of the refusals touched the store: the policy is still readable verbatim.
    let read = get_bucket_policy(&router, bucket).await;
    assert_eq!(
        String::from_utf8_lossy(&read.body),
        policy,
        "a refused delete must write nothing, got {} with body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );

    // The owner this emulator serves IS accepted, which is what makes the 403 above a
    // statement about the mismatch rather than about the header's presence.
    let as_owner =
        delete_bucket_policy_as_owner(&router, bucket, engine::emulator_account_id()).await;
    assert_eq!(
        as_owner.status, 204,
        "case \"the serving account as expected owner\": expected HTTP 204, got {} with \
         body {:?}",
        as_owner.status,
        String::from_utf8_lossy(&as_owner.body)
    );
    let after = get_bucket_policy(&router, bucket).await;
    assert_eq!(
        after.error_code(),
        "NoSuchBucketPolicy",
        "case \"the serving account as expected owner\": the policy must be gone, got {} with \
         body {:?}",
        after.status,
        String::from_utf8_lossy(&after.body)
    );
}

#[tokio::test]
async fn deleting_the_policy_leaves_the_bucket_and_its_objects_alone() {
    let router = router();
    let bucket = "config-delivery-destination";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let attached = put_bucket_policy(
        &router,
        bucket,
        r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject"}]}"#,
    )
    .await;
    assert_eq!(
        attached.status, 204,
        "the policy must be attached first, got {} with body {:?}",
        attached.status,
        String::from_utf8_lossy(&attached.body)
    );

    // Objects are a different subresource of the same bucket, so withdrawing the policy
    // must leave every one of them exactly as it was.
    let objects: &[(&str, &[u8])] = &[
        ("AWSLogs/report-1.json", b"{\"configurationItems\":[]}"),
        ("AWSLogs/report-2.json.gz", &[0x1f, 0x8b, 0x08, 0x00, 0x00]),
        ("empty", b""),
    ];
    for (key, body) in objects {
        let put = send_bytes(&router, "PUT", &format!("/{bucket}/{key}"), &[], body).await;
        assert_eq!(
            put.status, 200,
            "case {key:?}: the object must be stored, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
    }
    let etags: Vec<Option<String>> = {
        let mut collected = Vec::new();
        for (key, _) in objects {
            let head = send_bytes(&router, "HEAD", &format!("/{bucket}/{key}"), &[], b"").await;
            collected.push(head.header("etag"));
        }
        collected
    };

    let deleted = delete_bucket_policy(&router, bucket).await;
    assert_eq!(
        deleted.status, 204,
        "the delete must answer 204, got {} with body {:?}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    // The bucket itself is still there...
    let head_bucket = send(&router, "HEAD", &format!("/{bucket}"), &[], "").await;
    assert_eq!(
        head_bucket.status, 200,
        "withdrawing the policy must not remove the bucket, got {} with body {:?}",
        head_bucket.status,
        String::from_utf8_lossy(&head_bucket.body)
    );
    // ... and so is every object, byte for byte and with the entity tag it had.
    for ((key, body), etag) in objects.iter().zip(etags) {
        let read = send_bytes(&router, "GET", &format!("/{bucket}/{key}"), &[], b"").await;
        assert_eq!(
            read.body.as_ref(),
            *body,
            "case {key:?}: withdrawing the policy must not touch the object, got {} bytes",
            read.body.len()
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {key:?}: withdrawing the policy must not change the object's ETag, got {:?}",
            read.header("etag")
        );
    }
}

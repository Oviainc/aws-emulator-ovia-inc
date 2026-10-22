// SPDX-License-Identifier: CC0-1.0

//! The STATUS a successful `PutBucketPolicy` answers with.
//!
//! The rule under test: a successful write to a bucket's policy subresource has nothing
//! to return — the operation's modelled output shape is `smithy.api#Unit`, which
//! `smithy_query(query="operation", operation="PutBucketPolicy")` reports with no members
//! — so S3 answers it `204 No Content` with an EMPTY body. The status is a property of
//! the operation, never of the bucket, of the document's content, form or size, or of
//! whether the put creates the bucket's first policy or replaces one that was already
//! there. (S3's API Reference sample response for `PutBucketPolicy` is
//! `HTTP/1.1 204 NoContent`; the same subresource's `DeleteBucketPolicy` carries 204 in
//! the model itself, while `PutBucketPolicy`'s declared `@http(code:)` of 200 understates
//! it.)
//!
//! The rows below span what the rule generalises over — the bucket, the document's
//! statement count, action and principal form, whitespace, byte size (down to the
//! smallest accepted document and up to the published 20 KB ceiling) and the ORDER a
//! bucket sees its puts in (first put, replacement, identical repeat). The recorded case
//! that filed this is one row among them.
//!
//! The 204 is asserted NEXT TO the two statuses it must not swallow: `GetBucketPolicy`,
//! which has an output member to carry, still answers 200 with the document, and a
//! refused put still answers its own 4xx with an error document. So this is a statement
//! about this one no-output write, not a blanket "every policy call is bodyless".
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour.

use service_com_amazonaws_s3_amazons3::state::MAX_BUCKET_POLICY_BYTES;

use super::bucket_lifecycle::{
    create_bucket, get_bucket_policy, policy_of_size, put_bucket_policy, router,
};

/// Accepted policy documents, differing in statement count, action form, principal form,
/// whitespace and byte size. Each is a row of the table the rule is asserted over.
fn accepted_documents() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = vec![
        (
            "one statement, a single action".to_string(),
            serde_json::json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Resource": "arn:aws:s3:::any-bucket/*",
                }],
            })
            .to_string(),
        ),
        (
            "one statement, a wildcard action and a service principal".to_string(),
            serde_json::json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "example.amazonaws.com"},
                    "Action": "s3:*",
                }],
            })
            .to_string(),
        ),
        (
            "three statements, one of them a Deny".to_string(),
            serde_json::json!({
                "Statement": [
                    {"Effect": "Allow", "Principal": "*", "Action": ["s3:GetObject"]},
                    {"Effect": "Allow", "Principal": "*", "Action": "s3:PutObject"},
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
    ];
    // The smallest document this emulator accepts, and one exactly at the published
    // ceiling: the status must not depend on how many bytes the body carries.
    rows.push((
        "the smallest accepted document".to_string(),
        "{\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"s3:*\"}]}".to_string(),
    ));
    let at_ceiling = policy_of_size(MAX_BUCKET_POLICY_BYTES);
    assert_eq!(
        at_ceiling.len(),
        MAX_BUCKET_POLICY_BYTES,
        "case \"a document exactly at the 20 KB ceiling\": the row must be built at the \
         ceiling, got {} bytes",
        at_ceiling.len()
    );
    rows.push((
        "a document exactly at the 20 KB ceiling".to_string(),
        at_ceiling,
    ));
    rows
}

#[tokio::test]
async fn a_successful_put_bucket_policy_answers_204_with_an_empty_body() {
    let router = router();
    // Several buckets, differing in name length and form: the status is not a property of
    // the bucket. One is created outside `us-east-1` so the row set spans Regions too.
    let buckets: &[(&str, Option<&str>)] = &[
        ("abc", None),
        ("harvest-destination", None),
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

    let documents = accepted_documents();
    for (bucket, _) in buckets {
        // Every row is put on this bucket in order, so each bucket sees a FIRST put, then
        // a replacement for every later row, and finally an identical repeat of the last
        // one. The status must be 204 for all three situations.
        for (label, document) in &documents {
            let answer = put_bucket_policy(&router, bucket, document).await;
            assert_eq!(
                answer.status, 204,
                "case ({bucket:?}, {label:?}): expected HTTP 204, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                answer.body.is_empty(),
                "case ({bucket:?}, {label:?}): a 204 carries no body, got {:?}",
                String::from_utf8_lossy(&answer.body)
            );

            // The 204 is a successful WRITE, not a no-op: the read reports the bytes.
            let read = get_bucket_policy(&router, bucket).await;
            assert_eq!(
                read.status, 200,
                "case ({bucket:?}, {label:?}): GetBucketPolicy carries an output member, so \
                 it must answer 200, got {} with body {:?}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                String::from_utf8_lossy(&read.body),
                document.as_str(),
                "case ({bucket:?}, {label:?}): the stored policy must read back byte for byte"
            );
        }

        // An identical repeat of the last row: a put that changes nothing is still a
        // successful 204, never a 200 and never a 304.
        let (label, last) = documents
            .last()
            .expect("the table must carry at least one row");
        let repeat = put_bucket_policy(&router, bucket, last).await;
        assert_eq!(
            repeat.status, 204,
            "case ({bucket:?}, {label:?} put twice): expected HTTP 204, got {} with body {:?}",
            repeat.status,
            String::from_utf8_lossy(&repeat.body)
        );
        assert!(
            repeat.body.is_empty(),
            "case ({bucket:?}, {label:?} put twice): a 204 carries no body, got {:?}",
            String::from_utf8_lossy(&repeat.body)
        );
    }
}

#[tokio::test]
async fn a_refused_put_bucket_policy_keeps_its_own_status_and_error_document() {
    let router = router();
    let bucket = "harvest-destination";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    // The 204 belongs to SUCCESS only. Each row below is a different way for the put to
    // fail, and each must keep its own status and error code rather than being flattened
    // into a bodyless 204.
    let one_past_the_ceiling = policy_of_size(MAX_BUCKET_POLICY_BYTES + 1);
    let valid = "{\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"s3:*\"}]}";
    let refused: &[(&str, &str, &str, u16, &str)] = &[
        (
            "a document that is not JSON",
            bucket,
            "{not json",
            400,
            "MalformedPolicy",
        ),
        (
            "a JSON document that is not a policy",
            bucket,
            "[]",
            400,
            "MalformedPolicy",
        ),
        (
            "a document one byte past the 20 KB ceiling",
            bucket,
            one_past_the_ceiling.as_str(),
            400,
            "MalformedPolicy",
        ),
        (
            "a valid document on a bucket that is not there",
            "no-such-bucket-at-all",
            valid,
            404,
            "NoSuchBucket",
        ),
    ];
    for (label, target, document, status, code) in refused {
        let answer = put_bucket_policy(&router, target, document).await;
        assert_eq!(
            answer.status, *status,
            "case {label:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {label:?}: expected error code {code}, got {:?}",
            answer.error_code()
        );
        assert!(
            !answer.body.is_empty(),
            "case {label:?}: a refusal carries an error document, got an empty body"
        );
    }

    // And nothing a refused put carried reached the store: the bucket still has no policy.
    let read = get_bucket_policy(&router, bucket).await;
    assert_eq!(
        read.error_code(),
        "NoSuchBucketPolicy",
        "a refused put must write nothing, got {} with body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
}

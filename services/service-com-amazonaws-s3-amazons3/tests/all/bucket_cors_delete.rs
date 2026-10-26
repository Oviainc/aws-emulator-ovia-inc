// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WITHDRAWING a bucket's cors configuration: `DeleteBucketCors`.
//!
//! The rule under test: deleting a bucket subresource clears that subresource from the
//! bucket holding it and answers the operation's no-content status (`204`, which
//! `smithy_query(query="operation",operation="DeleteBucketCors")` reports as its `@http`
//! code, with `smithy.api#Unit` output so there is nothing to carry), whether or not the
//! subresource was configured — a subresource DELETE is IDEMPOTENT. After it, the
//! subresource's reader reports it as absent again (`GetBucketCors` →
//! `NoSuchCORSConfiguration`, 404), and a later write may configure it afresh. The status is
//! a property of the operation, never of the bucket, of the document the bucket held, or of
//! how many times the delete is called.
//!
//! The rows below span what the rule generalises over — the configuration the delete removes
//! (which of `CORSRule`'s optional members it names, which values they carry, how many rules
//! and how many entries per list, up to the published ceilings; the table is the
//! round-trip table the put/get suite beside this one owns, so the two cannot drift), the
//! bucket (name form, Region), the number of times the delete runs, and whether a cors
//! configuration was there at all. The recorded case that filed this is one row among them.
//!
//! Refusals keep their own status and code rather than collapsing into the 204: a bucket
//! that is not there is `NoSuchBucket` (404) and a request naming an account other than the
//! bucket's owner is `AccessDenied` (403), per
//! `DeleteBucketCorsRequest$ExpectedBucketOwner` ("the request fails with the HTTP status
//! code 403 Forbidden (access denied)").
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_cors::{get_cors, payload, put_cors, round_trip_rows, simple_rule};
use super::bucket_lifecycle::{
    create_bucket, get_bucket_policy, put_bucket_policy, router, send, send_bytes, Answer,
};
use super::bucket_website::{content, reported};

/// `DELETE /<bucket>?cors` — `DeleteBucketCors`.
async fn delete_cors(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "DELETE", &format!("/{bucket}?cors"), &[], "").await
}

/// `DELETE /<bucket>?cors` carrying `x-amz-expected-bucket-owner`.
async fn delete_cors_as_owner(
    router: &MultiServiceRouter,
    bucket: &str,
    expected_owner: &str,
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{bucket}?cors"),
        &[("x-amz-expected-bucket-owner", expected_owner)],
        "",
    )
    .await
}

/// The configuration a row re-attaches after the delete, and the one the refusal rows leave
/// in place.
fn held() -> String {
    simple_rule("HEAD", "https://replacement.example.com")
}

/// The buckets the delete is asserted over: differing in name length and form, one of them
/// outside `us-east-1`, because the rule is a property of the operation rather than of the
/// bucket.
const BUCKETS: &[(&str, Option<&str>)] = &[
    ("abc", None),
    ("cors-delete-destination", None),
    ("logs.2026.archive-bucket", Some("eu-west-1")),
];

#[tokio::test]
async fn deleting_a_cors_configuration_answers_204_and_leaves_the_bucket_without_one() {
    let router = router();
    for (bucket, region) in BUCKETS {
        let created = create_bucket(&router, bucket, *region, None).await;
        assert_eq!(
            created.status, 200,
            "case {bucket:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }

    let configurations = round_trip_rows();
    for (bucket, _) in BUCKETS {
        for (label, configuration) in &configurations {
            let attached = put_cors(&router, bucket, configuration).await;
            assert_eq!(
                attached.status, 200,
                "case ({bucket:?}, {label:?}): the cors configuration must be attached \
                 first, got {} with body {:?}",
                attached.status,
                String::from_utf8_lossy(&attached.body)
            );

            let deleted = delete_cors(&router, bucket).await;
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
            let read = get_cors(&router, bucket).await;
            assert_eq!(
                read.status, 404,
                "case ({bucket:?}, {label:?}): GetBucketCors after the delete must answer \
                 404, got {} with body {:?}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                read.error_code(),
                "NoSuchCORSConfiguration",
                "case ({bucket:?}, {label:?}): expected error code NoSuchCORSConfiguration, \
                 got {:?}",
                read.error_code()
            );

            // A delete is IDEMPOTENT: repeating it on a bucket that now has no cors
            // configuration is still a bodyless 204, never a 404 and never a 304.
            for repeat in 1..=2 {
                let again = delete_cors(&router, bucket).await;
                assert_eq!(
                    again.status, 204,
                    "case ({bucket:?}, {label:?}, repeat {repeat}): expected HTTP 204, got {} \
                     with body {:?}",
                    again.status,
                    String::from_utf8_lossy(&again.body)
                );
                assert!(
                    again.body.is_empty(),
                    "case ({bucket:?}, {label:?}, repeat {repeat}): a 204 carries no body, \
                     got {:?}",
                    String::from_utf8_lossy(&again.body)
                );
            }

            // And the subresource can be configured afresh afterwards: the delete cleared
            // the record, it did not retire the bucket's cors slot.
            let reattached = put_cors(&router, bucket, &held()).await;
            assert_eq!(
                reattached.status, 200,
                "case ({bucket:?}, {label:?}): a cors configuration must be attachable again \
                 after the delete, got {} with body {:?}",
                reattached.status,
                String::from_utf8_lossy(&reattached.body)
            );
            let reread = get_cors(&router, bucket).await;
            assert_eq!(
                reported(&reread),
                content(&payload(&held())),
                "case ({bucket:?}, {label:?}): the re-attached configuration must read back \
                 unchanged, got {} with body {:?}",
                reread.status,
                String::from_utf8_lossy(&reread.body)
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_cors_configuration_a_bucket_never_had_is_still_a_204() {
    let router = router();
    // A fresh bucket has no cors configuration at all, so the FIRST delete is already the
    // "nothing to clear" case the idempotence rule covers. One of the two is outside
    // `us-east-1`, so the answer is about the absent subresource rather than one bucket.
    for (bucket, region) in [
        ("cors-delete-fresh", None),
        ("cors-delete-fresh-regional", Some("eu-west-1")),
    ] {
        let created = create_bucket(&router, bucket, region, None).await;
        assert_eq!(
            created.status, 200,
            "case {bucket:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let before = get_cors(&router, bucket).await;
        assert_eq!(
            before.error_code(),
            "NoSuchCORSConfiguration",
            "case {bucket:?}: a fresh bucket must carry no cors configuration, got {} with \
             body {:?}",
            before.status,
            String::from_utf8_lossy(&before.body)
        );

        let deleted = delete_cors(&router, bucket).await;
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

        // Still the absent subresource afterwards, and still configurable.
        let after = get_cors(&router, bucket).await;
        assert_eq!(
            after.error_code(),
            "NoSuchCORSConfiguration",
            "case {bucket:?}: the bucket must still carry no cors configuration, got {} with \
             body {:?}",
            after.status,
            String::from_utf8_lossy(&after.body)
        );
        let put = put_cors(&router, bucket, &held()).await;
        assert_eq!(
            put.status, 200,
            "case {bucket:?}: the bucket must accept a cors configuration after the delete, \
             got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
    }
}

#[tokio::test]
async fn a_refused_delete_bucket_cors_keeps_its_own_status_and_error_document() {
    let router = router();
    let bucket = "cors-delete-refusals";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let attached = put_cors(&router, bucket, &held()).await;
    assert_eq!(
        attached.status, 200,
        "the configuration the refusals must not disturb has to be stored first, got {} \
         with body {:?}",
        attached.status,
        String::from_utf8_lossy(&attached.body)
    );

    // A bucket this account does not hold: the delete reports the bucket, not a success.
    // One name nothing was created under, and one close to the existing bucket's.
    for absent in ["cors-delete-missing", "cors-delete-refusal"] {
        let answer = delete_cors(&router, absent).await;
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
            !answer.error_message().trim().is_empty(),
            "case {absent:?}: a refusal carries an error document, got an empty message in \
             body {:?}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // An expected owner that is not the account serving the bucket: 403 AccessDenied. The
    // rows span a well-formed foreign account id and ones of a different length.
    for mismatched in ["123456789012", "0", "99999999999999"] {
        let answer = delete_cors_as_owner(&router, bucket, mismatched).await;
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

    // None of the refusals touched the store: the configuration is still reported in full.
    let read = get_cors(&router, bucket).await;
    assert_eq!(
        reported(&read),
        content(&payload(&held())),
        "a refused delete must write nothing, got {} with body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );

    // The owner this emulator serves IS accepted, which is what makes the 403 above a
    // statement about the mismatch rather than about the header's presence.
    let as_owner = delete_cors_as_owner(&router, bucket, engine::emulator_account_id()).await;
    assert_eq!(
        as_owner.status, 204,
        "case \"the serving account as expected owner\": expected HTTP 204, got {} with body \
         {:?}",
        as_owner.status,
        String::from_utf8_lossy(&as_owner.body)
    );
    let after = get_cors(&router, bucket).await;
    assert_eq!(
        after.error_code(),
        "NoSuchCORSConfiguration",
        "case \"the serving account as expected owner\": the cors configuration must be gone, \
         got {} with body {:?}",
        after.status,
        String::from_utf8_lossy(&after.body)
    );
}

#[tokio::test]
async fn deleting_the_cors_configuration_leaves_everything_else_alone() {
    let router = router();
    let bucket = "cors-delete-scope";
    let neighbour = "cors-delete-neighbour";
    for name in [bucket, neighbour] {
        let created = create_bucket(&router, name, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let attached = put_cors(&router, name, &held()).await;
        assert_eq!(
            attached.status, 200,
            "case {name:?}: the cors configuration must be attached first, got {} with body \
             {:?}",
            attached.status,
            String::from_utf8_lossy(&attached.body)
        );
    }

    // The bucket's policy is a DIFFERENT subresource of the same bucket, so withdrawing the
    // cors configuration must leave it exactly as it was.
    let policy = r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject"}]}"#;
    let attached = put_bucket_policy(&router, bucket, policy).await;
    assert_eq!(
        attached.status, 204,
        "the policy must be attached first, got {} with body {:?}",
        attached.status,
        String::from_utf8_lossy(&attached.body)
    );

    // Objects live under the same bucket and must survive the delete byte for byte.
    let objects: &[(&str, &[u8])] = &[
        ("assets/app.js", b"console.log(1)"),
        ("assets/logo.png.gz", &[0x1f, 0x8b, 0x08, 0x00, 0x00]),
        ("empty", b""),
    ];
    let mut etags: Vec<Option<String>> = Vec::new();
    for (key, body) in objects {
        let put = send_bytes(&router, "PUT", &format!("/{bucket}/{key}"), &[], body).await;
        assert_eq!(
            put.status, 200,
            "case {key:?}: the object must be stored, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let head = send_bytes(&router, "HEAD", &format!("/{bucket}/{key}"), &[], b"").await;
        etags.push(head.header("etag"));
    }

    let deleted = delete_cors(&router, bucket).await;
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
        "withdrawing the cors configuration must not remove the bucket, got {} with body {:?}",
        head_bucket.status,
        String::from_utf8_lossy(&head_bucket.body)
    );
    // ... its policy is untouched ...
    let read_policy = get_bucket_policy(&router, bucket).await;
    assert_eq!(
        String::from_utf8_lossy(&read_policy.body),
        policy,
        "withdrawing the cors configuration must not touch the bucket policy, got {} with \
         body {:?}",
        read_policy.status,
        String::from_utf8_lossy(&read_policy.body)
    );
    // ... every object is still there, byte for byte and with the entity tag it had ...
    for ((key, body), etag) in objects.iter().zip(etags) {
        let read = send_bytes(&router, "GET", &format!("/{bucket}/{key}"), &[], b"").await;
        assert_eq!(
            read.body.as_ref(),
            *body,
            "case {key:?}: withdrawing the cors configuration must not touch the object, got \
             {} bytes",
            read.body.len()
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {key:?}: withdrawing the cors configuration must not change the object's \
             ETag, got {:?}",
            read.header("etag")
        );
    }
    // ... and the neighbouring bucket keeps the cors configuration of its own.
    let neighbours = get_cors(&router, neighbour).await;
    assert_eq!(
        reported(&neighbours),
        content(&payload(&held())),
        "case {neighbour:?}: one bucket's delete must leave another bucket's cors \
         configuration in place, got {} with body {:?}",
        neighbours.status,
        String::from_utf8_lossy(&neighbours.body)
    );
}

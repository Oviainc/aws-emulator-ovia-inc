// SPDX-License-Identifier: CC0-1.0

//! A refusal that reports "this bucket has no such subresource" carries the FIXED text
//! the service publishes for its error code.
//!
//! The rule under test: where a condition's wording takes nothing from the request, the
//! `<Message>` S3 sends for that error code is a per-code constant — the text AWS's
//! "Error responses" list of error codes publishes beside the code — not a paraphrase of
//! the condition written at the handler that answers it. A compatibility client branches
//! on `Code` and renders `Message`, so a second wording for one code is a divergence even
//! though the code and status are right.
//!
//! The table spans what the rule generalises over: the bucket SUBRESOURCE being read
//! (tag set, policy, cors, website, replication, ownership controls, public access block),
//! and — for the tag set — how the bucket came to have none (never tagged, the tag set
//! deleted, the tag set replaced by an empty one) and which bucket is being read while
//! another bucket holds tags. One row reads a bucket this emulator does not hold at all,
//! whose condition is about the BUCKET, so it must answer that code's own constant
//! instead.
//!
//! Every expected message below is the text published for its code in AWS's S3 "Error
//! responses" list of error codes
//! (<https://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html#ErrorCodeList>),
//! written out here rather than read from the crate so that a change to the handler's
//! wording fails this suite.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_tagging::{get_bucket_tagging, put_bucket_tagging, tag, tag_set_of};

/// One row: which bucket's which subresource is read, and the status, wire code and
/// `<Message>` that read must answer with.
struct Row {
    case: &'static str,
    bucket: &'static str,
    /// The subresource query string, as it appears after `?` in the request URI.
    subresource: &'static str,
    status: u16,
    code: &'static str,
    message: &'static str,
}

/// `GET /<bucket>?<subresource>` — whichever bucket-subresource reader that URI routes to.
async fn read_subresource(
    router: &MultiServiceRouter,
    bucket: &str,
    subresource: &str,
) -> Answer {
    send(router, "GET", &format!("/{bucket}?{subresource}"), &[], "").await
}

/// `DELETE /<bucket>?tagging` — `DeleteBucketTagging`, one of the two ways below a bucket
/// ends up holding no tag set after having held one.
async fn delete_bucket_tagging(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "DELETE", &format!("/{bucket}?tagging"), &[], "").await
}

#[tokio::test]
async fn an_absent_subresource_is_refused_with_its_codes_published_message() {
    let router = router();

    // Five buckets: one that has nothing set on it at all, two that held a tag set and no
    // longer do, one that holds one throughout (so the refusals below are about the bucket
    // being read, not about the store being empty), and one whose name is long and
    // hyphen-heavy — nothing of which may reach any message.
    for bucket in [
        "subresource-absent",
        "subresource-tagging-deleted",
        "subresource-tagging-emptied",
        "subresource-tagging-neighbour",
        "a-rather-long-bucket-name-whose-text-no-message-may-carry",
    ] {
        let created = create_bucket(&router, bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {bucket:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }

    // Deliberately distinctive tag text, so a message that interpolated any of it would
    // be caught rather than matching an English word a published message happens to use.
    let stored = vec![
        tag("tagkey-alpha", "tagvalue-beta"),
        tag("tagkey-gamma", "tagvalue-delta"),
    ];

    // The tag set this bucket keeps: every refusal below must be unaffected by it.
    let neighbour = put_bucket_tagging(&router, "subresource-tagging-neighbour", &stored).await;
    assert_eq!(
        neighbour.status, 200,
        "the neighbouring bucket must accept a tag set, got {} with body {:?}",
        neighbour.status,
        String::from_utf8_lossy(&neighbour.body)
    );

    // Way two to hold no tag set: one was stored and then deleted.
    let put = put_bucket_tagging(&router, "subresource-tagging-deleted", &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    let deleted = delete_bucket_tagging(&router, "subresource-tagging-deleted").await;
    assert_eq!(
        deleted.status, 204,
        "the setup delete must succeed, got {} with body {:?}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    // Way three: one was stored and then replaced by an empty tag set.
    let put = put_bucket_tagging(&router, "subresource-tagging-emptied", &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    let emptied = put_bucket_tagging(&router, "subresource-tagging-emptied", &[]).await;
    assert_eq!(
        emptied.status, 200,
        "the setup put of an empty tag set must succeed, got {} with body {:?}",
        emptied.status,
        String::from_utf8_lossy(&emptied.body)
    );

    let rows = [
        Row {
            case: "a bucket that was never tagged",
            bucket: "subresource-absent",
            subresource: "tagging",
            status: 404,
            code: "NoSuchTagSet",
            message: "The TagSet does not exist",
        },
        Row {
            case: "a bucket whose tag set was deleted",
            bucket: "subresource-tagging-deleted",
            subresource: "tagging",
            status: 404,
            code: "NoSuchTagSet",
            message: "The TagSet does not exist",
        },
        Row {
            case: "a bucket whose tag set was replaced by an empty one",
            bucket: "subresource-tagging-emptied",
            subresource: "tagging",
            status: 404,
            code: "NoSuchTagSet",
            message: "The TagSet does not exist",
        },
        Row {
            case: "an untagged bucket with a long name, beside a bucket that holds tags",
            bucket: "a-rather-long-bucket-name-whose-text-no-message-may-carry",
            subresource: "tagging",
            status: 404,
            code: "NoSuchTagSet",
            message: "The TagSet does not exist",
        },
        Row {
            case: "a bucket with no policy",
            bucket: "subresource-absent",
            subresource: "policy",
            status: 404,
            code: "NoSuchBucketPolicy",
            message: "The bucket policy does not exist",
        },
        Row {
            case: "a bucket with no cors configuration",
            bucket: "subresource-absent",
            subresource: "cors",
            status: 404,
            code: "NoSuchCORSConfiguration",
            message: "The CORS configuration does not exist",
        },
        Row {
            case: "a bucket with no website configuration",
            bucket: "subresource-absent",
            subresource: "website",
            status: 404,
            code: "NoSuchWebsiteConfiguration",
            message: "The specified bucket does not have a website configuration",
        },
        Row {
            case: "a bucket with no replication configuration",
            bucket: "subresource-absent",
            subresource: "replication",
            status: 404,
            code: "ReplicationConfigurationNotFoundError",
            message: "There is no replication configuration for this bucket",
        },
        Row {
            case: "a bucket with no ownership controls",
            bucket: "subresource-absent",
            subresource: "ownershipControls",
            status: 404,
            code: "OwnershipControlsNotFoundError",
            message: "The bucket ownership controls were not found",
        },
        Row {
            case: "a bucket with no public access block configuration",
            bucket: "subresource-absent",
            subresource: "publicAccessBlock",
            status: 404,
            code: "NoSuchPublicAccessBlockConfiguration",
            message: "The public access block configuration was not found",
        },
        Row {
            case: "a tag set read on a bucket this emulator does not hold",
            bucket: "subresource-never-created",
            subresource: "tagging",
            status: 404,
            code: "NoSuchBucket",
            message: "The specified bucket does not exist",
        },
    ];

    for row in &rows {
        let Row {
            case,
            bucket,
            subresource,
            status,
            code,
            message,
        } = row;
        let read = read_subresource(&router, bucket, subresource).await;
        assert_eq!(
            read.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.error_code(),
            *code,
            "case {case:?}: expected error code {code:?}, got {:?}",
            read.error_code()
        );
        assert_eq!(
            read.error_message(),
            *message,
            "case {case:?}: expected the published message {message:?}, got {:?}",
            read.error_message()
        );

        // The message is a constant for the condition: no value the request carried (the
        // bucket it named) and nothing of what any bucket holds (a tag key or value) is
        // interpolated into it. The subresource is deliberately not checked for: the
        // published text for several of these codes names the subresource itself.
        for fragment in [
            *bucket,
            "tagkey-alpha",
            "tagvalue-beta",
            "tagkey-gamma",
            "tagvalue-delta",
        ] {
            assert!(
                !read.error_message().contains(fragment),
                "case {case:?}: the message must carry nothing from the request or the \
                 store, but {fragment:?} is in {:?}",
                read.error_message()
            );
        }

        // Reading again answers the same way: the refusal reported, it did not configure
        // anything.
        let again = read_subresource(&router, bucket, subresource).await;
        assert_eq!(
            again.error_message(),
            *message,
            "case {case:?}: a second read must answer the same message, got {:?}",
            again.error_message()
        );
    }

    // The bucket that DOES hold a tag set still reports it, so every row above was refused
    // on its absent subresource rather than on a store that lost the tags.
    let held = get_bucket_tagging(&router, "subresource-tagging-neighbour").await;
    assert_eq!(
        held.status, 200,
        "the bucket that holds a tag set must still report it, got {} with body {:?}",
        held.status,
        String::from_utf8_lossy(&held.body)
    );
    assert_eq!(
        tag_set_of(&held),
        stored,
        "the bucket that holds a tag set must report it unchanged, got {:?}",
        tag_set_of(&held)
    );

    // And a bucket whose tag set was refused above takes one afterwards and then reports
    // it, so the refusal left the bucket usable.
    let put = put_bucket_tagging(&router, "subresource-absent", &stored).await;
    assert_eq!(
        put.status, 200,
        "a refused read must leave the bucket tagable, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    let read = get_bucket_tagging(&router, "subresource-absent").await;
    assert_eq!(
        tag_set_of(&read),
        stored,
        "the newly tagged bucket must report its tag set, got {:?}",
        tag_set_of(&read)
    );
}

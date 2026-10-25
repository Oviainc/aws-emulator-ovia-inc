// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the buckets AWS itself publishes: the world-readable sample-data
//! buckets a documented getting-started path reads its training input out of.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, bound headers, body, error code) rather than any handler's
//! internals.
//!
//! The rule under test, asserted over the WHOLE published catalog
//! ([`PUBLISHED_BUCKETS`]) rather than the one key a client happened to read:
//!
//! * a bucket AWS publishes exists in S3 independently of any `CreateBucket` — no caller
//!   creates it and none has to — so a read addressed to one resolves the bucket and
//!   answers 200 with the published object's bytes, its length and its entity tag, while a
//!   key that bucket does not hold is `NoSuchKey` and a name nothing published and nobody
//!   created is still `NoSuchBucket`;
//! * and it belongs to no caller: it is not among the account's own buckets, its name is
//!   not available to a `CreateBucket`, and its life is not a caller's to end.
//!
//! The rows differ along what the rule generalises over — which bucket, which key, how
//! large the object is and which dataset split it carries — and the catalog is walked, so
//! a key added to it is covered here without a row being written for it.

use service_com_amazonaws_s3_amazons3::state::aws_published::{
    PublishedBucket, PublishedObject, PUBLISHED_BUCKETS, SAGEMAKER_SAMPLE_FILES,
};

use super::bucket_lifecycle::{create_bucket, head_bucket, router};
use super::list_buckets::list_buckets;
use super::object_and_tagging_roundtrip::{bucket, expected_etag, get_object, put_object, text};
use super::object_encryption::head_object;
use super::object_storage::{delete_bucket, list_objects_v2, listing_of};

/// Every `(bucket, object)` pair the catalog publishes, so a table spans the whole catalog
/// instead of naming one key.
fn published_pairs() -> Vec<(&'static PublishedBucket, &'static PublishedObject)> {
    PUBLISHED_BUCKETS
        .iter()
        .flat_map(|published| {
            published
                .objects
                .iter()
                .map(move |object| (published, object))
        })
        .collect()
}

#[tokio::test]
async fn every_published_object_reads_back_without_anybody_creating_its_bucket() {
    // Nothing is created in this test: the only buckets the store holds are the published
    // ones, which is the whole point — a client's first call can be a read.
    let router = router();

    for (published, object) in published_pairs() {
        let case = format!("{}/{}", published.name, object.key);
        let read = get_object(&router, published.name, object.key, "", &[]).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: a read of a published key must answer 200, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            object.body.as_bytes(),
            "case {case:?}: expected the published bytes, got {:?}",
            text(&read)
        );
        assert_eq!(
            read.header("content-type").as_deref(),
            Some(object.content_type),
            "case {case:?}: expected the published Content-Type {:?}, got {:?}",
            object.content_type,
            read.header("content-type")
        );
        assert_eq!(
            read.header("etag").as_deref(),
            Some(expected_etag(object.body.as_bytes()).as_str()),
            "case {case:?}: expected the entity tag of the published bytes, got {:?}",
            read.header("etag")
        );

        // The same object through the metadata read, which is what a staging client heads
        // before it fetches: same length, same type, same tag, no body.
        let headed = head_object(&router, published.name, object.key).await;
        assert_eq!(
            headed.status,
            200,
            "case {case:?}: HeadObject on a published key must answer 200, got {}",
            headed.status
        );
        assert_eq!(
            headed.header("content-length").as_deref(),
            Some(object.body.len().to_string().as_str()),
            "case {case:?}: expected content-length {}, got {:?}",
            object.body.len(),
            headed.header("content-length")
        );
        assert_eq!(
            headed.header("etag"),
            read.header("etag"),
            "case {case:?}: both readers must report one entity tag, got {:?} and {:?}",
            headed.header("etag"),
            read.header("etag")
        );

        // A read changes nothing a later caller sees: the second read of the same key
        // answers the same bytes, the same tag and the same modification instant.
        let again = get_object(&router, published.name, object.key, "", &[]).await;
        assert_eq!(
            again.body, read.body,
            "case {case:?}: a second read must answer the same bytes"
        );
        assert_eq!(
            (again.header("etag"), again.header("last-modified")),
            (read.header("etag"), read.header("last-modified")),
            "case {case:?}: a read must leave the object exactly as it was, got {:?} then {:?}",
            (read.header("etag"), read.header("last-modified")),
            (again.header("etag"), again.header("last-modified"))
        );
    }
}

#[tokio::test]
async fn a_published_bucket_tells_an_absent_key_from_a_bucket_nothing_published() {
    let router = router();

    // Keys the published bucket does NOT hold, each differing along a way a client could
    // get a published path wrong: a suffix added to a key it does hold, the key's own
    // directory prefix, a sibling file under the same prefix, and another dataset family
    // altogether. Every one of them resolves the BUCKET and reports the key.
    for (case, key) in [
        (
            "a published key with a suffix added",
            "datasets/tabular/uci_abalone/train_csv/abalone_dataset1_train.csv.gz",
        ),
        (
            "the prefix a published key lives under",
            "datasets/tabular/uci_abalone/train_csv/",
        ),
        (
            "a sibling the catalog does not publish",
            "datasets/tabular/uci_abalone/abalone.csv",
        ),
        ("another dataset family", "datasets/image/MNIST/mnist.pkl.gz"),
    ] {
        let read = get_object(&router, SAGEMAKER_SAMPLE_FILES, key, "", &[]).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a key the published bucket does not hold is 404, got {}",
            read.status
        );
        assert_eq!(
            read.error_code(),
            "NoSuchKey",
            "case {case:?}: the published bucket resolves, so the refusal names the KEY, got \
             {:?}",
            read.error_code()
        );
    }

    // And a name nothing published and nobody created is still no bucket at all — the
    // seeded catalog widens what resolves, it does not make every name resolve.
    for (case, name) in [
        ("one character off a published name", "sagemaker-sample-file"),
        (
            "a published name with a suffix",
            "sagemaker-sample-files-copy",
        ),
        ("a name of the caller's own shape", "a-bucket-nobody-created"),
    ] {
        let read = get_object(
            &router,
            name,
            "datasets/tabular/uci_abalone/train_csv/abalone_dataset1_train.csv",
            "",
            &[],
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a read of a bucket nothing holds is 404, got {}",
            read.status
        );
        assert_eq!(
            read.error_code(),
            "NoSuchBucket",
            "case {case:?}: expected NoSuchBucket, got {:?}",
            read.error_code()
        );
    }
}

#[tokio::test]
async fn every_published_dataset_row_carries_its_label_first() {
    // What a client staging training input does with the bytes: it splits a row into the
    // label and the feature vector. Asserted for every published object, not just the one
    // a flow happened to read: each row is comma-separated, carries the label in its first
    // field, and every row of one object has the same width.
    let router = router();

    for (published, object) in published_pairs() {
        let case = format!("{}/{}", published.name, object.key);
        let read = get_object(&router, published.name, object.key, "", &[]).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: the read must succeed, got {}",
            read.status
        );
        let body = String::from_utf8(read.body.to_vec())
            .unwrap_or_else(|error| panic!("case {case:?}: the body must be text: {error}"));

        let rows: Vec<&str> = body.lines().filter(|line| !line.is_empty()).collect();
        assert!(
            !rows.is_empty(),
            "case {case:?}: a published dataset split carries at least one row"
        );
        let width = rows[0].split(',').count();
        assert!(
            width >= 2,
            "case {case:?}: a row splits into a label and at least one feature, got {width} \
             field(s) in {:?}",
            rows[0]
        );
        for (index, row) in rows.iter().enumerate() {
            let fields: Vec<&str> = row.split(',').collect();
            assert_eq!(
                fields.len(),
                width,
                "case {case:?}: row {index} must be as wide as the first, got {:?}",
                row
            );
            assert!(
                fields[0].parse::<i64>().is_ok(),
                "case {case:?}: row {index} must carry its label first, got {:?}",
                fields[0]
            );
            for (position, feature) in fields[1..].iter().enumerate() {
                assert!(
                    feature.parse::<f64>().is_ok(),
                    "case {case:?}: row {index} feature {position} must be numeric, got {feature:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_published_bucket_lists_what_it_publishes_and_is_not_one_of_the_accounts_buckets() {
    let router = router();

    for published in PUBLISHED_BUCKETS {
        // It exists: the metadata read of the bucket answers, with the Region it is
        // published in.
        let headed = head_bucket(&router, published.name).await;
        assert_eq!(
            headed.status,
            200,
            "case {:?}: HeadBucket on a published bucket must answer 200, got {}",
            published.name,
            headed.status
        );
        assert_eq!(
            headed.header("x-amz-bucket-region").as_deref(),
            Some(published.region),
            "case {:?}: expected the published Region {:?}, got {:?}",
            published.name,
            published.region,
            headed.header("x-amz-bucket-region")
        );

        // And it holds exactly the keys the catalog publishes, in key order.
        let listed = listing_of(&list_objects_v2(&router, published.name, "").await);
        let mut expected: Vec<&str> = published.objects.iter().map(|object| object.key).collect();
        expected.sort_unstable();
        assert_eq!(
            listed.keys(),
            expected,
            "case {:?}: expected the published keys {expected:?}, got {:?}",
            published.name,
            listed.keys()
        );
    }

    // It is nobody's account bucket, so the account's own listing does not name it —
    // before or after the caller creates a bucket of its own.
    for stage in ["before any create", "after the caller creates one"] {
        if stage == "after the caller creates one" {
            bucket(&router, "a-bucket-of-the-callers-own").await;
        }
        let listing = text(&list_buckets(&router, "").await);
        for published in PUBLISHED_BUCKETS {
            assert!(
                !listing.contains(published.name),
                "case {stage:?}: ListBuckets must not name the published bucket {:?}, got {listing}",
                published.name
            );
        }
    }
    let listing = text(&list_buckets(&router, "").await);
    assert!(
        listing.contains("a-bucket-of-the-callers-own"),
        "the caller's own bucket must still be listed, got {listing}"
    );
}

#[tokio::test]
async fn a_published_bucket_is_not_the_callers_to_create_or_to_delete() {
    let router = router();

    for published in PUBLISHED_BUCKETS {
        // The name is not available: it is held by a bucket this account does not own.
        let created = create_bucket(&router, published.name, None, None).await;
        assert_eq!(
            created.status,
            409,
            "case {:?}: creating a published name must be refused with 409, got {} and body {:?}",
            published.name,
            created.status,
            text(&created)
        );
        assert_eq!(
            created.error_code(),
            "BucketAlreadyExists",
            "case {:?}: expected the name-not-available code, got {:?}",
            published.name,
            created.error_code()
        );

        // Nor is its life the caller's to end.
        let deleted = delete_bucket(&router, published.name).await;
        assert_eq!(
            deleted.status,
            403,
            "case {:?}: deleting a published bucket must be refused with 403, got {} and body \
             {:?}",
            published.name,
            deleted.status,
            text(&deleted)
        );
        assert_eq!(
            deleted.error_code(),
            "AccessDenied",
            "case {:?}: expected AccessDenied, got {:?}",
            published.name,
            deleted.error_code()
        );

        // And both refusals left every published object exactly where it was.
        for object in published.objects {
            let read = get_object(&router, published.name, object.key, "", &[]).await;
            assert_eq!(
                read.status,
                200,
                "case {:?}/{:?}: the refused calls must leave the object readable, got {}",
                published.name,
                object.key,
                read.status
            );
            assert_eq!(
                read.body.as_ref(),
                object.body.as_bytes(),
                "case {:?}/{:?}: the published bytes must be unchanged",
                published.name,
                object.key
            );
        }
    }
}

#[tokio::test]
async fn published_sample_data_stages_into_a_bucket_of_the_callers_own() {
    // The documented path end to end: read a published sample file, write it into a bucket
    // the caller creates, and read it back from there.
    let router = router();
    let staging = "sample-data-staging";
    bucket(&router, staging).await;

    for (published, object) in published_pairs() {
        let case = format!("{}/{}", published.name, object.key);
        let staged = get_object(&router, published.name, object.key, "", &[]).await;
        assert_eq!(
            staged.status,
            200,
            "case {case:?}: staging must read the published object, got {}",
            staged.status
        );

        let key = format!("input/{}", object.key);
        let written = put_object(
            &router,
            staging,
            &key,
            &staged.body,
            &[("content-type", object.content_type)],
        )
        .await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: the staged write must succeed, got {} and body {:?}",
            written.status,
            text(&written)
        );

        let read_back = get_object(&router, staging, &key, "", &[]).await;
        assert_eq!(
            read_back.status,
            200,
            "case {case:?}: the staged object must read back, got {}",
            read_back.status
        );
        assert_eq!(
            read_back.body, staged.body,
            "case {case:?}: the staged bytes must survive the round trip"
        );
        assert_eq!(
            read_back.header("etag"),
            staged.header("etag"),
            "case {case:?}: the staged copy must carry the same entity tag, got {:?} and {:?}",
            staged.header("etag"),
            read_back.header("etag")
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the reading-and-removing half of the object loop:
//! `ListObjectsV2`, `DeleteObject` and `DeleteBucket`.
//!
//! Every assertion drives a real HTTP request through the router — the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`] and
//! [`super::object_storage`] own — so what is pinned is the wire behaviour (status,
//! listing payload, error code) rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `ListObjectsV2` "returns some or all (up to 1,000) of the objects in a bucket … in
//!   lexicographical order", each entry's `Key`, `Size`, `ETag`, `LastModified` and
//!   `StorageClass` coming from the stored object, with `Name` the bucket and `KeyCount`
//!   "the number of keys returned with this request".
//! * `Prefix` limits the response to keys that begin with it, `StartAfter` makes S3
//!   "start listing after this specified key", and with a `Delimiter` the keys between a
//!   prefix and the next delimiter roll up into a common prefix, which "counts as a
//!   single return".
//! * `MaxKeys` "sets the maximum number of keys returned in the response"; a response
//!   that stopped short is `IsTruncated` with a `NextContinuationToken`, and sending that
//!   token back as `ContinuationToken` resumes the listing where it stopped.
//! * `DeleteObject` "removes an object from a bucket" and is idempotent: with nothing at
//!   the key there is nothing to remove, and the call still succeeds.
//! * "All objects … in the bucket must be deleted before the bucket itself can be
//!   deleted" (`DeleteBucket`), so a bucket that still holds one is refused with
//!   `BucketNotEmpty` and keeps its objects.
//! * A parameter whose answer this emulator cannot render keeps the scaffold's 501, and
//!   one outside its published range is refused with `InvalidArgument` — neither is
//!   answered as a plain success, and neither changes what is stored.

use super::bucket_lifecycle::{head_bucket, router, send};
use super::object_and_tagging_roundtrip::{bucket, expected_etag, get_object, put_object, text};
use super::object_storage::{
    delete_bucket, delete_object, list_objects_v2, listing_of, sorted_keys, store_keys, Listing,
};

#[tokio::test]
async fn a_listing_reports_every_stored_key_in_ascending_byte_order() {
    // Key sets whose byte order differs from any "natural" order a sort might fall back
    // on: upper case before lower case, `file-10` before `file-2`, and punctuation
    // ordered by code point (`!` 0x21 < `-` 0x2D < `/` 0x2F < `0` 0x30 < `_` 0x5F < `~`
    // 0x7E).
    let rows: &[(&str, &[&str])] = &[
        ("single key", &["only.txt"]),
        ("upper case before lower case", &["a.txt", "B.txt", "b.txt"]),
        ("digits are not numbers", &["file-1", "file-10", "file-2"]),
        (
            "punctuation by code point",
            &["a!b", "a-b", "a/b", "a0b", "a_b", "a~b"],
        ),
        (
            "nested prefixes",
            &["a", "a/b", "a/b/c", "ab", "b/a", "b/a/c"],
        ),
    ];

    for (case, keys) in rows {
        let router = router();
        let bucket_name = "listing-order";
        bucket(&router, bucket_name).await;
        let written = store_keys(&router, bucket_name, keys).await;

        let answer = list_objects_v2(&router, bucket_name, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a listing of a bucket that exists is 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let listing = listing_of(&answer);

        assert_eq!(
            listing.keys(),
            sorted_keys(&written),
            "case {case:?}: keys must be listed in ascending UTF-8 byte order, got {:?}",
            listing.keys()
        );
        assert_eq!(
            listing.field("Name"),
            bucket_name,
            "case {case:?}: Name is the bucket, got {:?}",
            listing.field("Name")
        );
        assert_eq!(
            listing.field("KeyCount"),
            keys.len().to_string(),
            "case {case:?}: KeyCount is the number of entries returned, got {:?}",
            listing.field("KeyCount")
        );
        assert_eq!(
            listing.field("IsTruncated"),
            "false",
            "case {case:?}: a page holding every key is not truncated, got {:?}",
            listing.field("IsTruncated")
        );
        assert_eq!(
            listing.field("NextContinuationToken"),
            "",
            "case {case:?}: an untruncated page owes no continuation token, got {:?}",
            listing.field("NextContinuationToken")
        );
        assert!(
            listing.common_prefixes.is_empty(),
            "case {case:?}: without a delimiter nothing rolls up, got {:?}",
            listing.common_prefixes
        );

        // Every entry is computed from the object stored at that key.
        for entry in &listing.contents {
            let key = Listing::entry_field(entry, "Key");
            let body = written.get(&key).unwrap_or_else(|| {
                panic!("case {case:?}: the listing named an unwritten key {key:?}")
            });
            assert_eq!(
                Listing::entry_field(entry, "Size"),
                body.len().to_string(),
                "case {case:?}, key {key:?}: Size is the stored byte length, got {:?}",
                Listing::entry_field(entry, "Size")
            );
            assert_eq!(
                Listing::entry_field(entry, "ETag"),
                expected_etag(body),
                "case {case:?}, key {key:?}: ETag is the stored object's entity tag, got {:?}",
                Listing::entry_field(entry, "ETag")
            );
            assert_eq!(
                Listing::entry_field(entry, "StorageClass"),
                "STANDARD",
                "case {case:?}, key {key:?}: an object written without a storage class is \
                 STANDARD, got {:?}",
                Listing::entry_field(entry, "StorageClass")
            );
            let last_modified = Listing::entry_field(entry, "LastModified");
            assert!(
                aws_smithy_types::DateTime::from_str(
                    &last_modified,
                    aws_smithy_types::date_time::Format::DateTime
                )
                .is_ok(),
                "case {case:?}, key {key:?}: LastModified must be a date-time, got \
                 {last_modified:?}"
            );
        }

        // A listing is a read: listing again answers the same thing, so nothing about the
        // store moved because something was enumerated.
        let again = listing_of(&list_objects_v2(&router, bucket_name, "").await);
        assert_eq!(
            again.keys(),
            listing.keys(),
            "case {case:?}: a second listing must report the same keys, got {:?}",
            again.keys()
        );
    }
}

#[tokio::test]
async fn prefix_start_after_and_delimiter_select_what_a_page_reports() {
    let keys: &[&str] = &["a/1", "a/2", "a/b/3", "b/1", "c"];

    // (case, query, the keys the page must report, the common prefixes it must report).
    let rows: &[(&str, &str, &[&str], &[&str])] = &[
        ("no selection", "", keys, &[]),
        (
            "prefix on a folder",
            "&prefix=a/",
            &["a/1", "a/2", "a/b/3"],
            &[],
        ),
        ("prefix matching nothing", "&prefix=zz", &[], &[]),
        ("prefix that is a whole key", "&prefix=c", &["c"], &[]),
        (
            "prefix shorter than a segment",
            "&prefix=a",
            &["a/1", "a/2", "a/b/3"],
            &[],
        ),
        (
            "delimiter rolls up every folder",
            "&delimiter=/",
            &["c"],
            &["a/", "b/"],
        ),
        (
            "prefix with a delimiter rolls up what is below it",
            "&prefix=a/&delimiter=/",
            &["a/1", "a/2"],
            &["a/b/"],
        ),
        (
            "start-after a stored key is exclusive",
            "&start-after=a/2",
            &["a/b/3", "b/1", "c"],
            &[],
        ),
        (
            "start-after a key that is not stored",
            "&start-after=a/10",
            &["a/2", "a/b/3", "b/1", "c"],
            &[],
        ),
        ("start-after the last key", "&start-after=c", &[], &[]),
        (
            "prefix and start-after together",
            "&prefix=a/&start-after=a/1",
            &["a/2", "a/b/3"],
            &[],
        ),
    ];

    let router = router();
    let bucket_name = "listing-selection";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, keys).await;

    for (case, query, expected_keys, expected_prefixes) in rows {
        let answer = list_objects_v2(&router, bucket_name, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let listing = listing_of(&answer);
        assert_eq!(
            listing.keys(),
            expected_keys
                .iter()
                .map(|key| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: wrong keys, got {:?}",
            listing.keys()
        );
        assert_eq!(
            listing.common_prefixes,
            expected_prefixes
                .iter()
                .map(|prefix| (*prefix).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: wrong common prefixes, got {:?}",
            listing.common_prefixes
        );
        assert_eq!(
            listing.field("KeyCount"),
            (expected_keys.len() + expected_prefixes.len()).to_string(),
            "case {case:?}: KeyCount counts each entry and each rolled-up prefix once, got {:?}",
            listing.field("KeyCount")
        );
        assert_eq!(
            listing.field("IsTruncated"),
            "false",
            "case {case:?}: nothing was left for a next page, got {:?}",
            listing.field("IsTruncated")
        );
    }
}

#[tokio::test]
async fn max_keys_bounds_a_page_and_the_continuation_token_resumes_it() {
    let keys: &[&str] = &["k1", "k2", "k3", "k4", "k5"];
    let router = router();
    let bucket_name = "listing-pages";
    bucket(&router, bucket_name).await;
    let written = store_keys(&router, bucket_name, keys).await;
    let every_key = sorted_keys(&written);

    // Page sizes below, at and above the number of stored keys: every one of them must
    // walk the whole listing, in order, with each page no larger than it asked for.
    for page_size in [1usize, 2, 3, 5, 7] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                None => format!("&max-keys={page_size}"),
                Some(token) => format!("&max-keys={page_size}&continuation-token={token}"),
            };
            let answer = list_objects_v2(&router, bucket_name, &query).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}, page {pages}: expected 200, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            let listing = listing_of(&answer);
            pages += 1;
            assert!(
                listing.keys().len() <= page_size,
                "case page size {page_size}, page {pages}: a page may not exceed MaxKeys, got \
                 {:?}",
                listing.keys()
            );
            assert_eq!(
                listing.field("KeyCount"),
                listing.keys().len().to_string(),
                "case page size {page_size}, page {pages}: KeyCount is what this page carries, \
                 got {:?}",
                listing.field("KeyCount")
            );
            walked.extend(listing.keys());

            let truncated = listing.field("IsTruncated");
            let next = listing.field("NextContinuationToken");
            assert_eq!(
                truncated,
                (walked.len() < every_key.len()).to_string(),
                "case page size {page_size}, page {pages}: truncated exactly while keys remain, \
                 got {truncated:?} after {} of {} keys",
                walked.len(),
                every_key.len()
            );
            if truncated == "true" {
                assert!(
                    !next.is_empty(),
                    "case page size {page_size}, page {pages}: a truncated page owes a token"
                );
                token = Some(next);
            } else {
                assert_eq!(
                    next, "",
                    "case page size {page_size}, page {pages}: a final page owes no token, got \
                     {next:?}"
                );
                break;
            }
            assert!(
                pages <= every_key.len() + 1,
                "case page size {page_size}: paging must make progress, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, every_key,
            "case page size {page_size}: paging must walk every key once, in order, got {walked:?}"
        );
        assert_eq!(
            pages,
            every_key.len().div_ceil(page_size),
            "case page size {page_size}: a page is filled before the next one starts, took \
             {pages} pages"
        );
    }

    // `max-keys=0` asks for no entries: an empty page, and NOT a truncated one — a token
    // there would make no progress.
    let empty = listing_of(&list_objects_v2(&router, bucket_name, "&max-keys=0").await);
    assert!(
        empty.keys().is_empty(),
        "case max-keys=0: no entries are returned, got {:?}",
        empty.keys()
    );
    assert_eq!(
        empty.field("KeyCount"),
        "0",
        "case max-keys=0: KeyCount is zero, got {:?}",
        empty.field("KeyCount")
    );
    assert_eq!(
        empty.field("IsTruncated"),
        "false",
        "case max-keys=0: an empty page is not a truncated one, got {:?}",
        empty.field("IsTruncated")
    );

    // A rolled-up prefix "counts as a single return", so it fills a slot of MaxKeys the
    // way an entry does, and the token resumes past it.
    let folded = super::bucket_lifecycle::router();
    let folded_bucket = "listing-pages-folded";
    bucket(&folded, folded_bucket).await;
    store_keys(&folded, folded_bucket, &["x/1", "x/2", "y/1", "z"]).await;
    let first = listing_of(&list_objects_v2(&folded, folded_bucket, "&delimiter=/&max-keys=1").await);
    assert_eq!(
        (first.keys(), first.common_prefixes.clone()),
        (Vec::new(), vec!["x/".to_string()]),
        "case delimiter with max-keys=1: the first page is one rolled-up prefix, got {:?} and \
         {:?}",
        first.keys(),
        first.common_prefixes
    );
    assert_eq!(
        first.field("KeyCount"),
        "1",
        "case delimiter with max-keys=1: a common prefix counts as one entry, got {:?}",
        first.field("KeyCount")
    );
    let resumed = listing_of(
        &list_objects_v2(
            &folded,
            folded_bucket,
            &format!(
                "&delimiter=/&continuation-token={}",
                first.field("NextContinuationToken")
            ),
        )
        .await,
    );
    assert_eq!(
        (resumed.keys(), resumed.common_prefixes.clone()),
        (vec!["z".to_string()], vec!["y/".to_string()]),
        "case delimiter resumed: the next page is what was left, got {:?} and {:?}",
        resumed.keys(),
        resumed.common_prefixes
    );
}

#[tokio::test]
async fn deleting_an_object_is_idempotent_and_removes_it_from_every_read() {
    let keys: &[&str] = &["keep/one", "remove/me", "keep/two"];
    let router = router();
    let bucket_name = "object-deletion";
    bucket(&router, bucket_name).await;
    let written = store_keys(&router, bucket_name, keys).await;

    // Deleting the same key twice, and a key that was never written, are all successes:
    // "Removes an object from a bucket" has nothing left to do the second time.
    for (case, key) in [
        ("a stored key", "remove/me"),
        ("the same key again", "remove/me"),
        ("a key that was never written", "never/written"),
    ] {
        let deleted = delete_object(&router, bucket_name, key).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: a delete answers 204, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        assert!(
            deleted.body.is_empty(),
            "case {case:?}: a 204 carries no body, got {:?}",
            text(&deleted)
        );
    }

    // The deleted key is gone from the read AND from the listing; the others are
    // untouched, bytes and entity tag included.
    let read = get_object(&router, bucket_name, "remove/me", "", &[]).await;
    assert_eq!(
        read.status, 404,
        "a deleted key is not found, got {} with body {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        read.error_code(),
        "NoSuchKey",
        "a deleted key answers NoSuchKey, got {:?}",
        read.error_code()
    );

    let listing = listing_of(&list_objects_v2(&router, bucket_name, "").await);
    assert_eq!(
        listing.keys(),
        vec!["keep/one".to_string(), "keep/two".to_string()],
        "the deleted key disappears from the listing, got {:?}",
        listing.keys()
    );
    assert_eq!(
        listing.field("KeyCount"),
        "2",
        "KeyCount follows the store, got {:?}",
        listing.field("KeyCount")
    );
    for key in ["keep/one", "keep/two"] {
        let kept = get_object(&router, bucket_name, key, "", &[]).await;
        assert_eq!(
            kept.status, 200,
            "case {key:?}: a key nobody deleted is still readable, got {} with body {:?}",
            kept.status,
            text(&kept)
        );
        let body = written.get(key).expect("the key was written");
        assert_eq!(
            kept.body.as_ref(),
            body.as_slice(),
            "case {key:?}: the stored bytes are unchanged by another key's delete"
        );
        assert_eq!(
            kept.header("etag").unwrap_or_default(),
            expected_etag(body),
            "case {key:?}: the entity tag is unchanged, got {:?}",
            kept.header("etag")
        );
    }

    // A delete addressed to a bucket this emulator does not hold is NOT a success: there
    // is no bucket for the call to have been idempotent in.
    let absent = delete_object(&router, "bucket-that-was-never-created", "remove/me").await;
    assert_eq!(
        absent.status, 404,
        "deleting in a bucket that does not exist is not found, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchBucket",
        "it answers NoSuchBucket, got {:?}",
        absent.error_code()
    );
}

#[tokio::test]
async fn a_bucket_is_deletable_only_once_it_holds_no_objects() {
    let keys: &[&str] = &["one", "two"];
    let router = router();
    let bucket_name = "bucket-deletion";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, keys).await;

    // While it still holds objects the delete is refused, and the refusal changes
    // nothing: the bucket and both objects are still there afterwards.
    let refused = delete_bucket(&router, bucket_name).await;
    assert_eq!(
        refused.status, 409,
        "a non-empty bucket is refused with a conflict, got {} with body {:?}",
        refused.status,
        text(&refused)
    );
    assert_eq!(
        refused.error_code(),
        "BucketNotEmpty",
        "the refusal is BucketNotEmpty, got {:?}",
        refused.error_code()
    );
    let survived = listing_of(&list_objects_v2(&router, bucket_name, "").await);
    assert_eq!(
        survived.keys(),
        vec!["one".to_string(), "two".to_string()],
        "a refused bucket delete keeps every object, got {:?}",
        survived.keys()
    );
    assert_eq!(
        head_bucket(&router, bucket_name).await.status,
        200,
        "the bucket itself is still there"
    );

    // Emptied one object at a time: still refused while one remains.
    assert_eq!(
        delete_object(&router, bucket_name, "one").await.status,
        204,
        "the first object is deleted"
    );
    let still_refused = delete_bucket(&router, bucket_name).await;
    assert_eq!(
        still_refused.status, 409,
        "one remaining object is enough to refuse the delete, got {} with body {:?}",
        still_refused.status,
        text(&still_refused)
    );

    assert_eq!(
        delete_object(&router, bucket_name, "two").await.status,
        204,
        "the last object is deleted"
    );
    let deleted = delete_bucket(&router, bucket_name).await;
    assert_eq!(
        deleted.status, 204,
        "an empty bucket is deleted, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );

    // Everything addressed to the deleted bucket now reports that it does not exist, and
    // nothing re-creates it implicitly.
    for (case, answer) in [
        (
            "ListObjectsV2",
            list_objects_v2(&router, bucket_name, "").await,
        ),
        (
            "GetObject",
            get_object(&router, bucket_name, "one", "", &[]).await,
        ),
        (
            "PutObject",
            put_object(&router, bucket_name, "three", b"xyz", &[]).await,
        ),
        (
            "DeleteObject",
            delete_object(&router, bucket_name, "one").await,
        ),
        ("DeleteBucket", delete_bucket(&router, bucket_name).await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a deleted bucket is not found, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchBucket",
            "case {case:?}: it answers NoSuchBucket, got {:?}",
            answer.error_code()
        );
    }

    // `HeadBucket` stops answering 200 too. Its status is the framework's client-error
    // default rather than the 404 S3 answers — `NotFound` is the only error it declares
    // and the model carries no `@httpError` on it — so what this pins is that the bucket
    // is no longer there, not that status: the `NoSuchBucket` above is where the code is
    // asserted, and HeadBucket's own status is the bucket-lifecycle suite's to pin.
    let headed = head_bucket(&router, bucket_name).await;
    assert_ne!(
        headed.status, 200,
        "HeadBucket must stop answering 200 once the bucket is deleted, got {}",
        headed.status
    );
}

#[tokio::test]
async fn a_listing_parameter_this_emulator_cannot_serve_is_refused_not_guessed() {
    let router = router();
    let bucket_name = "listing-refusals";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, &["one", "two"]).await;

    // (case, query, status, error code).
    let rows: &[(&str, &str, u16, &str)] = &[
        ("negative max-keys", "&max-keys=-1", 400, "InvalidArgument"),
        (
            "a continuation token this service did not issue",
            "&continuation-token=not-a-token-we-minted",
            400,
            "InvalidArgument",
        ),
        (
            "encoding-type, which changes how keys are written",
            "&encoding-type=url",
            501,
            "NotImplemented",
        ),
        (
            "fetch-owner, which adds an Owner to every entry",
            "&fetch-owner=true",
            501,
            "NotImplemented",
        ),
    ];

    for (case, query, status, code) in rows {
        let answer = list_objects_v2(&router, bucket_name, query).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected {code}, got {:?}",
            answer.error_code()
        );
    }

    // The third unsupported parameter is a HEADER rather than a query parameter:
    // `x-amz-optional-object-attributes` asks for an attribute (restore status) no stored
    // object here has, so it keeps the 501 too.
    let with_attributes = send(
        &router,
        "GET",
        &format!("/{bucket_name}?list-type=2"),
        &[("x-amz-optional-object-attributes", "RestoreStatus")],
        "",
    )
    .await;
    assert_eq!(
        with_attributes.status, 501,
        "case x-amz-optional-object-attributes: expected HTTP 501, got {} with body {:?}",
        with_attributes.status,
        text(&with_attributes)
    );
    assert_eq!(
        with_attributes.error_code(),
        "NotImplemented",
        "case x-amz-optional-object-attributes: expected NotImplemented, got {:?}",
        with_attributes.error_code()
    );

    // The refusals changed nothing: the listing still reports both keys.
    let listing = listing_of(&list_objects_v2(&router, bucket_name, "").await);
    assert_eq!(
        listing.keys(),
        vec!["one".to_string(), "two".to_string()],
        "a refused listing leaves the store alone, got {:?}",
        listing.keys()
    );

    // A listing of a bucket that was never created is not an empty listing.
    let absent = list_objects_v2(&router, "bucket-that-was-never-created", "").await;
    assert_eq!(
        absent.status, 404,
        "listing a bucket that does not exist is not found, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchBucket",
        "it answers NoSuchBucket, got {:?}",
        absent.error_code()
    );
}

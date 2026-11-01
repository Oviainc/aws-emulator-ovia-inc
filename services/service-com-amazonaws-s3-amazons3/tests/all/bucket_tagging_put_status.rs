// SPDX-License-Identifier: CC0-1.0

//! The STATUS a successful `PutBucketTagging` answers with.
//!
//! The rule under test: a successful write to a bucket's tagging subresource has nothing
//! to return — the operation's modelled output shape is `smithy.api#Unit`, which
//! `smithy_query(query="operation", operation="PutBucketTagging")` reports with no
//! members — so S3 answers it `204 No Content` with an EMPTY body. The status is a
//! property of the operation, never of the bucket, of how many tags the set carries, of
//! what its keys and values hold, or of whether the put stores the bucket's first tag set,
//! replaces one that was already there or empties it. (S3's API Reference sample response
//! for `PutBucketTagging` is `HTTP/1.1 204 No Content`; the same subresource's
//! `DeleteBucketTagging` carries 204 in the model itself, while `PutBucketTagging`'s
//! declared `@http(code:)` of 200 understates it.)
//!
//! The rows below span what the rule generalises over — the bucket (name form, length and
//! Region), the number of tags (none, one, three and the published per-bucket maximum),
//! key and value length (one character, empty, and both published bounds exactly), the
//! character class a key and a value hold (ASCII, non-ASCII, punctuation) and the ORDER a
//! bucket sees its puts in (first put, replacement, an identical repeat). The recorded
//! single-tag provisioning call that filed this is one row among them.
//!
//! The 204 is asserted NEXT TO the statuses it must not swallow: `GetBucketTagging`, which
//! has an output member to carry, still answers 200 with the tag-set document, and a
//! refused put still answers its own 4xx with an error document. So this is a statement
//! about this one no-output write, not a blanket "every tagging call is bodyless".
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour.

use service_com_amazonaws_s3_amazons3::state::{
    MAX_TAGS_PER_BUCKET, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};
use super::bucket_lifecycle::{create_bucket, router};
use super::bucket_tagging::{
    delete_bucket_tagging, get_bucket_tagging, put_bucket_tagging, tag, tag_set_of,
};

/// One row of the refusal table: the case, the bucket the put is addressed to, the tag set
/// it carries, and the status S3 refuses it with.
type RefusedPut = (String, &'static str, Vec<(String, String)>, u16);

/// The tag sets every row of which a successful put must answer 204 for.
///
/// The rows differ along what the rule generalises over: how many tags the set carries,
/// how long its keys and values are (including both published bounds exactly), and which
/// characters they hold. The bounds come from this crate's published per-bucket tagging
/// limits (`MAX_TAGS_PER_BUCKET`, `MAX_TAG_KEY_CHARS`, `MAX_TAG_VALUE_CHARS`).
fn accepted_tag_sets() -> Vec<(String, Vec<(String, String)>)> {
    vec![
        (
            "one tag, the shape a provisioning call sends".to_string(),
            vec![tag("sim2real", "component-configuration-lifecycle")],
        ),
        (
            "three tags in an order that is not key order".to_string(),
            vec![tag("zeta", "3"), tag("alpha", "1"), tag("middle", "2")],
        ),
        (
            "a one-character key and an empty value".to_string(),
            vec![tag("k", "")],
        ),
        (
            "a key and a value exactly at the published bounds".to_string(),
            vec![tag(
                &"k".repeat(MAX_TAG_KEY_CHARS),
                &"v".repeat(MAX_TAG_VALUE_CHARS),
            )],
        ),
        (
            "a non-ASCII key and value".to_string(),
            vec![tag("プロジェクト", "Grüße-日本語")],
        ),
        (
            "a value carrying the punctuation a tag may hold".to_string(),
            vec![tag("expression", "a + b = c.d_e:f/g@h")],
        ),
        (
            "the per-bucket tag count bound".to_string(),
            (0..MAX_TAGS_PER_BUCKET)
                .map(|index| tag(&format!("key-{index}"), &format!("value-{index}")))
                .collect(),
        ),
        (
            "an EMPTY tag set, which leaves the bucket with no tags".to_string(),
            Vec::new(),
        ),
    ]
}

#[tokio::test]
async fn a_successful_put_bucket_tagging_answers_204_with_an_empty_body() {
    let router = router();
    // Several buckets, differing in name length and form: the status is not a property of
    // the bucket. One is created outside `us-east-1` so the row set spans Regions too.
    let buckets: &[(&str, Option<&str>)] = &[
        ("abc", None),
        ("tagging-status-destination", None),
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

    let rows = accepted_tag_sets();
    for (bucket, _) in buckets {
        // Every row is put on this bucket in order, so each bucket sees a FIRST put and
        // then a replacement for every later row. The status must be 204 for all of them.
        for (case, tags) in &rows {
            let answer = put_bucket_tagging(&router, bucket, tags).await;
            assert_eq!(
                answer.status, 204,
                "case ({bucket:?}, {case:?}): expected HTTP 204, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                answer.body.is_empty(),
                "case ({bucket:?}, {case:?}): a 204 carries no body, got {:?}",
                String::from_utf8_lossy(&answer.body)
            );

            // The 204 is a successful WRITE, not a no-op: the reader reports the tag set
            // the put stored, and it answers 200 with its document because its output DOES
            // bind a member. An empty set leaves the bucket with no tag set at all, which
            // the reader reports as the documented `NoSuchTagSet`.
            let read = get_bucket_tagging(&router, bucket).await;
            if tags.is_empty() {
                assert_eq!(
                    read.error_code(),
                    "NoSuchTagSet",
                    "case ({bucket:?}, {case:?}): a bucket left with no tags has no tag \
                     set, got {} with body {:?}",
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                continue;
            }
            assert_eq!(
                read.status, 200,
                "case ({bucket:?}, {case:?}): GetBucketTagging carries an output member, so \
                 it must answer 200, got {} with body {:?}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                &tag_set_of(&read),
                tags,
                "case ({bucket:?}, {case:?}): the stored tag set must read back unchanged, \
                 got {:?}",
                tag_set_of(&read)
            );
        }

        // An identical repeat of a row already stored: a put that changes nothing is still
        // a successful 204, never a 200 and never a 304.
        let (case, stored) = &rows[0];
        for attempt in 1..=2 {
            let repeat = put_bucket_tagging(&router, bucket, stored).await;
            assert_eq!(
                repeat.status, 204,
                "case ({bucket:?}, {case:?} put attempt {attempt}): expected HTTP 204, got \
                 {} with body {:?}",
                repeat.status,
                String::from_utf8_lossy(&repeat.body)
            );
            assert!(
                repeat.body.is_empty(),
                "case ({bucket:?}, {case:?} put attempt {attempt}): a 204 carries no body, \
                 got {:?}",
                String::from_utf8_lossy(&repeat.body)
            );
        }

        // And the subresource's own delete answers the same 204, so the write and the
        // removal of a bucket's tag set are indistinguishable on the status line.
        let deleted = delete_bucket_tagging(&router, bucket).await;
        assert_eq!(
            deleted.status, 204,
            "case {bucket:?}: DeleteBucketTagging must answer 204, got {} with body {:?}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
    }
}

#[tokio::test]
async fn a_refused_put_bucket_tagging_keeps_its_own_status_and_error_document() {
    let router = router();
    let bucket = "tagging-status-refusals";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let stored = vec![tag("keep", "me")];
    let setup = put_bucket_tagging(&router, bucket, &stored).await;
    assert_eq!(
        setup.status, 204,
        "the setup put must succeed with 204, got {} with body {:?}",
        setup.status,
        String::from_utf8_lossy(&setup.body)
    );

    // The 204 belongs to SUCCESS only. Each row below is a different way for the put to
    // fail — one past each published bound, a set whose keys are not unique, and a bucket
    // this emulator does not hold — and each must keep its own status and error code
    // rather than being flattened into a bodyless 204.
    let rows: Vec<RefusedPut> = vec![
        (
            "one tag past the per-bucket count bound".to_string(),
            bucket,
            (0..=MAX_TAGS_PER_BUCKET)
                .map(|index| tag(&format!("key-{index}"), "v"))
                .collect(),
            400,
        ),
        (
            "a key one character past its bound".to_string(),
            bucket,
            vec![tag(&"k".repeat(MAX_TAG_KEY_CHARS + 1), "v")],
            400,
        ),
        (
            "a value one character past its bound".to_string(),
            bucket,
            vec![tag("key", &"v".repeat(MAX_TAG_VALUE_CHARS + 1))],
            400,
        ),
        (
            "the same key twice".to_string(),
            bucket,
            vec![tag("duplicate", "first"), tag("duplicate", "second")],
            400,
        ),
        (
            "a valid tag set on a bucket that is not there".to_string(),
            "no-such-bucket-at-all",
            vec![tag("sim2real", "absent")],
            404,
        ),
    ];
    for (case, target, tags, status) in &rows {
        let answer = put_bucket_tagging(&router, target, tags).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            !answer.body.is_empty(),
            "case {case:?}: a refusal carries an error document, got an empty body"
        );
    }

    // And nothing a refused put carried reached the store: the bucket still reports the
    // tag set the successful 204 above wrote.
    let read = get_bucket_tagging(&router, bucket).await;
    assert_eq!(
        tag_set_of(&read),
        stored,
        "a refused put must leave the stored tag set untouched, got {} with body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
}

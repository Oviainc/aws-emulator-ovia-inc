// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's TAG SET: `PutBucketTagging` stores it,
//! `GetBucketTagging` reports it and `DeleteBucketTagging` removes it.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! the bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutBucketTagging` replaces a bucket's whole tag set with the request's `TagSet` and
//!   answers 200 with an empty body: "When this operation sets the tags for a bucket, it
//!   will overwrite any current tags the bucket already has. You cannot use this operation
//!   to add tags to an existing list of tags" (the operation's own documentation).
//! * `GetBucketTagging` reports exactly that tag set, and when a bucket has none it
//!   answers the error the model's documentation names for exactly that case: "Error code:
//!   NoSuchTagSet Description: There is no tag set associated with the bucket", HTTP 404.
//! * `DeleteBucketTagging` removes the tag set and answers the modelled 204, whether or
//!   not the bucket carried tags.
//! * A tagging call naming a bucket this emulator does not hold answers the service's
//!   `NoSuchBucket` (HTTP 404), the shape each of the three handlers widens in.
//! * A tag set AWS would refuse with `InvalidTag` — past the published 50 tags per bucket,
//!   128-character key or 256-character value, or naming one key twice — is refused with
//!   HTTP 400 and leaves the stored tag set untouched.

use quick_xml::escape::{escape, unescape};
use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::{
    MAX_TAGS_PER_BUCKET, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The `Tagging` payload for a tag set, escaped the way a client's XML writer escapes it.
fn tagging_payload(tags: &[(String, String)]) -> String {
    let mut body = format!("<Tagging xmlns=\"{XMLNS}\"><TagSet>");
    for (key, value) in tags {
        body.push_str(&format!(
            "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
            escape(key),
            escape(value)
        ));
    }
    body.push_str("</TagSet></Tagging>");
    body
}

/// `PUT /<bucket>?tagging` — `PutBucketTagging`.
///
/// `pub(crate)`, like the reader below it: the object suite in this same test crate tags a
/// bucket through the same helper instead of writing a second tagging payload builder.
pub(crate) async fn put_bucket_tagging(
    router: &MultiServiceRouter,
    bucket: &str,
    tags: &[(String, String)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?tagging"),
        &[],
        &tagging_payload(tags),
    )
    .await
}

/// `GET /<bucket>?tagging` — `GetBucketTagging`.
pub(crate) async fn get_bucket_tagging(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?tagging"), &[], "").await
}

/// `DELETE /<bucket>?tagging` — `DeleteBucketTagging`.
async fn delete_bucket_tagging(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "DELETE", &format!("/{bucket}?tagging"), &[], "").await
}

/// The `(Key, Value)` pairs a `GetBucketTagging` body reports, in document order.
///
/// Parsed with `quick-xml` (already a dependency of this workspace) rather than by
/// slicing the body, so escaped text and an empty `<Value/>` are read the way a client's
/// XML reader reads them.
pub(crate) fn tag_set_of(answer: &Answer) -> Vec<(String, String)> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut element = String::new();
    let mut key = String::new();
    let mut value = String::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                if element == "Tag" {
                    // An element that carries no text is the empty string, which is why
                    // both accumulators start every `Tag` cleared.
                    key.clear();
                    value.clear();
                }
            }
            Event::Text(text) => {
                let decoded = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"));
                push_fragment(&element, &unescaped(&decoded), &mut key, &mut value);
            }
            // The reader breaks text at each entity reference and emits the reference on
            // its own, so a loop that handled only `Event::Text` would drop every `&amp;`.
            Event::GeneralRef(reference) => {
                let name = reference
                    .decode()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"));
                push_fragment(
                    &element,
                    &unescaped(&format!("&{name};")),
                    &mut key,
                    &mut value,
                );
            }
            Event::End(end) => {
                if end.name().as_ref() == b"Tag" {
                    pairs.push((std::mem::take(&mut key), std::mem::take(&mut value)));
                }
                element.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    pairs
}

/// `text` with its predefined entities and numeric character references resolved.
fn unescaped(text: &str) -> String {
    unescape(text)
        .unwrap_or_else(|error| panic!("the response body must unescape: {error}"))
        .into_owned()
}

/// Append one decoded fragment to whichever of `Key` / `Value` is open.
fn push_fragment(element: &str, fragment: &str, key: &mut String, value: &mut String) {
    match element {
        "Key" => key.push_str(fragment),
        "Value" => value.push_str(fragment),
        _ => {}
    }
}

/// One tag, owned, so a table row can hold generated keys and values.
pub(crate) fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// The table of tag sets every row of which must round-trip unchanged.
///
/// The rows differ along what the replace-and-report rule generalises over: how many tags
/// the set carries (one, three, and the published per-bucket maximum), the order they
/// arrive in, how long a key and a value are (one character, empty, and both published
/// bounds exactly), and which characters they contain (ASCII, non-ASCII, and XML
/// metacharacters). The single-tag row is the shape the issue's provisioning call sent;
/// every other row is a case it never mentioned.
fn round_trip_rows() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        (
            "one tag, the shape a provisioning script sends",
            vec![tag("sim2real", "create-pipeline-reaches-active-state")],
        ),
        (
            "three tags keep the order the request carried, not key order",
            vec![tag("zeta", "3"), tag("alpha", "1"), tag("middle", "2")],
        ),
        (
            "a one-character key and an empty value",
            vec![tag("k", "")],
        ),
        (
            "a key and a value exactly at the published bounds",
            vec![tag(
                &"k".repeat(MAX_TAG_KEY_CHARS),
                &"v".repeat(MAX_TAG_VALUE_CHARS),
            )],
        ),
        (
            "non-ASCII key and value are stored as given",
            vec![tag("プロジェクト", "Grüße-日本語")],
        ),
        (
            "a value with XML metacharacters survives the round trip",
            vec![tag("expression", "a < b & c > d \"quoted\" 'it'")],
        ),
        (
            "the per-bucket tag count bound",
            (0..MAX_TAGS_PER_BUCKET)
                .map(|index| tag(&format!("key-{index}"), &format!("value-{index}")))
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn putting_a_tag_set_stores_it_and_reads_back_through_get_bucket_tagging() {
    let router = router();

    for (index, (case, tags)) in round_trip_rows().into_iter().enumerate() {
        let bucket = format!("tagging-round-trip-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let put = put_bucket_tagging(&router, &bucket, &tags).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutBucketTagging, got {} with \
             body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: PutBucketTagging's output is Unit, so the body must be empty, \
             got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_bucket_tagging(&router, &bucket).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetBucketTagging on the bucket just \
             tagged, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            tag_set_of(&read),
            tags,
            "case {case:?}: expected the stored tag set back unchanged, got {:?}",
            tag_set_of(&read)
        );

        // The second put REPLACES: the tag set that comes back is the new one alone, with
        // nothing merged in from the set above.
        let replacement = vec![tag("replaced", "by-the-second-put")];
        let second = put_bucket_tagging(&router, &bucket, &replacement).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: the replacing put must succeed, got {} with body {:?}",
            second.status,
            String::from_utf8_lossy(&second.body)
        );
        let reread = get_bucket_tagging(&router, &bucket).await;
        assert_eq!(
            tag_set_of(&reread),
            replacement,
            "case {case:?}: the tag set must be replaced, not merged, got {:?}",
            tag_set_of(&reread)
        );
    }
}

#[tokio::test]
async fn removing_a_tag_set_leaves_the_bucket_with_none() {
    let router = router();
    let bucket = "tagging-removal";
    create_bucket(&router, bucket, None, None).await;

    // Three ways a bucket ends up with no tag set: it was never tagged, its tag set was
    // deleted, or a put replaced it with an empty one. Every one of them must read back
    // the same way, and a delete must succeed from any of them (it is idempotent: "removes
    // the entire tag set from the specified bucket").
    let stored = vec![tag("owner", "platform"), tag("stage", "test")];

    for (case, prepare) in [
        ("never tagged", 0usize),
        ("tag set deleted", 1),
        ("tag set replaced by an empty one", 2),
        ("deleted twice over", 3),
    ] {
        match prepare {
            1 => {
                let put = put_bucket_tagging(&router, bucket, &stored).await;
                assert_eq!(
                    put.status, 200,
                    "case {case:?}: the setup put must succeed, got {}",
                    put.status
                );
                let deleted = delete_bucket_tagging(&router, bucket).await;
                assert_eq!(
                    deleted.status, 204,
                    "case {case:?}: expected the modelled 204 from DeleteBucketTagging, \
                     got {} with body {:?}",
                    deleted.status,
                    String::from_utf8_lossy(&deleted.body)
                );
            }
            2 => {
                let put = put_bucket_tagging(&router, bucket, &stored).await;
                assert_eq!(
                    put.status, 200,
                    "case {case:?}: the setup put must succeed, got {}",
                    put.status
                );
                let emptied = put_bucket_tagging(&router, bucket, &[]).await;
                assert_eq!(
                    emptied.status, 200,
                    "case {case:?}: a put of an empty TagSet must succeed, got {} with \
                     body {:?}",
                    emptied.status,
                    String::from_utf8_lossy(&emptied.body)
                );
            }
            3 => {
                let deleted = delete_bucket_tagging(&router, bucket).await;
                assert_eq!(
                    deleted.status, 204,
                    "case {case:?}: deleting a tag set that is already gone still answers \
                     204, got {}",
                    deleted.status
                );
            }
            _ => {}
        }

        let read = get_bucket_tagging(&router, bucket).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a bucket with no tag set answers the documented NoSuchTagSet \
             rather than an empty tag set, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.error_code(),
            "NoSuchTagSet",
            "case {case:?}: expected the documented error code, got {:?}",
            read.error_code()
        );
    }

    // And the bucket is still tagable afterwards: the rows above left no tags, not a
    // broken bucket.
    let put = put_bucket_tagging(&router, bucket, &stored).await;
    assert_eq!(
        put.status, 200,
        "a bucket whose tag set was removed must accept a new one, got {}",
        put.status
    );
    let read = get_bucket_tagging(&router, bucket).await;
    assert_eq!(
        tag_set_of(&read),
        stored,
        "the re-tagged bucket must report the new tag set, got {:?}",
        tag_set_of(&read)
    );
}

#[tokio::test]
async fn a_tag_set_past_a_published_limit_is_refused_and_changes_nothing() {
    let router = router();
    let bucket = "tagging-refusals";
    create_bucket(&router, bucket, None, None).await;

    // The tag set the bucket holds throughout: every refusal below must leave it exactly
    // as it is.
    let stored = vec![tag("keep", "me"), tag("and", "me-too")];
    let put = put_bucket_tagging(&router, bucket, &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    // (case, the refused tag set). One row per published bound, each ONE past it, plus the
    // key-uniqueness the set itself has.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "one tag past the per-bucket count bound",
            (0..=MAX_TAGS_PER_BUCKET)
                .map(|index| tag(&format!("key-{index}"), "v"))
                .collect(),
        ),
        (
            "a key one character past its bound",
            vec![tag(&"k".repeat(MAX_TAG_KEY_CHARS + 1), "v")],
        ),
        (
            "a value one character past its bound",
            vec![tag("key", &"v".repeat(MAX_TAG_VALUE_CHARS + 1))],
        ),
        (
            "the same key twice",
            vec![tag("duplicate", "first"), tag("duplicate", "second")],
        ),
        (
            "the same key twice among tags that are fine",
            vec![
                tag("alpha", "1"),
                tag("beta", "2"),
                tag("alpha", "3"),
            ],
        ),
    ];

    for (case, tags) in rows {
        let refused = put_bucket_tagging(&router, bucket, &tags).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the tag set to be refused with HTTP 400, got {} with \
             body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected the framework's refusal code for AWS's unmodelled \
             InvalidTag, got {:?}",
            refused.error_code()
        );

        let read = get_bucket_tagging(&router, bucket).await;
        assert_eq!(
            tag_set_of(&read),
            stored,
            "case {case:?}: a refused put must leave the stored tag set untouched, got \
             {:?}",
            tag_set_of(&read)
        );
    }

    // Which bucket the request names is decided BEFORE the tag set is judged: a refused
    // tag set sent at a bucket this emulator does not hold answers the bucket, the way the
    // service orders the two refusals.
    let absent = put_bucket_tagging(
        &router,
        "tagging-refusals-absent",
        &[tag("duplicate", "first"), tag("duplicate", "second")],
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "a refused tag set at an absent bucket answers the bucket first, got {} with body \
         {:?}",
        absent.status,
        String::from_utf8_lossy(&absent.body)
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchBucket",
        "a refused tag set at an absent bucket answers the bucket first, got {:?}",
        absent.error_code()
    );
}

#[tokio::test]
async fn tagging_a_bucket_this_emulator_does_not_hold_is_no_such_bucket() {
    let router = router();
    create_bucket(&router, "held-tagged-bucket", None, None).await;

    // Names that are NOT the one bucket created above: a different name, a prefix of it,
    // an extension of it, and one differing only in case (bucket names are
    // case-sensitive). `NoSuchBucket` carries `httpError 404` in the model.
    for bucket in [
        "absent-bucket",
        "held",
        "held-tagged-bucket-2",
        "Held-Tagged-Bucket",
    ] {
        let tags = vec![tag("sim2real", "no-such-bucket")];
        for (operation, answer) in [
            ("PutBucketTagging", put_bucket_tagging(&router, bucket, &tags).await),
            ("GetBucketTagging", get_bucket_tagging(&router, bucket).await),
            (
                "DeleteBucketTagging",
                delete_bucket_tagging(&router, bucket).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {bucket:?}/{operation}: expected HTTP 404 for a bucket this \
                 emulator does not hold, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchBucket",
                "case {bucket:?}/{operation}: expected the modelled error code, got {:?}",
                answer.error_code()
            );
        }
    }

    // Nothing above created a bucket as a side effect of tagging one that is absent.
    for bucket in ["absent-bucket", "held"] {
        let read = get_bucket_tagging(&router, bucket).await;
        assert_eq!(
            read.status, 404,
            "case {bucket:?}: a refused tagging call must not have created the bucket, \
             got {}",
            read.status
        );
    }

    // And the bucket that IS held tags normally, so the rows above failed on the lookup
    // rather than on the operations being unreachable.
    let tags = vec![tag("sim2real", "held")];
    let put = put_bucket_tagging(&router, "held-tagged-bucket", &tags).await;
    assert_eq!(
        put.status, 200,
        "the held bucket must still accept a tag set, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    let read = get_bucket_tagging(&router, "held-tagged-bucket").await;
    assert_eq!(
        tag_set_of(&read),
        tags,
        "the held bucket must report the tag set it was given, got {:?}",
        tag_set_of(&read)
    );
}

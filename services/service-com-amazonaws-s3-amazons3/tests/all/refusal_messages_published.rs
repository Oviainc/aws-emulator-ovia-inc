// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MESSAGE S3 refuses a bad request value with.
//!
//! The rule under test: where S3 publishes its own sentence for a field, a refusal of that
//! field carries that sentence — never a report naming the Smithy member path the value
//! travelled on, which no S3 client or document uses. The error CODE and the HTTP status
//! are the ones the published refusal carries too.
//!
//! The sentences, each from the AWS S3 API reference for the operation that binds the
//! field:
//!
//! * `CopyObject`'s `x-amz-tagging-directive` outside `TaggingDirective` -> `Unknown
//!   tagging directive.`, and its `x-amz-metadata-directive` outside `MetadataDirective`
//!   -> `Unknown metadata directive.` (both enums over `[COPY, REPLACE]`).
//! * `GetObjectAttributes`' `x-amz-object-attributes` outside `ObjectAttributes` ->
//!   `Invalid attribute name specified.`
//! * `CreateBucket`'s `x-amz-object-ownership` outside `ObjectOwnership` -> `Invalid
//!   x-amz-object-ownership header: <the rejected value>`, the one sentence of the set that
//!   interpolates anything, and what it interpolates is the request's own header value.
//! * `ListBuckets`' `max-buckets` outside `MaxBuckets`' range (`min 1`, `max 10000`) ->
//!   `Argument max-buckets must be an integer between 1 and 10000`: the published BOUNDS,
//!   not the value that was sent.
//! * A tag set S3 will not store is refused with the fixed sentence for the restriction it
//!   broke — `Object tags cannot be greater than 10`, `Cannot provide multiple Tags with
//!   the same key`, `The TagKey you have provided is invalid`, `The TagValue you have
//!   provided is invalid` — none of which carries the offending key, value or count.

use service_com_amazonaws_s3_amazons3::state::{
    MAX_TAGS_PER_BUCKET, MAX_TAGS_PER_OBJECT, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};

use super::bucket_lifecycle::{create_bucket, router, send};
use super::bucket_tagging::{put_bucket_tagging, tag, tag_set_of, tagging_payload};
use super::object_and_tagging_roundtrip::{put_object, text};
use super::object_tagging::{get_object_tagging, put_object_tagging};

/// Every value `ObjectOwnership` publishes
/// (`smithy_query(query="shape",shape="ObjectOwnership")`), so a row that must be REFUSED
/// can be checked against the set rather than assumed to be outside it.
const OBJECT_OWNERSHIP_VALUES: [&str; 3] =
    ["BucketOwnerPreferred", "ObjectWriter", "BucketOwnerEnforced"];

/// The published bounds of `MaxBuckets` (`smithy_query(query="shape",shape="MaxBuckets")`
/// reports `range {min 1, max 10000}`), which the refusal states.
const MAX_BUCKETS_RANGE: (i64, i64) = (1, 10_000);

/// One refused request: the case, the request to send, and the code, status and message it
/// must be refused with.
struct Row {
    case: String,
    method: &'static str,
    uri: String,
    headers: Vec<(String, String)>,
    body: String,
    code: &'static str,
    message: String,
}

/// The per-field rows, each naming a DIFFERENT operation and a different HTTP binding (two
/// headers of one operation, a header whose target is a list of enums, a header on a
/// create, and a query parameter with a numeric range), so the table spans what the rule
/// generalises over rather than repeating one recorded call.
fn rows(bucket: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // The two `CopyObject` directives. Each is tried with two values outside its enum —
    // one that differs in case from a value inside it, one that is a word of its own — so
    // the sentence is not keyed to one rejected spelling.
    for (header, outside, message) in [
        (
            "x-amz-tagging-directive",
            ["MERGE", "copy"],
            "Unknown tagging directive.",
        ),
        (
            "x-amz-metadata-directive",
            ["MERGE", "replace-all"],
            "Unknown metadata directive.",
        ),
    ] {
        for value in outside {
            rows.push(Row {
                case: format!("{header}: {value}"),
                method: "PUT",
                uri: format!("/{bucket}/copied"),
                headers: vec![
                    (
                        "x-amz-copy-source".to_string(),
                        format!("{bucket}/source"),
                    ),
                    (header.to_string(), value.to_string()),
                ],
                body: String::new(),
                code: "InvalidArgument",
                message: message.to_string(),
            });
        }
    }

    // `GetObjectAttributes`' header is a LIST of enums, so the rejected element may be the
    // only one, the first of several or a later one — the sentence names none of that.
    for value in [
        "NotARealAttribute",
        "etag",
        "NotARealAttribute, ETag",
        "ETag, ObjectSize, NotARealAttribute",
    ] {
        rows.push(Row {
            case: format!("x-amz-object-attributes: {value}"),
            method: "GET",
            uri: format!("/{bucket}/source?attributes"),
            headers: vec![("x-amz-object-attributes".to_string(), value.to_string())],
            body: String::new(),
            code: "InvalidArgument",
            message: "Invalid attribute name specified.".to_string(),
        });
    }

    // `CreateBucket`'s header is the one row whose sentence carries a value from the
    // request, so each row asserts its OWN value comes back in it.
    for value in ["BucketOwnerOnly", "objectwriter", "Owner Enforced"] {
        assert!(
            !OBJECT_OWNERSHIP_VALUES.contains(&value),
            "every row of this table must name a value OUTSIDE {OBJECT_OWNERSHIP_VALUES:?}, \
             got {value:?}"
        );
        rows.push(Row {
            case: format!("x-amz-object-ownership: {value}"),
            method: "PUT",
            uri: "/refusal-message-ownership".to_string(),
            headers: vec![("x-amz-object-ownership".to_string(), value.to_string())],
            body: String::new(),
            code: "InvalidArgument",
            message: format!("Invalid x-amz-object-ownership header: {value}"),
        });
    }

    // `ListBuckets`' range, one past each published bound and far past the maximum: the
    // sentence states the bounds, so every row expects the same text.
    let (min, max) = MAX_BUCKETS_RANGE;
    for value in [min - 1, max + 1, max * 10] {
        rows.push(Row {
            case: format!("max-buckets={value}"),
            method: "GET",
            uri: format!("/?x-id=ListBuckets&max-buckets={value}"),
            headers: Vec::new(),
            body: String::new(),
            code: "InvalidArgument",
            message: format!(
                "Argument max-buckets must be an integer between {min} and {max}"
            ),
        });
    }

    rows
}

#[tokio::test]
async fn a_refused_field_carries_the_message_s3_publishes_for_it() {
    let router = router();
    let bucket = "refusal-messages";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );
    let stored = put_object(&router, bucket, "source", b"bytes", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the source object must be stored first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    for row in rows(bucket) {
        let headers: Vec<(&str, &str)> = row
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let answer = send(&router, row.method, &row.uri, &headers, &row.body).await;
        assert_eq!(
            answer.error_code(),
            row.code,
            "case {:?}: expected the error code {:?}, got {:?} with status {} and body {:?}",
            row.case,
            row.code,
            answer.error_code(),
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.status, 400,
            "case {:?}: an S3 refusal of a bad argument is HTTP 400, got {}",
            row.case, answer.status
        );
        assert_eq!(
            answer.error_message(),
            row.message,
            "case {:?}: expected the published message {:?}, got {:?}",
            row.case,
            row.message,
            answer.error_message()
        );
    }

    // The bounds themselves are SERVED, so the rows above are about being outside the
    // range and not about the parameter being present at all.
    let (min, max) = MAX_BUCKETS_RANGE;
    for value in [min, max] {
        let answer = send(
            &router,
            "GET",
            &format!("/?x-id=ListBuckets&max-buckets={value}"),
            &[],
            "",
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case \"max-buckets={value}\": a value at a published bound must be served, got \
             {} with body {:?}",
            answer.status,
            text(&answer)
        );
    }
    // And so is each value the ownership enum publishes.
    for (index, setting) in OBJECT_OWNERSHIP_VALUES.into_iter().enumerate() {
        let answer = send(
            &router,
            "PUT",
            &format!("/refusal-message-ownership-{index}"),
            &[("x-amz-object-ownership", setting)],
            "",
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {setting:?}: a value the enum publishes must be served, got {} with body \
             {:?}",
            answer.status,
            text(&answer)
        );
    }
}

/// One refused tag set: the case, the tags, and the code and message the refusal carries.
struct TagRow {
    case: String,
    tags: Vec<(String, String)>,
    code: &'static str,
    message: &'static str,
}

/// A character outside the class a tag may hold, one per row of the character rows: an
/// astral-plane code point, a BMP symbol and an ASCII punctuation mark the class leaves out.
const DISALLOWED: [char; 3] = ['\u{1f600}', '§', '?'];

#[tokio::test]
async fn a_refused_tag_set_carries_the_fixed_message_s3_publishes() {
    let router = router();
    let bucket = "refusal-message-tags";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {}",
        created.status
    );
    let key = "tagged.bin";
    let stored = put_object(&router, bucket, key, b"bytes", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the object must be stored first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
    // A tag set that IS acceptable, stored first: every refusal below must leave it.
    let held = vec![tag("held", "value")];
    let accepted = put_object_tagging(&router, bucket, key, &held).await;
    assert_eq!(
        accepted.status, 200,
        "the setup tag set must be stored, got {} with body {:?}",
        accepted.status,
        text(&accepted)
    );

    let mut rows: Vec<TagRow> = Vec::new();

    // Past the published count, one over and far over: the sentence carries the resource's
    // own maximum, never the count that was sent.
    for count in [MAX_TAGS_PER_OBJECT + 1, MAX_TAGS_PER_OBJECT * 3] {
        rows.push(TagRow {
            case: format!("{count} tags on an object"),
            tags: (0..count)
                .map(|index| tag(&format!("key{index}"), "value"))
                .collect(),
            code: "BadRequest",
            message: "Object tags cannot be greater than 10",
        });
    }

    // One key named twice, as the first pair and as a later one.
    rows.push(TagRow {
        case: "the same key twice".to_string(),
        tags: vec![tag("dup", "a"), tag("dup", "b")],
        code: "InvalidTag",
        message: "Cannot provide multiple Tags with the same key",
    });
    rows.push(TagRow {
        case: "the same key twice, after another tag".to_string(),
        tags: vec![tag("other", "x"), tag("dup", "a"), tag("dup", "b")],
        code: "InvalidTag",
        message: "Cannot provide multiple Tags with the same key",
    });

    // A character outside the class, in the VALUE and in the KEY, with its own sentence
    // for each half of the tag.
    for character in DISALLOWED {
        rows.push(TagRow {
            case: format!("a value carrying {character:?}"),
            tags: vec![tag("key", &format!("value{character}"))],
            code: "InvalidTag",
            message: "The TagValue you have provided is invalid",
        });
        rows.push(TagRow {
            case: format!("a key carrying {character:?}"),
            tags: vec![tag(&format!("key{character}"), "value")],
            code: "InvalidTag",
            message: "The TagKey you have provided is invalid",
        });
    }

    // Past the published character bounds: the same two sentences, because what S3 states
    // is which half of the tag was not valid.
    rows.push(TagRow {
        case: "a key one character past its bound".to_string(),
        tags: vec![tag(&"k".repeat(MAX_TAG_KEY_CHARS + 1), "value")],
        code: "InvalidTag",
        message: "The TagKey you have provided is invalid",
    });
    rows.push(TagRow {
        case: "a value one character past its bound".to_string(),
        tags: vec![tag("key", &"v".repeat(MAX_TAG_VALUE_CHARS + 1))],
        code: "InvalidTag",
        message: "The TagValue you have provided is invalid",
    });

    for row in &rows {
        let answer = put_object_tagging(&router, bucket, key, &row.tags).await;
        assert_eq!(
            answer.error_code(),
            row.code,
            "case {:?}: expected the error code {:?}, got {:?} with status {} and body {:?}",
            row.case,
            row.code,
            answer.error_code(),
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.status, 400,
            "case {:?}: a refused tag set is HTTP 400, got {}",
            row.case, answer.status
        );
        assert_eq!(
            answer.error_message(),
            row.message,
            "case {:?}: expected the published message {:?}, got {:?}",
            row.case,
            row.message,
            answer.error_message()
        );
        let read = get_object_tagging(&router, bucket, key, "").await;
        assert_eq!(
            tag_set_of(&read),
            held,
            "case {:?}: a refused put must leave the stored tag set untouched",
            row.case
        );
    }

    // The SAME restrictions on a BUCKET's tag set carry the same sentences, with the
    // bucket's own published count in the one that states a maximum.
    let bucket_rows: Vec<TagRow> = vec![
        TagRow {
            case: format!("{} tags on a bucket", MAX_TAGS_PER_BUCKET + 1),
            tags: (0..=MAX_TAGS_PER_BUCKET)
                .map(|index| tag(&format!("key{index}"), "value"))
                .collect(),
            code: "BadRequest",
            message: "Bucket tags cannot be greater than 50",
        },
        TagRow {
            case: "the same key twice on a bucket".to_string(),
            tags: vec![tag("dup", "a"), tag("dup", "b")],
            code: "InvalidTag",
            message: "Cannot provide multiple Tags with the same key",
        },
        TagRow {
            case: "a bucket tag value outside the class".to_string(),
            tags: vec![tag("key", "value\u{1f600}")],
            code: "InvalidTag",
            message: "The TagValue you have provided is invalid",
        },
    ];
    for row in &bucket_rows {
        let answer = put_bucket_tagging(&router, bucket, &row.tags).await;
        assert_eq!(
            answer.error_code(),
            row.code,
            "case {:?}: expected the error code {:?}, got {:?} with status {} and body {:?}",
            row.case,
            row.code,
            answer.error_code(),
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_message(),
            row.message,
            "case {:?}: expected the published message {:?}, got {:?}",
            row.case,
            row.message,
            answer.error_message()
        );
    }

    // A tag set the published restrictions ALLOW is still stored, so the rows above are
    // about what they carried: the count at exactly the object's maximum, with every
    // character class the published class does hold.
    let at_limit: Vec<(String, String)> = (0..MAX_TAGS_PER_OBJECT)
        .map(|index| tag(&format!("key.{index}"), "value-1 2:3/4@5_6=7+8"))
        .collect();
    let answer = put_object_tagging(&router, bucket, key, &at_limit).await;
    assert_eq!(
        answer.status, 200,
        "a tag set at the published maximum must be stored, got {} with body {:?}",
        answer.status,
        text(&answer)
    );
    let read = get_object_tagging(&router, bucket, key, "").await;
    assert_eq!(
        tag_set_of(&read),
        at_limit,
        "the accepted tag set must read back unchanged"
    );
    // The payload builder is the shared one, so this suite cannot drift from the others on
    // how a `Tagging` document is spelled.
    assert!(
        tagging_payload(&at_limit).contains("<TagSet>"),
        "the shared payload builder must carry the TagSet container"
    );
}

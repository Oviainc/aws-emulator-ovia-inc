// SPDX-License-Identifier: CC0-1.0

//! Extra cover for the tag set a WRITE carries: `PutObject`'s `x-amz-tagging` header.
//!
//! The rule, asserted over a SET of headers rather than one request: a write that supplies
//! object tags stores them and succeeds exactly as an untagged write does (HTTP 200 with
//! the object's `ETag`), the stored set is the header read as URL query parameters —
//! `Tagging`'s own documented encoding, "The tag-set must be encoded as URL Query
//! parameters. (For example, \"Key1=Value1\")" — and every reader reports it: the whole set
//! through `GetObjectTagging`, its size through the `TagCount` of `GetObject` and
//! `HeadObject`. A header outside the published restrictions is refused (HTTP 400) and
//! leaves the key exactly as it was.
//!
//! The rows differ along what the rule generalises over: how many pairs the header carries
//! (none, one, three, the published ten), how each side is encoded (plain,
//! percent-escaped, `+` for a space, an escaped `+`, a value containing the `=`
//! separator), the size of a key and a value (empty value, and each at its published
//! bound), and whether a later write carries a header at all. The modelled `Tagging`
//! container path (`PutObjectTagging` / `GetObjectTagging` / `DeleteObjectTagging`) is
//! covered by `object_tagging.rs` beside this suite; what is here is the HEADER on the
//! write.

use service_com_amazonaws_s3_amazons3::state::{
    object_etag, MAX_TAGS_PER_OBJECT, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, send_bytes, Answer};
use super::bucket_tagging::tag_set_of;

/// The bucket every row writes into.
const BUCKET: &str = "tagged-on-put";

/// The object key every row writes.
const KEY: &str = "artifacts/bundle.zip";

/// The payload every row writes; its digest is what the write reports back as the `ETag`.
const BODY: &[u8] = b"an object that carries tags";

/// `PUT /<bucket>/<key>` — `PutObject`, with `x-amz-tagging` when the row carries one.
async fn put_object(router: &MultiServiceRouter, tagging: Option<&str>) -> Answer {
    let mut headers: Vec<(&str, &str)> = vec![("content-type", "application/zip")];
    if let Some(tagging) = tagging {
        headers.push(("x-amz-tagging", tagging));
    }
    send_bytes(router, "PUT", &format!("/{BUCKET}/{KEY}"), &headers, BODY).await
}

/// `GET /<bucket>/<key>?tagging` — `GetObjectTagging`.
async fn get_object_tagging(router: &MultiServiceRouter) -> Answer {
    send(router, "GET", &format!("/{BUCKET}/{KEY}?tagging"), &[], "").await
}

/// `GET /<bucket>/<key>` — `GetObject`.
async fn get_object(router: &MultiServiceRouter) -> Answer {
    send(router, "GET", &format!("/{BUCKET}/{KEY}"), &[], "").await
}

/// `HEAD /<bucket>/<key>` — `HeadObject`.
async fn head_object(router: &MultiServiceRouter) -> Answer {
    send(router, "HEAD", &format!("/{BUCKET}/{KEY}"), &[], "").await
}

/// A bucket with nothing in it, ready for one row's write.
async fn bucket() -> MultiServiceRouter {
    let router = router();
    let created = create_bucket(&router, BUCKET, None, None).await;
    assert_eq!(
        created.status,
        200,
        "the test bucket must be created, got {} with {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    router
}

/// One accepted header, and the tag set it must store.
struct Accepted {
    name: &'static str,
    /// The `x-amz-tagging` header value, or `None` for a write that carries none.
    tagging: Option<String>,
    /// The `(Key, Value)` pairs the stored set must hold, in order.
    tags: Vec<(String, String)>,
}

/// The accepted rows: the class of headers a client sends, not one recorded example.
fn accepted() -> Vec<Accepted> {
    let many: Vec<(String, String)> = (0..MAX_TAGS_PER_OBJECT)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();
    vec![
        Accepted {
            name: "no x-amz-tagging header at all",
            tagging: None,
            tags: Vec::new(),
        },
        Accepted {
            name: "one pair",
            tagging: Some("sim2real=true".to_string()),
            tags: vec![("sim2real".to_string(), "true".to_string())],
        },
        Accepted {
            name: "three pairs, in the order the header carries them",
            tagging: Some("stage=build&owner=agents&tier=2".to_string()),
            tags: vec![
                ("stage".to_string(), "build".to_string()),
                ("owner".to_string(), "agents".to_string()),
                ("tier".to_string(), "2".to_string()),
            ],
        },
        Accepted {
            name: "the published maximum number of tags",
            tagging: Some(
                many.iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join("&"),
            ),
            tags: many,
        },
        Accepted {
            // URL query encoding: `%XX` is an escaped byte and `+` is a space.
            name: "percent-escaped and plus-encoded values",
            tagging: Some("path=%2Fdeploy%2Fbundle.zip&note=hello+world".to_string()),
            tags: vec![
                ("path".to_string(), "/deploy/bundle.zip".to_string()),
                ("note".to_string(), "hello world".to_string()),
            ],
        },
        Accepted {
            // A percent-escaped `+` is a literal `+`, not a space.
            name: "an escaped plus sign",
            tagging: Some("version=1%2B2".to_string()),
            tags: vec![("version".to_string(), "1+2".to_string())],
        },
        Accepted {
            // A query string splits each pair at its FIRST `=`, so a value may hold one.
            name: "a value containing the separator",
            tagging: Some("expression=a=b".to_string()),
            tags: vec![("expression".to_string(), "a=b".to_string())],
        },
        Accepted {
            name: "an empty value",
            tagging: Some("marker=".to_string()),
            tags: vec![("marker".to_string(), String::new())],
        },
        Accepted {
            name: "a key and a value at their published bounds",
            tagging: Some(format!(
                "{}={}",
                "k".repeat(MAX_TAG_KEY_CHARS),
                "v".repeat(MAX_TAG_VALUE_CHARS)
            )),
            tags: vec![(
                "k".repeat(MAX_TAG_KEY_CHARS),
                "v".repeat(MAX_TAG_VALUE_CHARS),
            )],
        },
    ]
}

#[tokio::test]
async fn a_tagged_write_succeeds_and_every_reader_reports_the_stored_tag_set() {
    for case in accepted() {
        let Accepted {
            name,
            tagging,
            tags,
        } = case;
        let router = bucket().await;

        let written = put_object(&router, tagging.as_deref()).await;
        let body = String::from_utf8_lossy(&written.body).to_string();
        assert_eq!(
            written.status, 200,
            "case {name:?}: a write that carries object tags succeeds exactly as an \
             untagged one does, got {} with {body:?}",
            written.status
        );
        assert_eq!(
            written.header("etag"),
            Some(object_etag(BODY)),
            "case {name:?}: the write must report the object's ETag, got {:?}",
            written.header("etag")
        );

        let reported = get_object_tagging(&router).await;
        let reported_body = String::from_utf8_lossy(&reported.body).to_string();
        assert_eq!(
            reported.status, 200,
            "case {name:?}: GetObjectTagging must report the stored set, got {} with \
             {reported_body:?}",
            reported.status
        );
        assert_eq!(
            tag_set_of(&reported),
            tags,
            "case {name:?}: expected the tag set {tags:?} the header stored, got \
             {reported_body:?}"
        );

        // "The number of tags, if any, on the object" — absent, not zero, for an object
        // that carries none.
        let expected_count = (!tags.is_empty()).then(|| tags.len().to_string());
        let read = get_object(&router).await;
        assert_eq!(
            (read.status, read.header("x-amz-tagging-count")),
            (200, expected_count.clone()),
            "case {name:?}: GetObject must count the stored tags, got {} with {:?}",
            read.status,
            read.header("x-amz-tagging-count")
        );
        let headed = head_object(&router).await;
        assert_eq!(
            (headed.status, headed.header("x-amz-tagging-count")),
            (200, expected_count),
            "case {name:?}: HeadObject must count the stored tags, got {} with {:?}",
            headed.status,
            headed.header("x-amz-tagging-count")
        );
    }
}

/// One row of the replacement table: the case name, the `x-amz-tagging` header that write
/// carries (when any), and the tag set the object then holds.
type ReplacementRow = (&'static str, Option<&'static str>, Vec<(String, String)>);

/// A write REPLACES the whole object, tags included: "You cannot use PutObject to only
/// update a single piece of metadata for an existing object", so a second write without an
/// `x-amz-tagging` header leaves the object with no tags, and one with a different header
/// leaves exactly that set.
#[tokio::test]
async fn a_second_write_replaces_the_tag_set() {
    let router = bucket().await;

    let writes: [ReplacementRow; 4] = [
        (
            "first write, two tags",
            Some("stage=build&owner=agents"),
            vec![
                ("stage".to_string(), "build".to_string()),
                ("owner".to_string(), "agents".to_string()),
            ],
        ),
        (
            "second write, a different single tag",
            Some("stage=release"),
            vec![("stage".to_string(), "release".to_string())],
        ),
        ("third write, no header", None, Vec::new()),
        (
            "fourth write, tagged again",
            Some("stage=rollback"),
            vec![("stage".to_string(), "rollback".to_string())],
        ),
    ];

    for (case, tagging, expected) in writes {
        let written = put_object(&router, tagging).await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: the write must succeed, got {} with {:?}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        let reported = get_object_tagging(&router).await;
        assert_eq!(
            tag_set_of(&reported),
            expected,
            "case {case:?}: expected the tag set {expected:?} after the write, got {:?}",
            String::from_utf8_lossy(&reported.body)
        );
    }
}

/// A header outside the published restrictions is refused with HTTP 400 — not a 501, and
/// not a silent 200 that drops the tags — and nothing is stored under the key.
#[tokio::test]
async fn a_header_outside_the_published_restrictions_is_refused() {
    // (case, the header value)
    let refused: [(&str, String); 4] = [
        (
            "one tag past the published maximum",
            (0..=MAX_TAGS_PER_OBJECT)
                .map(|index| format!("key{index}=value{index}"))
                .collect::<Vec<_>>()
                .join("&"),
        ),
        (
            "a key one character past its bound",
            format!("{}=v", "k".repeat(MAX_TAG_KEY_CHARS + 1)),
        ),
        (
            "a value one character past its bound",
            format!("key={}", "v".repeat(MAX_TAG_VALUE_CHARS + 1)),
        ),
        (
            "one key named twice",
            "stage=build&stage=release".to_string(),
        ),
    ];

    for (case, tagging) in refused {
        let router = bucket().await;
        let written = put_object(&router, Some(&tagging)).await;
        let body = String::from_utf8_lossy(&written.body).to_string();
        assert_eq!(
            written.status, 400,
            "case {case:?}: expected the 400 refusal, got {} with {body:?}",
            written.status
        );

        // Refused before anything was written: the key holds no object at all, which
        // `GetObjectTagging` reports as the service's `NoSuchKey`.
        let reported = get_object_tagging(&router).await;
        assert_eq!(
            (reported.status, reported.error_code()),
            (404, "NoSuchKey".to_string()),
            "case {case:?}: the refused write must leave the key empty, got {} with {:?}",
            reported.status,
            String::from_utf8_lossy(&reported.body)
        );
    }
}

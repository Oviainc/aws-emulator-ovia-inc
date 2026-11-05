// SPDX-License-Identifier: CC0-1.0

//! A tag key may not begin with the prefix AWS keeps for the tags it generates itself.
//!
//! The rule under test: a tag set carrying a key that begins with `aws:` — the prefix the
//! published tag restrictions reserve for AWS-generated tags, which a caller may therefore
//! neither add nor edit — is refused with HTTP 400, the error code `InvalidTag` and the
//! sentence S3 states for that reservation on the tag set's OWNER's tagging path, with the
//! offending key in the `<TagKey>` member of the error document, and the resource keeps the
//! tag set it already had. The two paths word that one refusal differently: a BUCKET tag set
//! is refused as a write of a system tag, an OBJECT tag set as a key carrying the reserved
//! prefix. The prefix itself comes from the shared tag-set rule
//! (`service_com_amazonaws_s3_amazons3::state::RESERVED_TAG_KEY_PREFIX`), so a row is built
//! from the published prefix rather than from a literal spelled here twice.
//!
//! The tables span what the rule generalises over:
//!
//! * the PATH the tag set arrives on — the modelled `Tagging` payload of
//!   `PutObjectTagging`, the same payload on `PutBucketTagging`, and the URL-encoded
//!   `x-amz-tagging` header a write carries — because the reservation is a rule of the tag
//!   set, not of one operation;
//! * the FORM of the reserved key (the prefix alone, a one-segment key, a key of several
//!   colon-separated segments, an over-long one) and its POSITION in the set (first, last,
//!   among keys that are fine);
//! * keys the rule ADMITS: the same letters without the colon, the prefix somewhere other
//!   than the start, and a key that merely contains a colon.
//!
//! Every assertion drives a real HTTP request through the router (the harness
//! [`super::bucket_lifecycle`] owns), so what is pinned is the wire document.

use service_com_amazonaws_s3_amazons3::state::{MAX_TAG_KEY_CHARS, RESERVED_TAG_KEY_PREFIX};

use super::bucket_lifecycle::router;
use super::bucket_tagging::{get_bucket_tagging, put_bucket_tagging, tag, tag_set_of};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_tagging::{get_object_tagging, object, put_object_tagging};

/// The code S3 refuses a tag that does not pass input validation with.
const INVALID_TAG: &str = "InvalidTag";

/// The fixed text S3's OBJECT tagging path states for a key carrying the reserved prefix.
const RESERVED_PREFIX_MESSAGE: &str = "Your TagKey cannot be prefixed with aws:";

/// The fixed text S3's BUCKET tagging path states for that same key: the other wording of
/// the one refusal, about the system TAGS a requester may not write rather than about the
/// key's spelling.
const SYSTEM_TAGS_MESSAGE: &str = "System tags cannot be added/updated by requester";

/// The status a served `PutBucketTagging` answers: the 204 its model states for it.
const BUCKET_TAGGING_SERVED: u16 = 204;

/// The status a served `PutObjectTagging` — and an object write carrying the tag set in its
/// `x-amz-tagging` header — answers: the 200 their models state for them.
const OBJECT_TAGGING_SERVED: u16 = 200;

/// One row: the case, the tag set the request carries, and the key the error document must
/// name — `None` for a set the rule admits.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    tags: Vec<(String, String)>,
    offending: Option<String>,
}

/// Every row of the reserved-prefix table.
///
/// The reserved keys are spelled from the published prefix itself, so the table cannot
/// drift from the rule the handlers judge a tag set by.
fn rows() -> Vec<Row> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    let refused = |case: &str, tags: Vec<(String, String)>, offending: &str| Row {
        case: case.to_string(),
        tags,
        offending: Some(offending.to_string()),
    };
    let admitted = |case: &str, tags: Vec<(String, String)>| Row {
        case: case.to_string(),
        tags,
        offending: None,
    };
    let long_reserved = format!(
        "{prefix}{}",
        "k".repeat(MAX_TAG_KEY_CHARS - prefix.chars().count())
    );
    vec![
        refused(
            "the reserved prefix with one segment after it",
            vec![tag(&format!("{prefix}reserved"), "v")],
            &format!("{prefix}reserved"),
        ),
        refused(
            "the reserved prefix and nothing else",
            vec![tag(prefix, "v")],
            prefix,
        ),
        refused(
            "a reserved key of several colon-separated segments",
            vec![tag(&format!("{prefix}cloudformation:stack-name"), "a-stack")],
            &format!("{prefix}cloudformation:stack-name"),
        ),
        refused(
            "a reserved key LAST among keys that are fine",
            vec![
                tag("owner", "platform"),
                tag("cost-centre", "7"),
                tag(&format!("{prefix}createdBy"), "someone"),
            ],
            &format!("{prefix}createdBy"),
        ),
        refused(
            "a reserved key FIRST, before keys that are fine",
            vec![
                tag(&format!("{prefix}autoscaling"), "group"),
                tag("owner", "platform"),
            ],
            &format!("{prefix}autoscaling"),
        ),
        // A key at the published length bound is one the key rules admit, so what refuses
        // it is the reservation.
        refused(
            "a reserved key exactly at the published key length",
            vec![tag(&long_reserved, "v")],
            &long_reserved,
        ),
        admitted(
            "the same letters without the colon",
            vec![tag("awsome-service", "v")],
        ),
        admitted(
            "the prefix somewhere other than the start of the key",
            vec![tag(&format!("team-{prefix}notes"), "v")],
        ),
        admitted(
            "a colon in a key that does not carry the prefix",
            vec![tag("team:notes", "v")],
        ),
    ]
}

/// The `x-amz-tagging` header value that encodes `tags`: "the tag-set must be encoded as
/// URL Query parameters" (`PutObjectRequest$Tagging`).
fn tagging_header(tags: &[(String, String)]) -> String {
    tags.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

#[tokio::test]
async fn a_tag_key_carrying_the_reserved_prefix_is_refused_on_every_path_a_tag_set_arrives_on() {
    let router = router();
    let bucket_name = "reserved-tag-prefix";
    bucket(&router, bucket_name).await;
    object(&router, bucket_name, "tagged.csv").await;

    // The tag sets the refusals must leave exactly as they are: one on the object, one on
    // the bucket.
    let stored = vec![tag("keep", "me"), tag("and", "me-too")];
    for (call, answer, served) in [
        (
            "PutObjectTagging",
            put_object_tagging(&router, bucket_name, "tagged.csv", &stored).await,
            OBJECT_TAGGING_SERVED,
        ),
        (
            "PutBucketTagging",
            put_bucket_tagging(&router, bucket_name, &stored).await,
            BUCKET_TAGGING_SERVED,
        ),
    ] {
        assert_eq!(
            answer.status, served,
            "the setup {call} must succeed, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
    }

    for row in rows() {
        let Row {
            case,
            tags,
            offending,
        } = &row;

        // The three paths a tag set reaches the store on, each judged by the same rule.
        let object_tagging = put_object_tagging(&router, bucket_name, "tagged.csv", tags).await;
        let bucket_tagging = put_bucket_tagging(&router, bucket_name, tags).await;
        let on_write = put_object(
            &router,
            bucket_name,
            "written.csv",
            b"a,b\n1,2\n",
            &[("x-amz-tagging", &tagging_header(tags))],
        )
        .await;

        for (call, answer, served, message) in [
            (
                "PutObjectTagging",
                &object_tagging,
                OBJECT_TAGGING_SERVED,
                RESERVED_PREFIX_MESSAGE,
            ),
            (
                "PutBucketTagging",
                &bucket_tagging,
                BUCKET_TAGGING_SERVED,
                SYSTEM_TAGS_MESSAGE,
            ),
            (
                "PutObject with x-amz-tagging",
                &on_write,
                OBJECT_TAGGING_SERVED,
                RESERVED_PREFIX_MESSAGE,
            ),
        ] {
            match offending {
                None => {
                    assert_eq!(
                        answer.status, served,
                        "case {case:?}/{call}: the rule admits this tag set, expected HTTP \
                         {served}, got {} with body {:?}",
                        answer.status,
                        text(answer)
                    );
                }
                Some(key) => {
                    assert_eq!(
                        answer.status, 400,
                        "case {case:?}/{call}: expected HTTP 400, got {} with body {:?}",
                        answer.status,
                        text(answer)
                    );
                    assert_eq!(
                        answer.error_code(),
                        INVALID_TAG,
                        "case {case:?}/{call}: expected the error code {INVALID_TAG:?}, got \
                         {:?}",
                        answer.error_code()
                    );
                    assert_eq!(
                        answer.error_message(),
                        message,
                        "case {case:?}/{call}: expected the message {message:?}, got {:?}",
                        answer.error_message()
                    );
                    assert_eq!(
                        answer.element("TagKey").as_deref(),
                        Some(key.as_str()),
                        "case {case:?}/{call}: the error document must name the offending \
                         key, got {:?} in {:?}",
                        answer.element("TagKey"),
                        text(answer)
                    );
                }
            }
        }

        // What the two resources hold afterwards: the admitted set replaced the stored one
        // (a `Tagging` container is a replacement), and a refused one changed nothing.
        let expected: Vec<(String, String)> = match offending {
            None => tags.clone(),
            Some(_) => stored.clone(),
        };
        let on_object = get_object_tagging(&router, bucket_name, "tagged.csv", "").await;
        let on_bucket = get_bucket_tagging(&router, bucket_name).await;
        for (call, answer) in [
            ("GetObjectTagging", &on_object),
            ("GetBucketTagging", &on_bucket),
        ] {
            assert_eq!(
                answer.status, 200,
                "case {case:?}/{call}: the read must be served, got {} with body {:?}",
                answer.status,
                text(answer)
            );
            assert_eq!(
                tag_set_of(answer),
                expected,
                "case {case:?}/{call}: expected the tag set {expected:?}, got {:?}",
                tag_set_of(answer)
            );
        }

        // Put the stored sets back, so the next row is judged against the same state
        // whichever way this one went.
        for (answer, served) in [
            (
                put_object_tagging(&router, bucket_name, "tagged.csv", &stored).await,
                OBJECT_TAGGING_SERVED,
            ),
            (
                put_bucket_tagging(&router, bucket_name, &stored).await,
                BUCKET_TAGGING_SERVED,
            ),
        ] {
            assert_eq!(
                answer.status, served,
                "case {case:?}: restoring the stored tag set must succeed, got {} with body \
                 {:?}",
                answer.status,
                text(&answer)
            );
        }
    }
}

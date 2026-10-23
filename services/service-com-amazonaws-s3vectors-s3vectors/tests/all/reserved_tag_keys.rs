// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE tag rule the model cannot express: the `aws:`
//! tag-key namespace is AWS's own, so a caller-supplied key in it is refused
//! wherever a caller names tag keys — `TagResource` (`POST /tags/{resourceArn}`),
//! `UntagResource` (`DELETE /tags/{resourceArn}?tagKeys=…`),
//! `CreateVectorBucket$tags` and `CreateIndex$tags`.
//!
//! RULE: a caller-supplied tag key whose first four characters are `aws:` (in any
//! case) is refused with `ValidationException` (400), and the request stores,
//! creates and removes nothing — not even the well-formed keys beside it.
//!
//! What lives where: the accepted side of the same line (the `TagKey`/`TagValue`
//! class, including a key that merely CONTAINS `aws:`) is `resource_tags.rs`; the
//! write/remove semantics, the 50-tag ceiling, ARN resolution and tag lifetime
//! are `resource_tagging.rs`. This suite shares their harness
//! (`vector_lifecycle.rs`) and fixture (`resource_tagging::resource_with_tags`).
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over:
//! the spelling and case of the reserved prefix, the operation the key arrives
//! on, and the kind of resource being tagged. Every case drives real HTTP through
//! the production front door (`MultiServiceRouter::dispatch`), so what is
//! asserted is what a client sees.

use serde_json::json;
use service_com_amazonaws_s3vectors_s3vectors::state::RESERVED_TAG_KEY_PREFIX;

use crate::resource_tagging::{resource_with_tags, Kind, BUCKET, INDEX};
use crate::vector_lifecycle::{
    call, error_code, expected_bucket_arn, expected_index_arn, keys, pairs, reported_tags, router,
    tag, tag_object, untag,
};

/// The spellings of the reserved prefix a refusal must cover.
///
/// The prefix comes from the AWS tagging reference ("the `aws:` prefix is
/// reserved for AWS use"), which `RESERVED_TAG_KEY_PREFIX` carries. The rows
/// include a key AWS itself generates, which the issue did not cite, and the
/// case-insensitive forms.
fn reserved_keys() -> Vec<(&'static str, &'static str)> {
    let rows: Vec<(&'static str, &'static str)> = vec![
        ("the bare reserved prefix as the whole key", "aws:"),
        ("a reserved key", "aws:cost-allocation"),
        (
            "a reserved key AWS itself generates",
            "aws:cloudformation:stack-name",
        ),
        ("the prefix in upper case", "AWS:billing"),
        ("the prefix in mixed case", "Aws:Billing"),
    ];
    assert!(
        rows.iter()
            .all(|(_, key)| key.to_ascii_lowercase().starts_with(RESERVED_TAG_KEY_PREFIX)),
        "every row must spell the reserved prefix {RESERVED_TAG_KEY_PREFIX:?}"
    );
    rows
}

/// A tag key in the reserved namespace is refused — with `ValidationException`
/// and with nothing stored, created or removed — wherever a caller names tag
/// keys: TagResource, UntagResource, CreateVectorBucket and CreateIndex.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_wherever_a_caller_names_tag_keys() {
    for (case, key) in reserved_keys() {
        // An untagged vector bucket holding an untagged vector index.
        let router = router();
        resource_with_tags(&router, Kind::Index, &[]).await;
        let bucket_arn = expected_bucket_arn(BUCKET);
        let index_arn = expected_index_arn(BUCKET, INDEX);
        let tags = tag_object(&pairs(&[(key, "x"), ("env", "dev")]));

        // TagResource refuses it, on either kind of resource, and stores none of
        // the request — not even the well-formed pair beside it.
        for arn in [&bucket_arn, &index_arn] {
            let (status, body) = tag(&router, arn, &tags).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: TagResource must refuse the reserved key on {arn}, \
                 got {status} {body}"
            );
            let reported = reported_tags(&router, case, arn).await;
            assert_eq!(
                reported,
                tag_object(&[]),
                "case {case:?}: a refused TagResource must store nothing on {arn}, \
                 got {reported}"
            );
        }

        // UntagResource refuses it too: a reserved tag is no more deletable by a
        // caller than it is creatable by one.
        for arn in [&bucket_arn, &index_arn] {
            let (status, body) = untag(&router, arn, &keys(&[key, "env"])).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: UntagResource must refuse the reserved key on {arn}, \
                 got {status} {body}"
            );
        }

        // CreateVectorBucket refuses it, and creates no bucket.
        let (status, body) = call(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": "reserved-bucket", "tags": tags }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: CreateVectorBucket must refuse the reserved key, got {status} {body}"
        );
        let (status, body) = call(
            &router,
            "GetVectorBucket",
            &json!({ "vectorBucketName": "reserved-bucket" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "NotFoundException"),
            "case {case:?}: a refused CreateVectorBucket must create nothing, got {status} {body}"
        );

        // CreateIndex refuses it, and creates no index.
        let (status, body) = call(
            &router,
            "CreateIndex",
            &json!({
                "vectorBucketName": BUCKET,
                "indexName": "reserved-index",
                "dataType": "float32",
                "dimension": 2,
                "distanceMetric": "cosine",
                "tags": tags,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: CreateIndex must refuse the reserved key, got {status} {body}"
        );
        let (status, body) = call(
            &router,
            "GetIndex",
            &json!({ "vectorBucketName": BUCKET, "indexName": "reserved-index" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "NotFoundException"),
            "case {case:?}: a refused CreateIndex must create nothing, got {status} {body}"
        );
    }
}

/// A reserved key is refused BEFORE anything is written, so a request that mixes
/// one into the set a resource already carries leaves that set exactly as it was
/// — for a write and for a removal.
#[tokio::test]
async fn a_refused_request_leaves_an_already_tagged_resource_unchanged() {
    let established = pairs(&[("env", "dev"), ("team", "search")]);

    for kind in [Kind::Bucket, Kind::Index] {
        let router = router();
        let arn = resource_with_tags(&router, kind, &established).await;

        for (case, applied) in [
            (
                "a reserved key beside a new key",
                pairs(&[("aws:billing", "x"), ("owner", "platform")]),
            ),
            (
                "a reserved key beside an update of a key already carried",
                pairs(&[("AWS:billing", "x"), ("env", "prod")]),
            ),
        ] {
            let (status, body) = tag(&router, &arn, &tag_object(&applied)).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?} on {kind:?}: the write must be refused, got {status} {body}"
            );
            let reported = reported_tags(&router, case, &arn).await;
            assert_eq!(
                reported,
                tag_object(&established),
                "case {case:?} on {kind:?}: a refused write must store nothing, got {reported}"
            );
        }

        let (status, body) = untag(&router, &arn, &keys(&["Aws:billing", "env"])).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "on {kind:?}: a removal naming a reserved key must be refused, got {status} {body}"
        );
        let reported = reported_tags(&router, "a refused removal", &arn).await;
        assert_eq!(
            reported,
            tag_object(&established),
            "on {kind:?}: a refused removal must remove nothing, got {reported}"
        );
    }
}

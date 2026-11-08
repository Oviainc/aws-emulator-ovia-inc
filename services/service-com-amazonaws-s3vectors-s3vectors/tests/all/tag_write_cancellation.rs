// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what becomes of a tag mutation's optimistic lock when the request
//! holding it GOES AWAY.
//!
//! RULE: the lock a tag mutation takes on one resource is held for as long as that mutation
//! is in flight and no longer. A mutation commits before its own caller is answered, so a
//! caller that waits reads committed state (`tag_write_conflict.rs` pins the overlapping
//! case, which is the other half of the same rule) — and a request that is DROPPED while
//! its write is committing is not in flight any more, so the next mutation of that resource
//! is served rather than told about a conflict with a write nobody is making.
//!
//! An `.await` is where a dropped request is noticed: the commit span is awaited inside the
//! handler, so a caller that goes away during it (a client that disconnected, a wait that
//! ran out) drops the handler's future there. Each row below drops the request at that
//! point, with a deadline shorter than the commit span
//! (`service_...::state::TAG_WRITE_COMMIT`), and then mutates the same resource
//! IMMEDIATELY — before that span would have elapsed — so a lock that outlived its request
//! would be visible as a `ConflictException`.
//!
//! The rows differ along what the rule generalises over: which mutation was dropped
//! (`TagResource`, `UntagResource`), which mutation follows it, and the KIND of resource
//! addressed (vector bucket, vector index).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_s3vectors_s3vectors::state::TAG_WRITE_COMMIT;

use crate::vector_lifecycle::{
    encode_label, error_code, expected_bucket_arn, expected_index_arn, keys, ok, reported_tags,
    router, tag, tag_object, untag, AUTHORIZATION, REGION,
};

/// The bucket and index every row addresses.
const BUCKET: &str = "cancelled-write-bucket";
/// See [`BUCKET`].
const INDEX: &str = "cancelled-write-index";

/// How long a dropped request is given before it is dropped: well inside the commit span,
/// so the request is abandoned while its write is still committing.
const DROPPED_AFTER: Duration = Duration::from_millis(40);

/// Which mutation a row fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mutation {
    /// `TagResource` of one key.
    Write,
    /// `UntagResource` of one key.
    Remove,
}

impl Mutation {
    fn operation(self) -> &'static str {
        match self {
            Self::Write => "TagResource",
            Self::Remove => "UntagResource",
        }
    }

    /// The method and URI this mutation is bound to, and the body it carries.
    fn request(self, arn: &str, key: &str, value: &str) -> (&'static str, String, Option<Value>) {
        match self {
            Self::Write => (
                "POST",
                format!("/tags/{}", encode_label(arn)),
                Some(json!({ "tags": { key: value } })),
            ),
            Self::Remove => (
                "DELETE",
                format!("/tags/{}?tagKeys={}", encode_label(arn), encode_label(key)),
                None,
            ),
        }
    }
}

/// Fire `mutation` and DROP it before it can commit: the dispatch future is given a
/// deadline shorter than the commit span, and dropping it is what cancels the handler.
///
/// Answers the elapsed error the deadline produced, so a row asserts the request really was
/// abandoned rather than served.
async fn dropped_midway(router: &MultiServiceRouter, mutation: Mutation, arn: &str, key: &str) {
    let (method, path_and_query, body) = mutation.request(arn, key, "dropped");
    let payload = match body {
        Some(body) => {
            bytes::Bytes::from(serde_json::to_vec(&body).expect("the request body serializes"))
        }
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://s3vectors.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let abandoned = tokio::time::timeout(DROPPED_AFTER, router.dispatch(request)).await;
    assert!(
        abandoned.is_err(),
        "case {}: the request must still be committing when it is dropped, it answered \
         already",
        mutation.operation()
    );
}

/// Fire `mutation` and read its status and body — a caller that waits for its answer.
async fn mutate(
    router: &MultiServiceRouter,
    mutation: Mutation,
    arn: &str,
    key: &str,
    value: &str,
) -> (u16, Value) {
    match mutation {
        Mutation::Write => tag(router, arn, &json!({ key: value })).await,
        Mutation::Remove => untag(router, arn, &keys(&[key])).await,
    }
}

/// A fresh router holding a bucket and an index in it, each carrying the key a removal row
/// removes.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": BUCKET, "tags": { "team": "web" } }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "cosine",
            "tags": { "team": "web" },
        }),
    )
    .await;
    router
}

/// A tag mutation whose request is dropped while it commits leaves the resource's lock
/// released: the next mutation of the same resource is served at once, and it is the one
/// whose effect the resource carries.
#[tokio::test]
async fn a_dropped_mutation_leaves_the_next_one_free_to_commit() {
    assert!(
        DROPPED_AFTER < TAG_WRITE_COMMIT,
        "the dropped request must be abandoned inside the commit span \
         ({DROPPED_AFTER:?} must be shorter than {TAG_WRITE_COMMIT:?})"
    );

    // (case, the mutation that is dropped, the mutation that follows it)
    let rows: [(&str, Mutation, Mutation); 4] = [
        ("a dropped write, then a write", Mutation::Write, Mutation::Write),
        ("a dropped write, then a removal", Mutation::Write, Mutation::Remove),
        ("a dropped removal, then a write", Mutation::Remove, Mutation::Write),
        (
            "a dropped removal, then a removal",
            Mutation::Remove,
            Mutation::Remove,
        ),
    ];

    for (case, dropped, next) in rows {
        for arn in [
            expected_bucket_arn(BUCKET),
            expected_index_arn(BUCKET, INDEX),
        ] {
            let router = fixture().await;
            dropped_midway(&router, dropped, &arn, "team").await;

            // Immediately — inside the span the dropped request's own lock would have
            // covered — the next mutation of that resource is served.
            let (status, body) = mutate(&router, next, &arn, "team", "committed").await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (200, ""),
                "case {case:?} on {arn}: {} must be served after the dropped {}, got \
                 {status} {body}",
                next.operation(),
                dropped.operation()
            );

            // And the set the resource carries is the one that mutation left: the dropped
            // request wrote nothing.
            let expected = match next {
                Mutation::Write => tag_object(&[("team".to_string(), "committed".to_string())]),
                Mutation::Remove => tag_object(&[]),
            };
            let reported = reported_tags(&router, case, &arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?} on {arn}: expected the tags the served mutation left, got \
                 {reported}"
            );
        }
    }
}

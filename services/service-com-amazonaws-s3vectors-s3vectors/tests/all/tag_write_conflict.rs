// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the optimistic concurrency control over one resource's tags,
//! served over the real `restJson1` wire through the router production uses.
//!
//! The rule every row asserts: the tag state of ONE resource is updated under optimistic
//! concurrency control, so when two tag mutations of the same resource overlap, at least
//! one of them is refused with `ConflictException` (HTTP 409) rather than both being
//! serialised silently — and the write that was refused leaves nothing behind. A mutation
//! commits before its own caller is answered, so mutations a caller issues one after the
//! other never overlap and are never refused; mutations of DIFFERENT resources never
//! contend either, because the lock is the resource's.
//!
//! The rows differ along what the rule generalises over: which pair of mutations overlaps
//! (`TagResource`/`TagResource`, `TagResource`/`UntagResource`, `UntagResource`/
//! `UntagResource`), which kind of resource they address (a vector bucket, a vector
//! index), and how many callers overlap at once.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{
    error_code, expected_bucket_arn, expected_index_arn, keys, ok, reported_tags, router, tag,
    untag,
};

const BUCKET: &str = "tag-conflict-bucket";
const INDEX: &str = "tag-conflict-index";

/// Which kind of resource a row's mutations address.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Bucket,
    Index,
}

/// One mutation a row fires: a write of `tags`, or a removal of `keys`.
#[derive(Debug, Clone)]
enum Mutation {
    Write(&'static str, &'static str),
    Remove(&'static str),
}

impl Mutation {
    /// The operation name, for a case label.
    fn operation(&self) -> &'static str {
        match self {
            Self::Write(..) => "TagResource",
            Self::Remove(_) => "UntagResource",
        }
    }
}

/// Fire one mutation against `arn` and report its status and body.
async fn mutate(
    router: &MultiServiceRouter,
    arn: &str,
    mutation: &Mutation,
) -> (u16, Value) {
    match mutation {
        Mutation::Write(key, value) => tag(router, arn, &json!({ *key: *value })).await,
        Mutation::Remove(key) => untag(router, arn, &keys(&[key])).await,
    }
}

/// A fresh router with a bucket and an index in it, each carrying the two tags a removal
/// row removes.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": BUCKET, "tags": { "team": "web", "env": "test" } }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "dataType": "float32",
            "dimension": 3,
            "distanceMetric": "cosine",
            "tags": { "team": "web", "env": "test" },
        }),
    )
    .await;
    router
}

/// The ARN a row's mutations address.
fn arn_of(kind: Kind) -> String {
    match kind {
        Kind::Bucket => expected_bucket_arn(BUCKET),
        Kind::Index => expected_index_arn(BUCKET, INDEX),
    }
}

/// The rows: every pair of mutations, on both kinds of resource.
fn overlap_cases() -> Vec<(Kind, Mutation, Mutation)> {
    let mut cases = Vec::new();
    for kind in [Kind::Bucket, Kind::Index] {
        for (first, second) in [
            (Mutation::Write("owner", "a"), Mutation::Write("owner", "b")),
            (Mutation::Write("owner", "a"), Mutation::Remove("team")),
            (Mutation::Remove("team"), Mutation::Write("owner", "b")),
            (Mutation::Remove("team"), Mutation::Remove("env")),
        ] {
            cases.push((kind, first, second));
        }
    }
    cases
}

/// Two mutations of the same resource that OVERLAP: one is served and the other is
/// refused with the modelled 409, and only the served one's change is left behind.
#[tokio::test]
async fn overlapping_tag_mutations_of_one_resource_refuse_one_caller() {
    for (kind, first, second) in overlap_cases() {
        let case = format!(
            "{} then {} on a {:?} resource, overlapping",
            first.operation(),
            second.operation(),
            kind
        );
        let router = fixture().await;
        let arn = arn_of(kind);
        let before = reported_tags(&router, &case, &arn).await;

        // Both calls are in flight at once: the second reaches the store while the first
        // one's write is still committing.
        let (left, right) = ::tokio::join!(
            mutate(&router, &arn, &first),
            mutate(&router, &arn, &second)
        );

        let statuses = [left.0, right.0];
        let served = statuses.iter().filter(|status| **status == 200).count();
        let refused = statuses
            .iter()
            .filter(|status| **status == 409)
            .count();
        assert_eq!(
            (served, refused),
            (1, 1),
            "case {case:?}: exactly one caller is served and one refused, got {left:?} {right:?}"
        );
        for (status, body) in [&left, &right] {
            if *status == 409 {
                assert_eq!(
                    error_code(body),
                    "ConflictException",
                    "case {case:?}: the refusal must be the modelled conflict, got {body}"
                );
            }
        }

        // The refused write left nothing behind: the tag map is the one the SERVED
        // mutation alone leaves.
        let served_mutation = if left.0 == 200 { &first } else { &second };
        let expected = applied(&before, served_mutation);
        let after = reported_tags(&router, &case, &arn).await;
        assert_eq!(
            after, expected,
            "case {case:?}: only the served mutation may be applied, expected {expected}, \
             got {after}"
        );
    }
}

/// The tag map `mutation` leaves when it is applied to `before`.
fn applied(before: &Value, mutation: &Mutation) -> Value {
    let mut map = before.as_object().cloned().unwrap_or_default();
    match mutation {
        Mutation::Write(key, value) => {
            map.insert((*key).to_string(), Value::String((*value).to_string()));
        }
        Mutation::Remove(key) => {
            map.remove(*key);
        }
    }
    Value::Object(map)
}

/// The same mutations issued one after the other — each caller waiting for its own
/// response — are never refused, however fast they follow one another, and every one of
/// them is applied.
#[tokio::test]
async fn sequential_tag_mutations_of_one_resource_are_never_refused() {
    for (kind, first, second) in overlap_cases() {
        let case = format!(
            "{} then {} on a {:?} resource, in sequence",
            first.operation(),
            second.operation(),
            kind
        );
        let router = fixture().await;
        let arn = arn_of(kind);
        let before = reported_tags(&router, &case, &arn).await;

        let (status, body) = mutate(&router, &arn, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first mutation must be served, got {status} {body}"
        );
        let (status, body) = mutate(&router, &arn, &second).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second mutation must be served too, got {status} {body}"
        );

        let expected = applied(&applied(&before, &first), &second);
        let after = reported_tags(&router, &case, &arn).await;
        assert_eq!(
            after, expected,
            "case {case:?}: both mutations must be applied, expected {expected}, got {after}"
        );
    }
}

/// The lock is the RESOURCE's: mutations of different resources never contend, however
/// many of them overlap.
#[tokio::test]
async fn overlapping_mutations_of_different_resources_are_all_served() {
    let router = fixture().await;
    let bucket_arn = expected_bucket_arn(BUCKET);
    let index_arn = expected_index_arn(BUCKET, INDEX);

    let bucket_tags = json!({ "owner": "bucket" });
    let index_tags = json!({ "owner": "index" });
    let (bucket_write, index_write) = ::tokio::join!(
        tag(&router, &bucket_arn, &bucket_tags),
        tag(&router, &index_arn, &index_tags)
    );
    for (label, (status, body)) in [("the bucket", &bucket_write), ("the index", &index_write)] {
        assert_eq!(
            *status, 200,
            "{label}'s own mutation must be served, got {status} {body}"
        );
    }

    for (label, arn, owner) in [
        ("bucket", &bucket_arn, "bucket"),
        ("index", &index_arn, "index"),
    ] {
        let tags = reported_tags(&router, label, arn).await;
        assert_eq!(
            tags["owner"].as_str(),
            Some(owner),
            "the {label}'s write must be applied, got {tags}"
        );
    }
}

/// With MORE than two callers overlapping on one resource, exactly one is served and
/// every other is refused with the modelled 409 — the signal to retry its
/// read-modify-write.
#[tokio::test]
async fn a_whole_fan_out_of_overlapping_callers_leaves_exactly_one_served() {
    let router = fixture().await;
    let arn = expected_bucket_arn(BUCKET);

    // Five callers in flight at once, each writing its own value for one key.
    let write = |caller: usize| {
        let tags = json!({ "owner": format!("caller-{caller}") });
        let arn = arn.clone();
        let router = &router;
        async move { tag(router, &arn, &tags).await }
    };
    let (first, second, third, fourth, fifth) =
        ::tokio::join!(write(0), write(1), write(2), write(3), write(4));
    let answers = [first, second, third, fourth, fifth];

    let served = answers.iter().filter(|(status, _)| *status == 200).count();
    assert_eq!(
        served, 1,
        "exactly one of five overlapping callers is served, got {answers:?}"
    );
    for (status, body) in &answers {
        if *status != 200 {
            assert_eq!(
                (*status, error_code(body).as_str()),
                (409, "ConflictException"),
                "every other caller is refused with the conflict, got {status} {body}"
            );
        }
    }

    // And the one served caller's write is the one the resource carries.
    let tags = reported_tags(&router, "five overlapping callers", &arn).await;
    let owner = tags["owner"].as_str().unwrap_or_default().to_string();
    assert!(
        owner.starts_with("caller-"),
        "the served caller's write must be the stored one, got {tags}"
    );
}

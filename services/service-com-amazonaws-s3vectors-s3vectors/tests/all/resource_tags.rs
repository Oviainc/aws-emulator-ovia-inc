// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tag KEY AND VALUE CLASS an S3 Vectors resource
//! accepts — the character categories and length bounds of the model's
//! `TagsMap`, which is the class the reserved-`aws:`-prefix rule in
//! `reserved_tag_keys.rs` draws its line through.
//!
//! RULE (the accepted side of that rule): a caller-supplied tag key is ordinary
//! unless it STARTS with the reserved `aws:` prefix, so every key and value the
//! `TagKey`/`TagValue` class allows — including a key that merely CONTAINS
//! `aws:` — is stored by `TagResource` and read back unchanged by
//! `ListTagsForResource`, on a vector bucket and on a vector index alike.
//!
//! What lives where: the write/remove SEMANTICS (merge, idempotent removal), the
//! 50-tag ceiling, ARN resolution and tag lifetime are `resource_tagging.rs`;
//! the refusals are `reserved_tag_keys.rs`; this suite owns only the class rows,
//! and shares both of their harness (`vector_lifecycle.rs`) and fixture
//! (`resource_tagging::resource_with_tags`).
//!
//! The suite is a TABLE whose rows differ along what the class generalises over:
//! character category, and key and value length at each published bound. Every
//! case drives real HTTP through the production front door
//! (`MultiServiceRouter::dispatch`), so what is asserted is what a client sees.

use crate::resource_tagging::{resource_with_tags, Kind, BUCKET, INDEX};
use crate::vector_lifecycle::{
    expected_bucket_arn, expected_index_arn, pairs, reported_tags, router, tag, tag_object,
};

/// Tag sets that must be accepted and read back unchanged.
///
/// The bounds and the character class come from the model's `TagsMap`
/// (`smithy_query(query="shape", shape="TagsMap")`): its key is `TagKey`
/// (`length 1..128`, pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) and its value is
/// `TagValue` (`length 0..256`, same pattern). The rows walk that class — ASCII
/// letters, non-ASCII letters (`\p{L}`), digits and a separator space (`\p{N}`,
/// `\p{Z}`), every punctuation character the pattern allows, each length bound at
/// its published extreme — and include the keys that draw the line through the
/// namespace rule: ones containing `aws:` without starting with it.
fn accepted_tag_sets() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        (
            "a one-character key and an empty value, the lower length bounds",
            pairs(&[("k", "")]),
        ),
        (
            "non-ASCII letters in the key and the value (the pattern's \\p{L})",
            pairs(&[
                ("mitarbeiter", "gr\u{fc}\u{df}e"),
                ("\u{30c6}\u{30b9}\u{30c8}", "\u{5024}"),
            ]),
        ),
        (
            "digits and a separator space (the pattern's \\p{N} and \\p{Z})",
            pairs(&[("quarter 1", "2026 07")]),
        ),
        (
            "every punctuation character the pattern allows: _ . : / = + - @",
            pairs(&[("a_b.c:d/e=f+g-h@i", "j_k.l:m/n=o+p-q@r")]),
        ),
        (
            "keys that contain the reserved prefix without starting with it",
            pairs(&[("team-aws:cost", "shared"), ("paws:claw", "four")]),
        ),
        (
            "a key at the 128-character ceiling and a value at the 256-character ceiling",
            vec![("k".repeat(128), "v".repeat(256))],
        ),
    ]
}

/// Every accepted tag set round-trips through TagResource and
/// ListTagsForResource, for a vector bucket and for a vector index alike, and
/// tagging one resource leaves the other untagged.
#[tokio::test]
async fn every_accepted_tag_key_and_value_round_trips_for_a_bucket_and_for_an_index() {
    for (case, set) in accepted_tag_sets() {
        for kind in [Kind::Bucket, Kind::Index] {
            // A fresh store per row, holding an untagged bucket AND an untagged
            // index, so the row's set is the whole set the resource carries.
            let router = router();
            resource_with_tags(&router, Kind::Index, &[]).await;
            let bucket_arn = expected_bucket_arn(BUCKET);
            let index_arn = expected_index_arn(BUCKET, INDEX);
            let (target, other) = match kind {
                Kind::Bucket => (&bucket_arn, &index_arn),
                Kind::Index => (&index_arn, &bucket_arn),
            };

            let (status, body) = tag(&router, target, &tag_object(&set)).await;
            assert_eq!(
                status, 200,
                "case {case:?} on {kind:?}: TagResource must succeed, got {status} {body}"
            );

            let reported = reported_tags(&router, case, target).await;
            assert_eq!(
                reported,
                tag_object(&set),
                "case {case:?} on {kind:?}: the tags read back must be the tags applied, \
                 got {reported}"
            );
            let untouched = reported_tags(&router, case, other).await;
            assert_eq!(
                untouched,
                tag_object(&[]),
                "case {case:?} on {kind:?}: tagging one resource must not tag another, \
                 got {untouched}"
            );
        }
    }
}

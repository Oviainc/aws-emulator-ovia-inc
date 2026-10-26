// SPDX-License-Identifier: CC0-1.0

//! Which endpoint may tag a resource, end to end over the wire.
//!
//! The rule under test: the operations that reach a resource by its `resourceArn`
//! (`TagResource`, `UntagResource`) act on the resource that ARN names IN the Region the
//! request was signed for, so an ARN naming a resource of any other Region names nothing
//! at the endpoint the request reached and earns the modelled `ResourceNotFoundException`
//! (HTTP 404) — the same answer the id-based operations give for another Region's
//! resource.
//!
//! What a Region-scoped lookup generalises over is the PAIR of Regions involved and the
//! FORM the ARN takes, so each case below is a row of that table: every ordered pair of
//! the Regions in [`REGIONS`], crossed with the three ways a request can present an ARN
//! whose Region is not the one it was signed for, on both resources this service holds
//! and through both ARN-based operations.

use crate::harness::{
    call_in_region, create_tagged_resource_in, expected_tags, pair, read_resource_in, router,
    tag_object, tags_of, tags_path, untag_resource_in, TAGGABLE_KINDS,
};
use crate::region_isolation::{arn_region, assert_not_found, REGIONS};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// `POST /v1/tags/{resourceArn}` through `region`'s endpoint — the write half of the
/// tagging path, which no other suite sends through a chosen Region.
async fn tag_in(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
    tags: &[(String, String)],
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "POST",
        &tags_path(resource_arn),
        Some(json!({"tags": tag_object(tags)})),
    )
    .await
}

/// `DELETE /v1/tags/{resourceArn}?tagKeys=…` through `region`'s endpoint, for keys given
/// as `&str`.
async fn untag_in(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
    tag_keys: &[&str],
) -> (u16, Value) {
    let keys: Vec<String> = tag_keys.iter().map(|key| (*key).to_string()).collect();
    untag_resource_in(router, region, resource_arn, &keys).await
}

/// A tagging request whose ARN belongs to a Region other than the one it was signed for
/// reaches nothing, whichever side of the pair presents it.
#[tokio::test]
async fn the_tagging_path_reaches_only_the_resources_of_the_signed_for_region() {
    let router = router();

    for owner in REGIONS {
        for kind in TAGGABLE_KINDS {
            let owner_tag = vec![pair("owner", owner)];
            let (id, arn) = create_tagged_resource_in(
                &router,
                owner,
                kind,
                &format!("tagscope{kind}"),
                &owner_tag,
            )
            .await;
            assert_eq!(
                arn_region(&arn),
                owner,
                "case {kind} of {owner:?}: the minted ARN carries the Region of the endpoint \
                 that created the resource, got {arn}"
            );

            for other in REGIONS.iter().filter(|region| **region != owner) {
                // The same id under the OTHER Region's ARN: a value the modelled
                // `resourceArn` pattern admits, naming that Region's resource.
                let rewritten = arn.replacen(arn_region(&arn), other, 1);
                for (form, signed_for, presented) in [
                    (
                        "the owning Region's own ARN, signed for the other Region",
                        *other,
                        arn.clone(),
                    ),
                    (
                        "the other Region's ARN for the same id, signed for that Region",
                        *other,
                        rewritten.clone(),
                    ),
                    (
                        "the other Region's ARN for the same id, signed for the owning Region",
                        owner,
                        rewritten.clone(),
                    ),
                ] {
                    let label = format!("{kind} of {owner}, other Region {other}: {form}");
                    let (status, body) = tag_in(
                        &router,
                        signed_for,
                        &presented,
                        &[pair("written-by", signed_for)],
                    )
                    .await;
                    assert_not_found(&format!("TagResource with {label}"), status, &body);

                    let (status, body) = untag_in(&router, signed_for, &presented, &["owner"]).await;
                    assert_not_found(&format!("UntagResource with {label}"), status, &body);
                }
            }

            // None of those refusals touched the record: its own Region still reports
            // exactly the tag set the resource was created with, so neither the foreign
            // write landed nor the foreign delete removed anything.
            let (status, stored) = read_resource_in(&router, owner, kind, &id).await;
            assert_eq!(
                (status, tags_of(&stored)),
                (200, expected_tags(&owner_tag)),
                "case {kind} of {owner:?}: no other Region's tag write or delete reached it, \
                 got {status} with {stored}"
            );

            // ...and the refusals are about the Region, not about the path: the owning
            // endpoint with the ARN it minted still tags the resource,
            let added = pair("added", "by-its-own-region");
            let (status, body) = tag_in(&router, owner, &arn, std::slice::from_ref(&added)).await;
            assert_eq!(
                status, 204,
                "case {kind} of {owner:?}: its own Region tags it through its own ARN, got \
                 {status} with {body}"
            );
            let (_, stored) = read_resource_in(&router, owner, kind, &id).await;
            assert_eq!(
                tags_of(&stored),
                expected_tags(&[owner_tag[0].clone(), added.clone()]),
                "case {kind} of {owner:?}: its own Region's write landed beside the tag it was \
                 created with, got {stored}"
            );

            // and still deletes a tag from it.
            let (status, body) = untag_in(&router, owner, &arn, &[added.0.as_str()]).await;
            assert_eq!(
                status, 204,
                "case {kind} of {owner:?}: its own Region untags it through its own ARN, got \
                 {status} with {body}"
            );
            let (_, stored) = read_resource_in(&router, owner, kind, &id).await;
            assert_eq!(
                tags_of(&stored),
                expected_tags(&owner_tag),
                "case {kind} of {owner:?}: the delete took the key its own Region named and \
                 left the rest, got {stored}"
            );
        }
    }
}

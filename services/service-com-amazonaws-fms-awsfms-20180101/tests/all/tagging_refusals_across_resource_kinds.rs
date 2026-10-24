// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a Firewall Manager tagging operation does with an
//! ARN whose resource is NOT a policy — on the refusal paths and across the
//! resource's lifetime.
//!
//! The rule asserted here, in one sentence: a tagging operation names its
//! resource by a generic `ResourceArn`, so it resolves that ARN against every
//! kind of taggable resource the service currently persists, and its outcome —
//! the merge, each modelled refusal, and not-found — depends only on whether the
//! ARN names a stored resource and on the `TagList` sent, never on which KIND of
//! resource it is.
//!
//! `TagResourceRequest$ResourceArn` and `ListTagsForResourceRequest$ResourceArn`
//! are the model's generic `com.amazonaws.fms#ResourceArn`, and `PutPolicy`,
//! `PutResourceSet`, `PutAppsList` and `PutProtocolsList` each model a `TagList`
//! input, so each of those four kinds is a resource a tagging operation can be
//! handed.
//!
//! The sibling [`crate::tagging_across_resource_kinds`] suite asserts the SUCCESS
//! path of that rule over (kind x tag set); this suite asserts the rest of it:
//!
//! * every modelled refusal (`InvalidInputException` for a `TagList` repeating a
//!   key, the `TagList` length bound) answers the SAME way whatever kind the ARN
//!   names, and leaves the resource's tags untouched;
//! * an ARN the emulator minted for a resource that has since been DELETED names
//!   nothing, so both tagging operations answer the modelled
//!   `ResourceNotFoundException` and no tag state survives the resource.
//!
//! The resource of each kind is created through its own wire API (an applications
//! list through the store, because `PutAppsList` is still a 501 stub tracked by
//! its own issue) by the shared `create_resource` helper, and deleted through its
//! own `Delete*` operation.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code};
use crate::tagging_across_resource_kinds::{create_resource, Kind, KINDS};
use crate::tagging_and_managing_admins::{
    list_tags, owned, tag_list, tag_list_of_size, MAX_TAGS_PER_RESOURCE,
};

/// The resource id the ARN ends in — a `PolicyId`, a `ListId` or a resource
/// set's `Base62Id`, which is what that kind's own `Delete*` operation names.
/// A string with no `/` carries no resource id at all, which is a broken test
/// fixture rather than a service behaviour.
fn resource_id(arn: &str) -> &str {
    arn.rsplit_once('/')
        .unwrap_or_else(|| panic!("the ARN {arn:?} carries no resource id"))
        .1
}

/// Delete the resource `arn` names through the `Delete*` operation of its own
/// kind, and assert the delete itself answered 200.
async fn delete_resource(router: &MultiServiceRouter, kind: Kind, arn: &str) {
    let id = resource_id(arn);
    let (operation, body) = kind.delete_request(id);
    let (status, answered) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {kind:?}: expected {operation}({id:?}) to answer 200, got {status} with {answered:?}"
    );
}

/// Call `TagResource` and return `(status, error code or "")`.
async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: &Value) -> (u16, String) {
    let (status, body) = call(
        router,
        "TagResource",
        &json!({ "ResourceArn": arn, "TagList": tags }),
    )
    .await;
    let code = if status == 200 {
        String::new()
    } else {
        error_code(&body)
    };
    (status, code)
}

/// One refused tag write: the `TagList` that cannot be stored, and the modelled
/// codes the refusal may carry.
struct Refusal {
    name: &'static str,
    tags: Value,
    /// The one modelled code this refusal carries, with its HTTP status.
    expected: (u16, &'static str),
}

/// The refused-write class, built fresh per row because the over-long list is
/// generated.
fn refusals() -> Vec<Refusal> {
    // One key cannot carry two values, so such a request has no single outcome.
    let duplicate_key = tag_list(&[("k", "1"), ("k", "2")]);
    let duplicate_among_others = tag_list(&[("a", "1"), ("b", "2"), ("a", "3")]);
    // One tag past the modelled `TagList` maximum length.
    let over_bound = tag_list_of_size(MAX_TAGS_PER_RESOURCE + 1);
    vec![
        Refusal {
            name: "the same key twice",
            tags: duplicate_key,
            expected: (400, "InvalidInputException"),
        },
        Refusal {
            name: "a repeated key among other keys",
            tags: duplicate_among_others,
            expected: (400, "InvalidInputException"),
        },
        Refusal {
            // A `TagList` LONGER than the modelled maximum violates the member's
            // own length constraint, which is refused before the handler runs,
            // so this path is the boundary validator's `ValidationException` —
            // not the `LimitExceededException` of a merge that would cross the
            // bound (asserted by the bound test below).
            name: "a TagList one longer than the modelled bound",
            tags: over_bound,
            expected: (400, "ValidationException"),
        },
    ]
}

/// Every modelled refusal of a tag write answers identically whatever KIND of
/// resource the ARN names, and leaves the resource holding exactly what it held
/// before.
#[tokio::test]
async fn tag_write_refusals_do_not_depend_on_the_resource_kind() {
    let held = [("stage", "test")];

    for refusal in refusals() {
        for kind in KINDS {
            let case = format!("{kind:?} / {}", refusal.name);
            let (router, engine) = emulator_with_engine();
            associate_admin(&router).await;
            let arn = create_resource(
                &router,
                &engine,
                kind,
                "refused.resource",
                Some(&tag_list(&held)),
                &case,
            )
            .await;

            let (refused_status, refused_code) = tag_resource(&router, &arn, &refusal.tags).await;
            // The SAME expected pair for every kind, so this one assertion is
            // what pins "the refusal does not depend on the resource kind".
            assert_eq!(
                (refused_status, refused_code.as_str()),
                refusal.expected,
                "case {case:?}: expected {:?}, got {refused_status} / {refused_code:?}",
                refusal.expected
            );

            // A refused write stores nothing: the resource still holds the tags
            // it was created with, on the wire and in the store's own records.
            let (status, read_code, tags) = list_tags(&router, &arn).await;
            assert_eq!(
                (status, read_code.as_str()),
                (200, ""),
                "case {case:?}: expected ListTagsForResource to answer 200 after the refusal, \
                 got {status} / {read_code:?}"
            );
            assert_eq!(
                tags,
                owned(&held),
                "case {case:?}: a refused TagResource must leave the tag set alone, got {tags:?}"
            );
            let stored = with_store(&engine, |store| {
                store.tags_for_arn(&arn).map(|held| {
                    let mut pairs: Vec<(String, String)> = held
                        .iter()
                        .map(|tag| (tag.key.clone(), tag.value.clone()))
                        .collect();
                    pairs.sort();
                    pairs
                })
            });
            assert_eq!(
                stored.as_deref(),
                Some(owned(&held).as_slice()),
                "case {case:?}: the resource's own record must be unchanged by the refusal, got \
                 {stored:?}"
            );
        }
    }
}

/// The tag set a resource of ANY taggable kind can hold is bounded by the
/// modelled `TagList` length: the bound itself is accepted and read back, and one
/// tag past it is refused rather than stored.
#[tokio::test]
async fn the_tag_list_bound_holds_for_every_resource_kind() {
    for kind in KINDS {
        let case = format!("{kind:?}");
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        // Created with no tags, so the write below lands exactly at the bound.
        let arn = create_resource(&router, &engine, kind, "bounded.resource", None, &case).await;

        let at_bound = tag_list_of_size(MAX_TAGS_PER_RESOURCE);
        let (status, code) = tag_resource(&router, &arn, &at_bound).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: expected a {MAX_TAGS_PER_RESOURCE}-tag write to answer 200, got \
             {status} / {code:?}"
        );
        let (_, _, tags) = list_tags(&router, &arn).await;
        assert_eq!(
            tags.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: expected the resource to hold {MAX_TAGS_PER_RESOURCE} tags, got {}",
            tags.len()
        );

        // A `TagList` the member's own length constraint admits, whose MERGE
        // with the tags already held would cross the bound: the modelled
        // `LimitExceededException`, the error documented as "the operation
        // exceeds a resource limit".
        let (status, code) =
            tag_resource(&router, &arn, &tag_list(&[("key-past-the-bound", "v")])).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "LimitExceededException"),
            "case {case:?}: expected a write past the {MAX_TAGS_PER_RESOURCE}-tag bound to be \
             refused with 400 LimitExceededException, got {status} / {code:?}"
        );
        let (_, _, tags) = list_tags(&router, &arn).await;
        assert_eq!(
            tags.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: a refused write must not grow the tag set, got {}",
            tags.len()
        );
    }
}

/// A resource of any taggable kind is taggable while it is stored and names
/// nothing once it is deleted: the same ARN that answered 200 answers the
/// modelled `ResourceNotFoundException` after the resource's own `Delete*`, and
/// no tag state outlives the resource.
#[tokio::test]
async fn an_arn_is_taggable_while_its_resource_is_stored_and_not_found_after_it_is_deleted() {
    let created = [("stage", "test")];
    let added = [("owner", "ops")];

    for kind in KINDS {
        let case = format!("{kind:?}");
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        let arn = create_resource(
            &router,
            &engine,
            kind,
            "deleted.resource",
            Some(&tag_list(&created)),
            &case,
        )
        .await;

        // While the resource is stored, the write lands and the read reports it.
        let (status, code) = tag_resource(&router, &arn, &tag_list(&added)).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: expected TagResource({arn:?}) to answer 200 while the resource is \
             stored, got {status} / {code:?}"
        );
        let (status, read_code, tags) = list_tags(&router, &arn).await;
        let expected = owned(&[created[0], added[0]]);
        assert_eq!(
            (status, read_code.as_str()),
            (200, ""),
            "case {case:?}: expected ListTagsForResource({arn:?}) to answer 200, got {status} / \
             {read_code:?}"
        );
        assert_eq!(
            tags, expected,
            "case {case:?}: expected the merged tag set {expected:?}, got {tags:?}"
        );

        delete_resource(&router, kind, &arn).await;

        // The ARN now names nothing, so both tagging operations are not-found.
        let (status, code) = tag_resource(&router, &arn, &tag_list(&[("after", "delete")])).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected TagResource({arn:?}) to answer 400 \
             ResourceNotFoundException after the delete, got {status} / {code:?}"
        );
        let (status, code, _) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected ListTagsForResource({arn:?}) to answer 400 \
             ResourceNotFoundException after the delete, got {status} / {code:?}"
        );

        // No tag state outlived the resource, in the records or in the ARN-keyed
        // index, so a later resource reusing the ARN could not inherit tags.
        let (recorded, indexed) = with_store(&engine, |store| {
            (
                store.tags_for_arn(&arn),
                store.tags_by_arn.get(&arn).cloned(),
            )
        });
        assert!(
            recorded.is_none(),
            "case {case:?}: no record must hold tags for a deleted resource, got {recorded:?}"
        );
        assert!(
            indexed.is_none(),
            "case {case:?}: the ARN-keyed tag index must not outlive the resource, got \
             {indexed:?}"
        );
    }
}

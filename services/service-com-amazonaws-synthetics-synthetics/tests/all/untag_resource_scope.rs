// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of what an `UntagResource` call is SCOPED to.
//!
//! One rule, asserted over a table of key sets rather than over one recorded request:
//! a tag removal drops exactly the keys the request names from exactly the resource its
//! `ResourceArn` names — every other key on that resource, and every key on every other
//! resource, survives — and because the call states only that those keys are absent
//! afterwards, repeating it answers the same success and leaves the same map.
//! ("Removes one or more tags from the specified resource",
//! `smithy_query(query="doc",shape="UntagResource")`.)
//!
//! The rows differ along what the rule generalises over: how many keys one call names
//! (one, several, and the modelled maximum of `TagKeyList`, `length 1..50`), whether a
//! named key is present at all, whether it is named twice, and in what order the keys
//! arrive — none of which the rule distinguishes. Every row runs for both resource kinds
//! `ResourceArn` admits, canary and group.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the emulator
//! boots, reusing the tag harness the tag-write suite defines.

use crate::canary_lifecycle::Synthetics;
use crate::tag_write_path::{
    expect_ok, full_tag_set, list_tags, pairs, seed, tag_map, tags_on_the_resource, untag, Kind,
};

/// Every key of a tag set, in the order the set lists them.
fn keys_of(tags: &[(String, String)]) -> Vec<String> {
    tags.iter().map(|(key, _)| key.clone()).collect()
}

/// The tag set `tags` minus every key in `removed`, which is what the resource must carry
/// after the removal — computed from the row's own inputs, never from the response.
fn without(tags: &[(String, String)], removed: &[String]) -> Vec<(String, String)> {
    tags.iter()
        .filter(|(key, _)| !removed.iter().any(|dropped| dropped == key))
        .cloned()
        .collect()
}

/// One removal: the tags the resource starts with and the `TagKeys` the call names.
#[derive(Debug)]
struct Row {
    case: &'static str,
    seeded: Vec<(String, String)>,
    remove: Vec<String>,
}

/// The key-set classes the rule generalises over.
fn rows() -> Vec<Row> {
    let three = pairs(&[("env", "prod"), ("team", "core"), ("tier:level", "gold")]);
    let fifty = full_tag_set();
    vec![
        Row {
            case: "one-key-of-three",
            seeded: three.clone(),
            remove: vec!["team".to_string()],
        },
        Row {
            case: "two-keys-in-one-call",
            seeded: three.clone(),
            remove: vec!["env".to_string(), "tier:level".to_string()],
        },
        Row {
            // The list is a set of keys, so the order they arrive in cannot matter.
            case: "keys-in-reverse-order",
            seeded: three.clone(),
            remove: vec!["tier:level".to_string(), "env".to_string()],
        },
        Row {
            // One key named twice removes it once and is not a failure.
            case: "the-same-key-named-twice",
            seeded: three.clone(),
            remove: vec!["env".to_string(), "env".to_string()],
        },
        Row {
            // A key the resource never carried alongside one it does.
            case: "a-present-key-and-an-absent-one",
            seeded: three.clone(),
            remove: vec!["never-set".to_string(), "team".to_string()],
        },
        Row {
            // Only keys the resource never carried: nothing changes.
            case: "only-keys-the-resource-never-carried",
            seeded: three.clone(),
            remove: vec!["never-set".to_string(), "also/absent".to_string()],
        },
        Row {
            // `TagKeyList` is `length 1..50`, so fifty keys in one call is the maximum a
            // request may name — and here it empties a resource at the tag maximum.
            case: "the-modelled-maximum-number-of-keys",
            seeded: fifty.clone(),
            remove: keys_of(&fifty),
        },
        Row {
            // The other end of the same resource: one key out of fifty, forty-nine left.
            case: "one-key-out-of-the-maximum-tag-set",
            seeded: fifty,
            remove: vec!["key7".to_string()],
        },
    ]
}

/// The rule, over both resource kinds: a removal is scoped to the keys it names and the
/// resource it addresses, and repeating it is a no-op.
#[tokio::test]
async fn untag_resource_removes_only_the_keys_named_on_only_the_resource_named() {
    let region = "eu-west-1";
    let aws = Synthetics::new(region);
    let account = emulator_server::emulator_account_id();

    for (index, row) in rows().into_iter().enumerate() {
        for kind in [Kind::Canary, Kind::Group] {
            let case = format!("{}/{}", kind.case(), row.case);
            // The target and a bystander of the OTHER kind, carrying the very same keys:
            // the removal addresses one ARN, so the bystander must keep all of them.
            let bystander_kind = match kind {
                Kind::Canary => Kind::Group,
                Kind::Group => Kind::Canary,
            };
            let target_name = format!("untag-{}-{index}", kind.case());
            let bystander_name = format!("keep-{}-{index}", bystander_kind.case());
            let (_, target_arn) = seed(
                &aws,
                kind,
                &target_name,
                Some(row.seeded.clone()),
                region,
                account,
            )
            .await;
            let (_, bystander_arn) = seed(
                &aws,
                bystander_kind,
                &bystander_name,
                Some(row.seeded.clone()),
                region,
                account,
            )
            .await;

            let expected = tag_map(without(&row.seeded, &row.remove));

            // The same call twice: the second one asserts the removal is idempotent.
            for attempt in ["first", "repeated"] {
                let answer = untag(&aws, &target_arn, &row.remove).await;
                expect_ok(&format!("{case}/{attempt}"), &answer);

                let listed = list_tags(&aws, &target_arn).await;
                expect_ok(&format!("{case}/{attempt}"), &listed);
                assert_eq!(
                    listed.at("Tags"),
                    &expected,
                    "case {case}/{attempt}: expected only the named keys removed, got {:?}",
                    listed.at("Tags")
                );

                let on_resource = tags_on_the_resource(&aws, kind, &target_name, &target_arn).await;
                assert_eq!(
                    on_resource, expected,
                    "case {case}/{attempt}: expected the resource's own tag member to \
                     report the tags the removal left, got {on_resource:?}"
                );
            }

            let bystander = list_tags(&aws, &bystander_arn).await;
            expect_ok(&format!("{case}/bystander"), &bystander);
            assert_eq!(
                bystander.at("Tags"),
                &tag_map(row.seeded.clone()),
                "case {case}/bystander: expected another resource's tags to survive the \
                 removal, got {:?}",
                bystander.at("Tags")
            );
        }
    }
}

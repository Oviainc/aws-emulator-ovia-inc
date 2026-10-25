// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` on the tags a resource's OWN CREATE persisted, driven end to end over
//! the service's real `restJson1` wire through the harness in `tests/all/tagging_surface.rs`.
//!
//! RULE under test: `UntagResource` deletes exactly the keys its `TagKeys` names from the
//! stored record of whichever taggable kind its `ResourceArn` resolves to, whatever call
//! persisted those keys — the resource's own `Create*` included — leaves every key the
//! request does not name in place, and answers the modelled success either way ("Deletes
//! specified tags from a resource", `smithy_query(query="operation",
//! operation="UntagResource")`, `http.code` 204). Only an ARN naming no stored resource is
//! a not-found.
//!
//! `tests/all/tagging_surface.rs` removes a key a preceding `TagResource` had merged in;
//! this suite removes the pairs that arrived with the resource, which is the half a
//! resolution gap in one kind's record lookup hides: a create may persist `Tags` that no
//! later call can ever take off.
//!
//! The rows differ along what the rule generalises over: the resource KIND (all four the
//! modelled tagging `Arn` enumerates — `pricingplan`, `billinggroup`, `pricingrule` and
//! `customlineitem`), the ARN SPELLING of one resource (the full ARN, the optional
//! `billinggroup` separator, and the bare custom-line-item id), the charge shape of a
//! custom line item (flat and percentage), how many create-time pairs the record carries,
//! how many of them one request removes (one, several, all), whether a named key was ever
//! set, and a key at the modelled `TagKey` maximum.

use crate::tagging_surface::{bare_id, expected, filler, tag_map, Emulator, TAGGING_STATUS};
use serde_json::json;

/// One row: the ARN the removal addresses, the spelling the tags are read back through
/// (the same resource), what the record holds before the removal, the keys the request
/// names, and what the record holds afterwards.
struct UntagCase<'a> {
    label: &'a str,
    address: &'a str,
    read_through: &'a str,
    before: Vec<(&'a str, &'a str)>,
    remove: Vec<&'a str>,
    after: Vec<(&'a str, &'a str)>,
}

/// RULE: the keys a create persisted are ordinary tags of the stored record, so
/// `UntagResource` removes exactly the ones it names from any taggable kind, through any
/// ARN spelling that kind admits, and leaves the rest.
#[tokio::test]
async fn untag_resource_removes_the_tags_a_create_persisted() {
    let em = Emulator::new();

    // A `TagKey` at its modelled maximum (`smithy_query(query="shape",shape="TagKey")`:
    // len 1..128), persisted by a create and removed like any other key.
    let long_key = filler(128);

    let plan = em
        .create_plan(
            "CreatedTagsPlan",
            Some(tag_map(&[
                ("env", "prod"),
                ("owner", "ops"),
                ("team", "finance"),
            ])),
        )
        .await;
    // Created after the plan so the group's minted id is still all digits: the tagging
    // `Arn` pattern spells a billing group `billinggroup/?[0-9]{12}`.
    let group = em
        .create_group(
            "CreatedTagsGroup",
            &plan,
            Some(tag_map(&[("env", "prod"), ("owner", "ops")])),
        )
        .await;
    // The same group, in the spelling the optional separator admits.
    let group_unseparated = group.replace("billinggroup/", "billinggroup");
    let rule = em
        .create_rule("CreatedTagsRule", Some(tag_map(&[("env", "prod")])))
        .await;
    let flat_item = em
        .create_item(
            "CreatedTagsFlatItem",
            &group,
            json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            Some(tag_map(&[
                ("env", "prod"),
                ("owner", "ops"),
                (long_key.as_str(), "max-length-key"),
            ])),
        )
        .await;
    let percentage_item = em
        .create_item(
            "CreatedTagsPercentageItem",
            &group,
            json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 12.5 } }),
            Some(tag_map(&[("env", "prod"), ("owner", "ops")])),
        )
        .await;
    let percentage_item_bare = bare_id(&percentage_item).to_string();
    // A second custom line item carrying the same create-time pairs, to show a removal is
    // scoped to the record its `ResourceArn` names.
    let sibling_item = em
        .create_item(
            "CreatedTagsSiblingItem",
            &group,
            json!({ "Type": "FEE", "Flat": { "ChargeValue": 1.0 } }),
            Some(tag_map(&[("env", "prod"), ("owner", "ops")])),
        )
        .await;

    let cases: Vec<UntagCase<'_>> = vec![
        UntagCase {
            label: "one of a pricing plan's three create-time keys",
            address: &plan,
            read_through: &plan,
            before: vec![("env", "prod"), ("owner", "ops"), ("team", "finance")],
            remove: vec!["env"],
            after: vec![("owner", "ops"), ("team", "finance")],
        },
        UntagCase {
            label: "the pricing plan's two remaining create-time keys at once",
            address: &plan,
            read_through: &plan,
            before: vec![("owner", "ops"), ("team", "finance")],
            remove: vec!["owner", "team"],
            after: vec![],
        },
        UntagCase {
            label: "a billing group's create-time key, addressed without the separator",
            address: &group_unseparated,
            read_through: &group,
            before: vec![("env", "prod"), ("owner", "ops")],
            remove: vec!["env"],
            after: vec![("owner", "ops")],
        },
        UntagCase {
            label: "a pricing rule's only create-time key",
            address: &rule,
            read_through: &rule,
            before: vec![("env", "prod")],
            remove: vec!["env"],
            after: vec![],
        },
        UntagCase {
            label: "one create-time key of a flat custom line item, by its full ARN",
            address: &flat_item,
            read_through: &flat_item,
            before: vec![
                ("env", "prod"),
                ("owner", "ops"),
                (long_key.as_str(), "max-length-key"),
            ],
            remove: vec!["env"],
            after: vec![("owner", "ops"), (long_key.as_str(), "max-length-key")],
        },
        UntagCase {
            label: "a create-time key and a key never set, on the same item by its bare id",
            address: bare_id(&flat_item),
            read_through: &flat_item,
            before: vec![("owner", "ops"), (long_key.as_str(), "max-length-key")],
            remove: vec!["owner", "never-set"],
            after: vec![(long_key.as_str(), "max-length-key")],
        },
        UntagCase {
            label: "the item's create-time key at the modelled TagKey maximum",
            address: &flat_item,
            read_through: &flat_item,
            before: vec![(long_key.as_str(), "max-length-key")],
            remove: vec![long_key.as_str()],
            after: vec![],
        },
        UntagCase {
            label: "every create-time key of a percentage item, addressed by its bare id",
            address: &percentage_item_bare,
            read_through: &percentage_item,
            before: vec![("env", "prod"), ("owner", "ops")],
            remove: vec!["env", "owner"],
            after: vec![],
        },
    ];

    for case in &cases {
        let label = case.label;

        let before = em.stored_tags(label, case.read_through).await;
        assert_eq!(
            before,
            expected(&case.before),
            "case {label:?}: expected the stored tags {:?} before the removal, got {before:?}",
            expected(&case.before)
        );

        let (status, body) = em.untag(case.address, &case.remove).await;
        assert_eq!(
            status, TAGGING_STATUS,
            "case {label:?}: UntagResource expected {TAGGING_STATUS}, got {status} with {body}"
        );
        let left = em.stored_tags(label, case.read_through).await;
        assert_eq!(
            left,
            expected(&case.after),
            "case {label:?}: expected the tags {:?} to be left, got {left:?}",
            expected(&case.after)
        );

        // The request asks for a state that now holds, so the same removal again is
        // served and changes nothing.
        let (status, body) = em.untag(case.address, &case.remove).await;
        assert_eq!(
            status, TAGGING_STATUS,
            "case {label:?}: the repeated UntagResource expected {TAGGING_STATUS}, got \
             {status} with {body}"
        );
        let again = em.stored_tags(label, case.read_through).await;
        assert_eq!(
            again, left,
            "case {label:?}: expected the repeated removal to change nothing, got {again:?} \
             after {left:?}"
        );
    }

    // Every removal above acted on the record its `ResourceArn` named: a sibling custom
    // line item created with the same pairs still carries both of them.
    let sibling = em
        .stored_tags("a sibling item keeps its own create-time tags", &sibling_item)
        .await;
    assert_eq!(
        sibling,
        expected(&[("env", "prod"), ("owner", "ops")]),
        "expected the sibling item to keep its own create-time tags {:?}, got {sibling:?}",
        expected(&[("env", "prod"), ("owner", "ops")])
    );

    // A removed create-time key is gone for good: a later `TagResource` can put it back,
    // and the record is the one place the value lives.
    let (status, body) = em.tag(&rule, &tag_map(&[("env", "staging")])).await;
    assert_eq!(
        status, TAGGING_STATUS,
        "re-tagging a pricing rule after the removal expected {TAGGING_STATUS}, got {status} \
         with {body}"
    );
    let restored = em
        .stored_tags("a removed key can be set again", &rule)
        .await;
    assert_eq!(
        restored,
        expected(&[("env", "staging")]),
        "expected the re-tagged value {:?}, got {restored:?}",
        expected(&[("env", "staging")])
    );
}

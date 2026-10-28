// SPDX-License-Identifier: CC0-1.0

//! The READ half and the WRITE half of the tagging surface land on the SAME record, for
//! every taggable kind — driven end to end over the service's real `restJson1` wire
//! through the harness in `tests/all/tagging_surface.rs`.
//!
//! RULE under test: a tagging `ResourceArn` names ONE stored resource, so the operation
//! that reports a resource's tags (`ListTagsForResource`) and the operations that write
//! them (`TagResource`, `UntagResource`) reach that one record — whichever spelling of
//! the ARN each call used — and touch no other resource's tags.
//!
//! The rows differ along the resource KIND (all four the modelled tagging `Arn`
//! enumerates: `pricingplan`, `billinggroup`, `pricingrule` and `customlineitem`) and
//! along the pair of ARN SPELLINGS the two halves address one resource by (the full ARN
//! on both halves; the `billinggroup` separator the modelled pattern makes optional on
//! the write half only; the bare custom-line-item id on the write half only), so each
//! row writes through one spelling and reads back through another.
//!
//! `tests/all/tagging_surface.rs` asserts what each tagging operation ANSWERS; this suite
//! asserts the two halves agree on which record they act on, for every kind and in both
//! directions (a write the read sees, and a removal the read sees), plus that a write to
//! one kind leaves every other kind's tags where they were.

use crate::tagging_surface::{bare_id, expected, tag_map, Emulator, TAGGING_STATUS};
use serde_json::json;

/// One row: a taggable resource, the pairs its create persisted, and the ARN spelling
/// each half of the surface addresses it by.
struct KindCase {
    label: &'static str,
    /// The spelling `TagResource` and `UntagResource` address the resource by.
    write_through: String,
    /// The spelling `ListTagsForResource` reads it back through.
    read_through: String,
    /// The pairs this resource's own create persisted.
    created: Vec<(&'static str, &'static str)>,
    /// The pair the write half merges in and then removes again.
    added: (&'static str, &'static str),
}

/// RULE: both halves of the tagging surface resolve one record per resource, for every
/// taggable kind and through every ARN spelling that kind admits — a merge is visible to
/// the read, a removal is visible to the read, and neither disturbs another kind.
#[tokio::test]
async fn both_halves_of_the_tagging_surface_resolve_one_record_per_kind() {
    let em = Emulator::new();
    // The billing group is created early so its minted id is still all digits: the
    // tagging `Arn` pattern spells a billing group `billinggroup/?[0-9]{12}`.
    let plan = em
        .create_plan("DispatchPlan", Some(tag_map(&[("kind", "pricingplan")])))
        .await;
    let group = em
        .create_group(
            "DispatchGroup",
            &plan,
            Some(tag_map(&[("kind", "billinggroup")])),
        )
        .await;
    let rule = em
        .create_rule("DispatchRule", Some(tag_map(&[("kind", "pricingrule")])))
        .await;
    let item = em
        .create_item(
            "DispatchItem",
            &group,
            json!({ "Type": "FEE", "Flat": { "ChargeValue": 3.5 } }),
            Some(tag_map(&[("kind", "customlineitem")])),
        )
        .await;

    let cases: Vec<KindCase> = vec![
        KindCase {
            label: "a pricing plan, both halves addressing its full ARN",
            write_through: plan.clone(),
            read_through: plan.clone(),
            created: vec![("kind", "pricingplan")],
            added: ("added", "plan"),
        },
        KindCase {
            label: "a billing group, written without the optional separator and read with it",
            write_through: group.replace("billinggroup/", "billinggroup"),
            read_through: group.clone(),
            created: vec![("kind", "billinggroup")],
            added: ("added", "group"),
        },
        KindCase {
            label: "a pricing rule, both halves addressing its full ARN",
            write_through: rule.clone(),
            read_through: rule.clone(),
            created: vec![("kind", "pricingrule")],
            added: ("added", "rule"),
        },
        KindCase {
            label: "a custom line item, written by its bare id and read by its full ARN",
            write_through: bare_id(&item).to_string(),
            read_through: item.clone(),
            created: vec![("kind", "customlineitem")],
            added: ("added", "item"),
        },
    ];

    // What each row's resource is expected to hold right now.
    let mut held: Vec<Vec<(&'static str, &'static str)>> =
        cases.iter().map(|case| case.created.clone()).collect();

    // Every row starts at what its own create persisted, as the read half reports it
    // through that row's read spelling.
    for (index, case) in cases.iter().enumerate() {
        let reported = em.stored_tags(case.label, &case.read_through).await;
        assert_eq!(
            reported,
            expected(&held[index]),
            "case {:?}: expected the create's own tags {:?}, got {reported:?}",
            case.label,
            expected(&held[index])
        );
    }

    for (index, case) in cases.iter().enumerate() {
        let label = case.label;

        // The WRITE half merges one pair, through this row's write spelling.
        let (status, body) = em.tag(&case.write_through, &tag_map(&[case.added])).await;
        assert_eq!(
            status, TAGGING_STATUS,
            "case {label:?}: TagResource through {:?} expected {TAGGING_STATUS}, got {status} \
             with {body}",
            case.write_through
        );
        held[index].push(case.added);

        // …and the READ half, through the OTHER spelling, reports it: one record.
        let merged = em.stored_tags(label, &case.read_through).await;
        assert_eq!(
            merged,
            expected(&held[index]),
            "case {label:?}: expected the merged tags {:?} read back through {:?}, got \
             {merged:?}",
            expected(&held[index]),
            case.read_through
        );
        let through_write_spelling = em.stored_tags(label, &case.write_through).await;
        assert_eq!(
            through_write_spelling, merged,
            "case {label:?}: expected both ARN spellings to report one record's tags, got \
             {through_write_spelling:?} through {:?} and {merged:?} through {:?}",
            case.write_through, case.read_through
        );
        assert_untouched(&em, &cases, &held, index, "the merge").await;

        // The WRITE half removes the same pair again, and the READ half sees that too.
        let (status, body) = em.untag(&case.write_through, &[case.added.0]).await;
        assert_eq!(
            status, TAGGING_STATUS,
            "case {label:?}: UntagResource through {:?} expected {TAGGING_STATUS}, got \
             {status} with {body}",
            case.write_through
        );
        held[index].pop();
        let left = em.stored_tags(label, &case.read_through).await;
        assert_eq!(
            left,
            expected(&held[index]),
            "case {label:?}: expected the removal to leave {:?}, got {left:?}",
            expected(&held[index])
        );
        assert_untouched(&em, &cases, &held, index, "the removal").await;
    }
}

/// Assert that every row OTHER than `acting` still holds exactly what it held: a write
/// addressed at one resource reaches that resource's record and no other kind's.
async fn assert_untouched(
    em: &Emulator,
    cases: &[KindCase],
    held: &[Vec<(&'static str, &'static str)>],
    acting: usize,
    what: &str,
) {
    for (index, other) in cases.iter().enumerate() {
        if index == acting {
            continue;
        }
        let reported = em.stored_tags(other.label, &other.read_through).await;
        assert_eq!(
            reported,
            expected(&held[index]),
            "case {:?}: {what} on {:?} must leave it holding {:?}, got {reported:?}",
            other.label,
            cases[acting].label,
            expected(&held[index])
        );
    }
}

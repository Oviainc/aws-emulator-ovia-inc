// SPDX-License-Identifier: CC0-1.0

//! A taggable resource's tags over that resource's LIFECYCLE, driven end to end over the
//! service's real `restJson1` wire through the harness in `tests/all/tagging_surface.rs`.
//!
//! RULE under test: a tagging operation addresses the resource its `ResourceArn` names
//! through that resource's OWN stored record, so a resource's tags exist exactly as long
//! as the resource does — every mutation of the resource that is not a tagging call
//! leaves its tags alone, the record stays writable through every ARN spelling the
//! modelled tagging `Arn` admits for its kind, and once the record is gone every tagging
//! operation answers the surface's not-found instead of reporting or storing anything.
//!
//! `tests/all/tagging_surface.rs` asserts the same rule ACROSS kinds at one point in a
//! resource's life; this suite walks the life of the one taggable kind that has further
//! lifecycle operations of its own — a custom line item, which `UpdateCustomLineItem`
//! re-shapes and `DeleteCustomLineItem` either narrows or removes.
//!
//! The rows differ along: the charge shape of the item (flat `FEE` and percentage
//! `CREDIT`), whether its create persisted tags, how many pairs it persisted, the ARN
//! spelling the tagging calls address it by (the full ARN and the bare 10-character id
//! the pattern admits), which member an intervening update changes (the description, the
//! name, the charge value, the billing-period span), and — for the removal half — whether
//! the delete narrows the item's billing-period span or removes the record outright.

use crate::tagging_surface::{
    assert_tagging_not_found, bare_id, expected, tag_map, Emulator, TAGGING_STATUS,
};
use serde_json::{json, Value};

/// `POST /update-custom-line-item` with whatever members `request` carries beside `Arn`.
async fn update(em: &Emulator, case: &str, arn: &str, request: &Value) {
    let mut body = json!({ "Arn": arn });
    for (member, value) in request
        .as_object()
        .expect("the update request is a JSON object")
    {
        body[member.as_str()] = value.clone();
    }
    let (status, answered) = em
        .call("POST", "/update-custom-line-item", Some(&body))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateCustomLineItem expected 200, got {status} with {answered}"
    );
}

/// `POST /delete-custom-line-item`, with the `BillingPeriodRange` the request names
/// (none when `range` is null).
async fn delete(em: &Emulator, case: &str, arn: &str, range: &Value) {
    let mut body = json!({ "Arn": arn });
    if !range.is_null() {
        body["BillingPeriodRange"] = range.clone();
    }
    let (status, answered) = em
        .call("POST", "/delete-custom-line-item", Some(&body))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteCustomLineItem expected 200, got {status} with {answered}"
    );
}

/// One row: a custom line item, the pairs its create persisted, the ARN spelling the
/// tagging calls address it by, and the update that intervenes between the tag and the
/// read-back.
struct LifecycleCase<'a> {
    label: &'a str,
    name: &'a str,
    charge_details: Value,
    created: Vec<(&'a str, &'a str)>,
    /// Whether the tagging calls address the item by its bare id rather than its ARN.
    by_bare_id: bool,
    /// The `UpdateCustomLineItem` members applied after the merge.
    update: Value,
}

/// RULE: a mutation of a taggable resource that is not a tagging call leaves the
/// resource's tags exactly as they were, and the resource stays writable through every
/// ARN spelling its kind admits.
///
/// The rows differ along the item's charge shape, the pairs its create persisted, the ARN
/// spelling the tagging calls use, and which member the intervening update changes.
#[tokio::test]
async fn updating_a_custom_line_item_leaves_its_tags_exactly_as_they_were() {
    let em = Emulator::new();
    let plan = em.create_plan("LifecyclePlan", None).await;
    let group = em.create_group("LifecycleGroup", &plan, None).await;

    let cases: Vec<LifecycleCase<'_>> = vec![
        LifecycleCase {
            label: "a flat item created with two pairs, its description updated",
            name: "FlatTwoPairs",
            charge_details: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            created: vec![("kind", "customlineitem"), ("env", "prod")],
            by_bare_id: false,
            update: json!({ "Description": "a new description" }),
        },
        LifecycleCase {
            label: "a flat item addressed by its bare id, its charge value updated",
            name: "FlatBareId",
            charge_details: json!({ "Type": "FEE", "Flat": { "ChargeValue": 1.0 } }),
            created: vec![("only", "one")],
            by_bare_id: true,
            update: json!({ "ChargeDetails": { "Flat": { "ChargeValue": 42.5 } } }),
        },
        LifecycleCase {
            label: "a percentage item created with no tags, its name updated",
            name: "PercentageUntagged",
            charge_details: json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 12.5 } }),
            created: vec![],
            by_bare_id: false,
            update: json!({ "Name": "PercentageRenamed" }),
        },
        LifecycleCase {
            label: "a percentage item by bare id, its billing-period span narrowed",
            name: "PercentageRanged",
            charge_details: json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 1.0 } }),
            created: vec![("a", "1"), ("b", "2"), ("c", "3")],
            by_bare_id: true,
            update: json!({ "BillingPeriodRange": { "InclusiveStartBillingPeriod": "2026-1" } }),
        },
    ];

    for case in &cases {
        let label = case.label;
        let tags = if case.created.is_empty() {
            None
        } else {
            Some(tag_map(&case.created))
        };
        let arn = em
            .create_item(case.name, &group, case.charge_details.clone(), tags)
            .await;
        let address = if case.by_bare_id {
            bare_id(&arn).to_string()
        } else {
            arn.clone()
        };

        // One merge, through the spelling this row addresses the item by.
        let (status, body) = em.tag(&address, &tag_map(&[("added", "yes")])).await;
        assert_eq!(
            status, TAGGING_STATUS,
            "case {label:?}: TagResource expected {TAGGING_STATUS}, got {status} with {body}"
        );
        let mut after = case.created.clone();
        after.push(("added", "yes"));
        let merged = em.stored_tags(label, &arn).await;
        assert_eq!(
            merged,
            expected(&after),
            "case {label:?}: expected the merged tags {:?}, got {merged:?}",
            expected(&after)
        );

        // The update changes the resource, not its tags.
        update(&em, label, &arn, &case.update).await;
        let survived = em.stored_tags(label, &arn).await;
        assert_eq!(
            survived,
            expected(&after),
            "case {label:?}: expected the tags {:?} to survive the update, got {survived:?}",
            expected(&after)
        );

        // …and the updated record is still writable and still removable through the same
        // spelling.
        let (status, body) = em.tag(&address, &tag_map(&[("after-update", "yes")])).await;
        assert_eq!(
            status, TAGGING_STATUS,
            "case {label:?}: TagResource after the update expected {TAGGING_STATUS}, got \
             {status} with {body}"
        );
        let (status, body) = em.untag(&address, &["added", "after-update"]).await;
        assert_eq!(
            status, TAGGING_STATUS,
            "case {label:?}: UntagResource after the update expected {TAGGING_STATUS}, got \
             {status} with {body}"
        );
        let restored = em.stored_tags(label, &arn).await;
        assert_eq!(
            restored,
            expected(&case.created),
            "case {label:?}: expected the create's own tags {:?} to be left, got {restored:?}",
            expected(&case.created)
        );
    }
}

/// RULE, the end of the life: a resource's tags live on the resource's own record, so a
/// delete that REMOVES the record takes the tags with it (every tagging operation then
/// answers the surface's not-found), while a delete that only NARROWS the record's
/// billing-period span keeps both the record and its tags.
///
/// The rows differ along the charge shape, whether the create persisted tags, the ARN
/// spelling addressed, and whether the delete narrows the span or removes the record.
#[tokio::test]
async fn a_deleted_custom_line_item_takes_its_tags_with_it() {
    let em = Emulator::new();
    let plan = em.create_plan("DeletePlan", None).await;
    let group = em.create_group("DeleteGroup", &plan, None).await;

    // A delete whose `BillingPeriodRange` names only periods inside the item's span
    // leaves a narrower item behind; one that names no range at all removes the record.
    // (`DeleteCustomLineItem`: "Deletes the custom line item identified by the given
    // ARN".)
    //
    // That delete may only act "in the current, or previous billing period", so the
    // span it names starts at the current one, which the service's own clock decides.
    let current_billing_period =
        service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period();
    struct DeleteCase<'a> {
        label: &'a str,
        name: &'a str,
        charge_details: Value,
        created: Vec<(&'a str, &'a str)>,
        by_bare_id: bool,
        /// `Value::Null` for a delete that names no billing-period range.
        range: Value,
        /// Whether the record — and so its tags — survives the delete.
        survives: bool,
    }

    let cases: Vec<DeleteCase<'_>> = vec![
        DeleteCase {
            label: "a tagged flat item, deleted outright by its full ARN",
            name: "FlatDeleted",
            charge_details: json!({ "Type": "FEE", "Flat": { "ChargeValue": 5.0 } }),
            created: vec![("kind", "customlineitem"), ("env", "prod")],
            by_bare_id: false,
            range: Value::Null,
            survives: false,
        },
        DeleteCase {
            label: "an untagged percentage item, deleted outright by its bare id",
            name: "PercentageDeleted",
            charge_details: json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 2.5 } }),
            created: vec![],
            by_bare_id: true,
            range: Value::Null,
            survives: false,
        },
        DeleteCase {
            label: "a tagged flat item whose span is only narrowed",
            name: "FlatNarrowed",
            charge_details: json!({ "Type": "FEE", "Flat": { "ChargeValue": 7.5 } }),
            created: vec![("stage", "test")],
            by_bare_id: false,
            range: json!({
                // The item applies to every period, so a span starting at the current
                // one leaves it applying to the periods before it — record and tags
                // included.
                "InclusiveStartBillingPeriod": current_billing_period,
            }),
            survives: true,
        },
    ];

    for case in &cases {
        let label = case.label;
        let tags = if case.created.is_empty() {
            None
        } else {
            Some(tag_map(&case.created))
        };
        let arn = em
            .create_item(case.name, &group, case.charge_details.clone(), tags)
            .await;
        let address = if case.by_bare_id {
            bare_id(&arn).to_string()
        } else {
            arn.clone()
        };

        // The item is taggable while it exists.
        let (status, body) = em.tag(&address, &tag_map(&[("doomed", "maybe")])).await;
        assert_eq!(
            status, TAGGING_STATUS,
            "case {label:?}: TagResource before the delete expected {TAGGING_STATUS}, got \
             {status} with {body}"
        );

        delete(&em, label, &arn, &case.range).await;

        if case.survives {
            let mut after = case.created.clone();
            after.push(("doomed", "maybe"));
            let kept = em.stored_tags(label, &arn).await;
            assert_eq!(
                kept,
                expected(&after),
                "case {label:?}: expected the narrowed item to keep the tags {:?}, got {kept:?}",
                expected(&after)
            );
            // Both spellings still name the surviving record.
            let by_address = em.stored_tags(label, &address).await;
            assert_eq!(
                by_address, kept,
                "case {label:?}: expected both ARN spellings to report the same tags, got \
                 {by_address:?} and {kept:?}"
            );
        } else {
            // Addressed by EITHER spelling the pattern admits, the record is gone and so
            // are its tags.
            assert_tagging_not_found(&em, label, &arn).await;
            assert_tagging_not_found(&em, label, bare_id(&arn)).await;
        }
    }

    // A new item created after the deletes carries only its own tags: nothing of a
    // deleted item's map is inherited by a later resource.
    let fresh = em
        .create_item(
            "FreshAfterDeletes",
            &group,
            json!({ "Type": "FEE", "Flat": { "ChargeValue": 1.0 } }),
            Some(tag_map(&[("fresh", "yes")])),
        )
        .await;
    let fresh_tags = em.stored_tags("a fresh item after the deletes", &fresh).await;
    assert_eq!(
        fresh_tags,
        expected(&[("fresh", "yes")]),
        "expected the fresh item to carry only its own tags, got {fresh_tags:?}"
    );
}

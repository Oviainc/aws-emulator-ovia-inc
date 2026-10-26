// SPDX-License-Identifier: CC0-1.0

//! The per-resource tag COUNT cap, over every operation that accepts a `TagMap`.
//!
//! The RULE under test, asserted over tables rather than one recorded request: a resource
//! holds at most `state::MAX_TAGS_PER_RESOURCE` tag entries, counted on the SET the
//! request would leave it holding — the entries it already carries with the request's
//! entries merged in, each key the request names taking the request's value — and a
//! request whose set would be larger is refused with HTTP 402
//! `ServiceQuotaExceededException`, writing nothing. Counting the merged set is what
//! makes re-stating keys a resource already carries always acceptable, however many
//! entries the request carries.
//!
//! The rule belongs to the tag store, not to one operation, so `CreateDomain` (whose
//! resource holds nothing yet, so its set is the entries the create carried) and
//! `TagResource` (whose set is the merge) decide it with the ONE shared comparison,
//! `service::tag_bounds::merged_within_cap`. These rows drive that one path from both
//! sides, so the two operations cannot bound different counts or count different sets.
//!
//! The rows differ along what the rule generalises over: how many entries the resource
//! already holds, how many of those the request re-states, how many new keys it adds,
//! and therefore how the request's own entry count relates to the merged total — each
//! swept at the published cap, below it and past it.
//!
//! Rows that look like a sibling suite's are deliberate and not redundant: the cap at and
//! past the count is one row of `tests/all/domain_subscription_lifecycle.rs`
//! (`store_growth_is_bounded_and_refused_with_the_modelled_errors`) for a create, and of
//! `tests/all/resource_tagging.rs` (`the_tagging_bounds_are_enforced_on_the_merged_set`)
//! for a merge, each pinning ONE operation. What only this suite asserts is that the one
//! condition decides BOTH — the same count accepted or refused identically whichever
//! operation carries it — so none of the three is a copy of another.
//!
//! Sources for every bound written by hand here:
//!
//! * `smithy_query(query="shape",shape="TagMap,TagKey,TagValue",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `TagMap` declares no `@length`, so the count limit is the service's own
//!   (`state::MAX_TAGS_PER_RESOURCE`), which this suite imports rather than restating;
//!   the per-ENTRY lengths are a different rule with its own suite
//!   (`tests/all/tag_map_bounds.rs`), so every entry here is well inside them.
//! * `smithy_query(query="digest")` — the operations whose input carries a `TagMap` are
//!   `CreateDomain` (`POST /domain`, 201) and `TagResource` (`POST /tags/{resourceArn}`,
//!   204), and `ServiceQuotaExceededException` is the shape this service models for a
//!   quota (HTTP 402).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::service::tag_bounds::merged_within_cap;
use service_com_amazonaws_connecthealth_connecthealth::state::MAX_TAGS_PER_RESOURCE;

use crate::domain_subscription_lifecycle::{error_code, string_at, tag_map, Emulator};
use crate::resource_tagging::{domain_tags, tags_path};

/// A tag map as the wire carries it.
fn tags_value(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// How many domains this emulator currently holds, so a refused `CreateDomain` can be
/// shown to have minted none.
async fn held_domains(emulator: &Emulator) -> usize {
    let (status, listed) = emulator.send("GET", "/domain", None).await;
    assert_eq!(status, 200, "ListDomains got {status} {listed}");
    listed
        .get("domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListDomains must report a domains list, got {listed}"))
        .len()
}

/// `count` entries no resource in this suite already carries, keyed apart from the ones
/// [`tag_map`] generates so an "added" key is never an overwrite.
fn added_entries(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("added-{index}"), format!("v{index}")))
        .collect()
}

/// The refusal of a set past the cap: 402 and the service's quota error.
fn assert_refused(case: &str, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "case {case:?}: a set past {MAX_TAGS_PER_RESOURCE} tags must be refused, \
         got {status} with {body}"
    );
}

#[test]
fn the_shared_cap_check_bounds_the_merged_set_rather_than_the_requests_entry_count() {
    // Each row: what the resource already holds, how many of those keys the request
    // re-states, and how many new keys it adds. Acceptance is computed from the rule —
    // the merged total against the cap — never restated per row.
    let rows: [(&str, usize, usize, usize); 10] = [
        ("an untagged resource taking nothing", 0, 0, 0),
        ("an untagged resource taking one entry", 0, 0, 1),
        ("an untagged resource taking exactly the cap", 0, 0, MAX_TAGS_PER_RESOURCE),
        (
            "an untagged resource taking one past the cap",
            0,
            0,
            MAX_TAGS_PER_RESOURCE + 1,
        ),
        ("one short of the cap taking its last slot", MAX_TAGS_PER_RESOURCE - 1, 0, 1),
        (
            "one short of the cap taking two more",
            MAX_TAGS_PER_RESOURCE - 1,
            0,
            2,
        ),
        (
            "a resource at the cap re-stating one key it holds",
            MAX_TAGS_PER_RESOURCE,
            1,
            0,
        ),
        (
            "a resource at the cap re-stating every key it holds",
            MAX_TAGS_PER_RESOURCE,
            MAX_TAGS_PER_RESOURCE,
            0,
        ),
        (
            "a resource at the cap taking one new key beside an overwrite",
            MAX_TAGS_PER_RESOURCE,
            1,
            1,
        ),
        (
            // The request carries the full cap of entries and is still accepted: most of
            // them are keys the resource already holds, so the merged set does not grow
            // past the cap.
            "a request of cap entries whose merge stays at the cap",
            10,
            10,
            MAX_TAGS_PER_RESOURCE - 10,
        ),
    ];

    for (case, held_count, restated, added) in rows {
        let held: BTreeMap<String, String> = (0..held_count)
            .map(|index| (format!("held-{index}"), format!("h{index}")))
            .collect();
        let mut requested: BTreeMap<String, String> = held
            .keys()
            .take(restated)
            .map(|key| (key.clone(), "restated".to_string()))
            .collect();
        requested.extend(added_entries(added));

        let mut expected = held.clone();
        expected.extend(requested.clone());
        let within = expected.len() <= MAX_TAGS_PER_RESOURCE;

        let bounded = merged_within_cap(held.clone(), requested.clone());
        match bounded {
            Ok(merged) => {
                assert!(
                    within,
                    "case {case:?}: a merged set of {} entries is past the cap of \
                     {MAX_TAGS_PER_RESOURCE} and must be refused, got {merged:?}",
                    expected.len()
                );
                assert_eq!(
                    merged, expected,
                    "case {case:?}: the set handed back must be the merge, got {merged:?}"
                );
            }
            Err(count) => {
                assert!(
                    !within,
                    "case {case:?}: a merged set of {} entries is within the cap of \
                     {MAX_TAGS_PER_RESOURCE} and must be accepted, got a refusal of {count}",
                    expected.len()
                );
                assert_eq!(
                    count,
                    expected.len(),
                    "case {case:?}: the refusal must report the count the resource would \
                     have held, got {count}"
                );
            }
        }
    }
}

#[tokio::test]
async fn create_domain_bounds_the_tag_set_its_create_would_leave_the_domain_holding() {
    // Every count around the published cap: a create's domain holds no tags yet, so the
    // set bounded is the entries the request carried.
    let counts = [
        0,
        1,
        MAX_TAGS_PER_RESOURCE - 1,
        MAX_TAGS_PER_RESOURCE,
        MAX_TAGS_PER_RESOURCE + 1,
        MAX_TAGS_PER_RESOURCE + 2,
        MAX_TAGS_PER_RESOURCE * 2,
    ];
    for count in counts {
        let case = format!("a create carrying {count} tags");
        // A fresh emulator per row, so no row's domains count against the next row's
        // account-wide domain cap.
        let emulator = Emulator::new("us-east-1");
        let before = held_domains(&emulator).await;
        let (status, body) = emulator
            .send(
                "POST",
                "/domain",
                Some(&json!({ "name": "clinic", "tags": tag_map(count, 8, 8) })),
            )
            .await;
        let within = count <= MAX_TAGS_PER_RESOURCE;
        if within {
            assert_eq!(
                status, 201,
                "case {case:?}: a set within {MAX_TAGS_PER_RESOURCE} tags must be \
                 accepted, got {status} with {body}"
            );
            // Read the domain back through its own reader: every entry the create
            // carried is what the stored record reports.
            let reported = domain_tags(&emulator, &string_at(&body, "domainId")).await;
            assert_eq!(
                reported.len(),
                count,
                "case {case:?}: the stored set must hold every entry the create carried, \
                 got {reported:?}"
            );
        } else {
            assert_refused(&case, status, &body);
            let after = held_domains(&emulator).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused create must mint no domain, got {after} held"
            );
        }

        // The SAME count through `TagResource`, against a domain that holds no tags
        // either — the other side of the one shared comparison, which must therefore
        // decide the same way.
        let arn = string_at(
            &emulator
                .create_domain(&json!({ "name": "clinic-untagged" }))
                .await,
            "arn",
        );
        let (tag_status, tag_body) = emulator
            .send(
                "POST",
                &tags_path(&arn),
                Some(&json!({ "tags": tag_map(count, 8, 8) })),
            )
            .await;
        if within {
            assert_eq!(
                tag_status, 204,
                "case {case:?}: TagResource must accept the same count, got {tag_status} \
                 with {tag_body}"
            );
        } else {
            assert_refused(&format!("{case} (through TagResource)"), tag_status, &tag_body);
        }
    }
}

#[tokio::test]
async fn tag_resource_bounds_the_merged_set_through_the_same_shared_check() {
    // Each row: how many tags the domain is created with, how many of those keys the
    // `TagResource` call re-states, and how many new keys it adds. The expectation is
    // computed by applying the rule to the merge, not restated per row.
    let rows: [(&str, usize, usize, usize); 8] = [
        ("a first entry on an untagged domain", 0, 0, 1),
        ("the whole cap at once on an untagged domain", 0, 0, MAX_TAGS_PER_RESOURCE),
        (
            "one past the cap at once on an untagged domain",
            0,
            0,
            MAX_TAGS_PER_RESOURCE + 1,
        ),
        ("the last free slot", MAX_TAGS_PER_RESOURCE - 1, 0, 1),
        ("two keys for one free slot", MAX_TAGS_PER_RESOURCE - 1, 0, 2),
        ("an overwrite at the cap", MAX_TAGS_PER_RESOURCE, 1, 0),
        ("a new key at the cap", MAX_TAGS_PER_RESOURCE, 0, 1),
        (
            // The request carries as many entries as a resource may hold and is still
            // accepted: the count is on the merged SET, not on the request.
            "a request of cap entries that only re-states and tops up",
            20,
            20,
            MAX_TAGS_PER_RESOURCE - 20,
        ),
    ];

    for (case, seeded, restated, added) in rows {
        let emulator = Emulator::new("us-east-1");
        let created = emulator
            .create_domain(&json!({ "name": "clinic", "tags": tag_map(seeded, 8, 8) }))
            .await;
        let domain_id = string_at(&created, "domainId");
        let arn = string_at(&created, "arn");
        let held = domain_tags(&emulator, &domain_id).await;
        assert_eq!(
            held.len(),
            seeded,
            "case {case:?}: the seeded domain must hold {seeded} tags, got {held:?}"
        );

        let mut requested: BTreeMap<String, String> = held
            .keys()
            .take(restated)
            .map(|key| (key.clone(), "restated".to_string()))
            .collect();
        requested.extend(added_entries(added));
        let mut merged = held.clone();
        merged.extend(requested.clone());
        let within = merged.len() <= MAX_TAGS_PER_RESOURCE;

        let (status, body) = emulator
            .send(
                "POST",
                &tags_path(&arn),
                Some(&json!({ "tags": tags_value(&requested) })),
            )
            .await;
        let reported = domain_tags(&emulator, &domain_id).await;
        if within {
            assert_eq!(
                status, 204,
                "case {case:?}: a merge to {} entries must be accepted, got {status} \
                 with {body}",
                merged.len()
            );
            assert_eq!(
                reported, merged,
                "case {case:?}: the stored set must be the merge, got {reported:?}"
            );
        } else {
            assert_refused(case, status, &body);
            assert_eq!(
                reported, held,
                "case {case:?}: a refused merge must write nothing, got {reported:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a `TagMap` earns when it breaks more than one of the rules the shape is
//! held to — over every operation that accepts one.
//!
//! The RULE under test, asserted over tables rather than one recorded request: the
//! per-ENTRY rule (the modelled `TagKey` / `TagValue` lengths and the reserved key
//! namespace, refused 400 `ValidationException`) is decided from the REQUEST alone, so it
//! is decided before anything the service has to read state for — before the resource the
//! request addresses is resolved (404 `ResourceNotFoundException`) and before the
//! per-RESOURCE count is compared (402 `ServiceQuotaExceededException`), which is the
//! order a modelled constraint is decided in at the request boundary, before a handler
//! runs at all. One map therefore earns ONE status and ONE error code whichever operation
//! carries it: a map that is both past the count and carrying an unacceptable entry is
//! refused for the entry, by `CreateDomain` and by `TagResource` alike, in the same
//! sentence.
//!
//! The rows differ along what the rule generalises over: which operation carries the map,
//! WHICH entry rule the map breaks (each end of each modelled length, in an ASCII and in
//! a multi-byte form, and the reserved namespace), HOW MANY of its entries are
//! unacceptable (one, or every kind at once), WHERE the unacceptable entry falls in key
//! order (first, middle, last), how the map's entry count relates to the published cap
//! (inside it, exactly at it, one past it, twice it), whether the addressed resource
//! already holds tags, and whether the ARN resolves at all. `UntagResource`'s `tagKeys`
//! bounds no count, so it is swept along the one precedence it has: its per-key rule
//! against the ARN it addresses.
//!
//! Sources for every bound and catalog written by hand here:
//!
//! * `smithy_query(query="shape",shape="TagMap,TagKey,TagValue",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `TagMap`'s key targets `TagKey` (`@length` 1..128), its value targets `TagValue`
//!   (`@length` 0..256), and `TagMap` itself declares no `@length`, so the per-resource
//!   count is the service's own (`state::MAX_TAGS_PER_RESOURCE`) and the reserved key
//!   namespace (`state::RESERVED_TAG_KEY_PREFIX`) is too. Every number and the prefix are
//!   imported from the crate rather than restated here.
//! * `smithy_query(query="digest")` — the operations whose input carries a `TagMap` are
//!   `CreateDomain` (`POST /domain`, 201) and `TagResource` (`POST /tags/{resourceArn}`,
//!   204), and the one that names tag keys for removal is `UntagResource`
//!   (`DELETE /tags/{resourceArn}`, 204); `ValidationException` is HTTP 400,
//!   `ResourceNotFoundException` 404 and `ServiceQuotaExceededException` 402.
//!
//! What the sibling suites pin is each rule ON ITS OWN: the entry lengths
//! (`tests/all/tag_map_bounds.rs`), the reserved namespace
//! (`tests/all/reserved_tag_key_prefix.rs`) and the count
//! (`tests/all/tag_count_cap.rs`), each with maps that break exactly one of them. No row
//! of any of them carries a map that breaks TWO at once, which is the only thing this
//! suite asserts, so none of them covers it.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::service::tag_bounds::accepted_entries;
use service_com_amazonaws_connecthealth_connecthealth::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN, MIN_TAG_KEY_LEN,
    RESERVED_TAG_KEY_PREFIX,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, tag_map, Emulator};
use crate::resource_tag_removal::untag_path;
use crate::resource_tagging::{domain_tags, entries, tags_path};
use crate::tag_count_cap::held_domains;
use crate::tag_entry_refusal_completeness::{at_least, at_most, clause, envelope};
use crate::tag_map_bounds::detail;

/// Which refusal — or none — the rule gives a map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The per-entry rule: HTTP 400 `ValidationException`.
    Entry,
    /// The per-resource count: HTTP 402 `ServiceQuotaExceededException`.
    Count,
    /// Every rule satisfied.
    Accepted,
}

/// The rule itself, applied to one map: the entry rule first — it needs nothing but the
/// request — then the count of the set the resource would hold.
fn expected(unacceptable_entries: usize, merged_count: usize) -> Expected {
    if unacceptable_entries > 0 {
        Expected::Entry
    } else if merged_count > MAX_TAGS_PER_RESOURCE {
        Expected::Count
    } else {
        Expected::Accepted
    }
}

/// One entry the service does not accept: a label, the entry itself, and whether the
/// HANDLER is what refuses it.
///
/// A map's KEY is written against the map member's own schema and captured as the
/// following value's path segment rather than checked, so every rule on a key reaches the
/// handler, which refuses it through the shared check; a map's VALUE is checked by the
/// walk itself, through the element constraints the collection member's schema carries,
/// so an over-long one is refused at the request boundary before the handler runs at all
/// (`vendor/smithy-schema-server/src/constraints.rs`, `ValidatingSerializer::write_leaf`,
/// `check_leaf`). Either way the refusal is the modelled 400 and both operations answer
/// it identically — which is what the rows below assert; only the sentence's author
/// differs, so only a handler-refused entry can be compared to the shared check's own
/// text.
type UnacceptableEntry = (&'static str, String, String, bool);

/// Every way ONE entry can be unacceptable, each derived from the bound it falls outside
/// rather than from a recorded request: both ends of the key length, the top of the value
/// length (its minimum is 0, so no value is below it), the same key bound in a multi-byte
/// form — `@length` counts characters — and the reserved key namespace.
fn unacceptable_entries() -> Vec<UnacceptableEntry> {
    vec![
        (
            "a key one character below the modelled minimum",
            "k".repeat(MIN_TAG_KEY_LEN - 1),
            "v".to_string(),
            true,
        ),
        (
            "a key one character past the modelled maximum",
            "k".repeat(MAX_TAG_KEY_LEN + 1),
            "v".to_string(),
            true,
        ),
        (
            "a multi-byte key one character past the modelled maximum",
            "é".repeat(MAX_TAG_KEY_LEN + 1),
            "v".to_string(),
            true,
        ),
        (
            "a value one character past the modelled maximum",
            "zz-long-value".to_string(),
            "v".repeat(MAX_TAG_VALUE_LEN + 1),
            false,
        ),
        (
            "a key in the reserved namespace",
            format!("{RESERVED_TAG_KEY_PREFIX}owner"),
            "mine".to_string(),
            true,
        ),
    ]
}

/// `base` with `extra` merged in, as the wire carries one map.
fn with_entries(base: &Value, extra: &[(String, String)]) -> Value {
    let mut map = base
        .as_object()
        .unwrap_or_else(|| panic!("a tags map, got {base}"))
        .clone();
    for (key, value) in extra {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// The refusal text the shared seam gives this map, which is what both operations have to
/// answer with: the request's entries in key order, refused for the first the service does
/// not accept.
fn seam_refusal(tags: &Value) -> Option<String> {
    accepted_entries(entries(tags)).err()
}

/// What one operation answered for a map, asserted against the rule — and, when the
/// handler is the one that refuses the entry, against the exact sentence the shared check
/// gives that map, so neither operation can word it its own way.
fn assert_answer(
    case: &str,
    want: Expected,
    tags: &Value,
    handler_refused: bool,
    status: u16,
    body: &Value,
) {
    match want {
        Expected::Entry => {
            assert_eq!(
                (status, error_code(body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: an unacceptable entry must be refused as the modelled \
                 400 whatever else the map breaks, got {status} with {body}"
            );
            let message = detail(case, body);
            assert!(
                message.contains("tags"),
                "case {case:?}: the refusal must name the member the map was bound to, \
                 got {message:?}"
            );
            let seam = seam_refusal(tags);
            assert!(
                seam.is_some(),
                "case {case:?}: the shared entry check must refuse this map too, got \
                 nothing for {tags}"
            );
            if handler_refused {
                assert_eq!(
                    Some(message.as_str()),
                    seam.as_deref(),
                    "case {case:?}: the refusal must be the one the shared entry check \
                     gives this map, got {message:?}"
                );
            }
        }
        Expected::Count => assert_eq!(
            (status, error_code(body).as_str()),
            (402, "ServiceQuotaExceededException"),
            "case {case:?}: a set past {MAX_TAGS_PER_RESOURCE} entries whose every entry \
             is acceptable must be refused as the modelled 402, got {status} with {body}"
        ),
        Expected::Accepted => panic!("case {case:?}: an accepted map is asserted by its caller"),
    }
}

/// Each row of the main table: a label, the map one call carries, how many of its entries
/// the service does not accept, and whether the HANDLER is what refuses the first of them
/// (see [`UnacceptableEntry`]).
type Row = (String, Value, usize, bool);

fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // --- Maps carrying ONE unacceptable entry, swept against the count cap: inside it,
    // exactly at it, one past it and twice it. Only the count varies between the four, so
    // the answer may not.
    for (entry_case, key, value, handler_refused) in unacceptable_entries() {
        for (fill_case, acceptable_entries) in [
            ("well inside the cap", 1),
            ("exactly at the cap", MAX_TAGS_PER_RESOURCE - 1),
            ("one past the cap", MAX_TAGS_PER_RESOURCE),
            ("twice the cap", MAX_TAGS_PER_RESOURCE * 2),
        ] {
            rows.push((
                format!("{entry_case}, in a map {fill_case}"),
                with_entries(
                    &tag_map(acceptable_entries, 8, 8),
                    &[(key.clone(), value.clone())],
                ),
                1,
                handler_refused,
            ));
        }
    }

    // --- A map carrying EVERY kind of unacceptable entry at once, past the cap: several
    // rules broken several times over, and still one refusal. One of the kinds is a value
    // the request boundary refuses, so the boundary is what words this map's refusal.
    let every_kind: Vec<(String, String)> = unacceptable_entries()
        .into_iter()
        .map(|(_, key, value, _)| (key, value))
        .collect();
    let kinds = every_kind.len();
    for (fill_case, acceptable_entries) in [
        ("inside the cap", 1),
        ("past the cap", MAX_TAGS_PER_RESOURCE),
    ] {
        rows.push((
            format!("every kind of unacceptable entry at once, in a map {fill_case}"),
            with_entries(&tag_map(acceptable_entries, 8, 8), &every_kind),
            kinds,
            false,
        ));
    }

    // --- The unacceptable entry at each position of the key ORDER the entries are walked
    // in: first, among them, and last. The key generated by `tag_map` starts with `k`, so
    // a reserved key (`aws:`) sorts before every acceptable one, a `k`-key of the maximum
    // length sorts among them and `zz-…` sorts after them.
    for (position, key, value, handler_refused) in [
        (
            "before every acceptable key",
            format!("{RESERVED_TAG_KEY_PREFIX}owner"),
            "mine".to_string(),
            true,
        ),
        (
            "among the acceptable keys",
            "k".repeat(MAX_TAG_KEY_LEN + 1),
            "v".to_string(),
            true,
        ),
        (
            "after every acceptable key",
            "zz-long-value".to_string(),
            "v".repeat(MAX_TAG_VALUE_LEN + 1),
            false,
        ),
    ] {
        rows.push((
            format!("an unacceptable entry sorting {position}, in a map past the cap"),
            with_entries(&tag_map(MAX_TAGS_PER_RESOURCE, 8, 8), &[(key, value)]),
            1,
            handler_refused,
        ));
    }

    // --- Controls: maps that break ONE rule or none, so the table also shows the two
    // refusals are still told apart and an acceptable map is still accepted.
    for (case, acceptable_entries) in [
        ("no entries at all", 0),
        ("one acceptable entry", 1),
        ("acceptable entries exactly at the cap", MAX_TAGS_PER_RESOURCE),
        ("acceptable entries one past the cap", MAX_TAGS_PER_RESOURCE + 1),
        ("acceptable entries twice the cap", MAX_TAGS_PER_RESOURCE * 2),
    ] {
        rows.push((
            case.to_string(),
            tag_map(acceptable_entries, 8, 8),
            0,
            false,
        ));
    }

    rows
}

#[tokio::test]
async fn both_operations_refuse_a_map_that_breaks_two_rules_for_its_entry_and_word_it_identically()
{
    for (case, tags, unacceptable, handler_refused) in rows() {
        // A fresh emulator per row, so no row's domains count against the next row's
        // account-wide domain cap and no row's tag set is what a later row reads.
        let emulator = Emulator::new("us-east-1");
        let pairs: BTreeMap<String, String> = entries(&tags).into_iter().collect();
        let want = expected(unacceptable, pairs.len());

        // --- CreateDomain carries the map on the create itself: the domain it would
        // create holds no tags, so the set the count is compared on is the map.
        let before = held_domains(&emulator).await;
        let (status, created) = emulator
            .send(
                "POST",
                "/domain",
                Some(&json!({ "name": "clinic", "tags": tags })),
            )
            .await;
        let create_refusal = if want == Expected::Accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: a map breaking no rule must be accepted, got {status} \
                 with {created}"
            );
            let reported = domain_tags(&emulator, &string_at(&created, "domainId")).await;
            assert_eq!(
                reported, pairs,
                "case {case:?}: the stored set must be the entries the create carried, \
                 got {reported:?}"
            );
            None
        } else {
            assert_answer(
                &format!("{case} (CreateDomain)"),
                want,
                &tags,
                handler_refused,
                status,
                &created,
            );
            let after = held_domains(&emulator).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused create must mint no domain, got {after} held"
            );
            Some(detail(&case, &created))
        };

        // --- TagResource carries the same map against a domain that exists and holds no
        // tags, so the count is compared on the same set and the two answers must match.
        let created = emulator
            .create_domain(&json!({ "name": "clinic-untagged" }))
            .await;
        let domain_id = string_at(&created, "domainId");
        let arn = string_at(&created, "arn");
        let (status, body) = emulator
            .send("POST", &tags_path(&arn), Some(&json!({ "tags": tags })))
            .await;
        let reported = domain_tags(&emulator, &domain_id).await;
        if want == Expected::Accepted {
            assert_eq!(
                status, 204,
                "case {case:?}: TagResource must accept the same map, got {status} \
                 with {body}"
            );
            assert_eq!(
                reported, pairs,
                "case {case:?}: the merged set must be the entries the call carried, \
                 got {reported:?}"
            );
        } else {
            assert_answer(
                &format!("{case} (TagResource)"),
                want,
                &tags,
                handler_refused,
                status,
                &body,
            );
            assert!(
                reported.is_empty(),
                "case {case:?}: a refused merge must write nothing, got {reported:?}"
            );
            if want == Expected::Entry {
                assert_eq!(
                    create_refusal.as_deref(),
                    Some(detail(&case, &body).as_str()),
                    "case {case:?}: one map must earn one sentence from both operations"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_entry_rule_outranks_the_count_on_a_resource_that_already_holds_tags() {
    // The entry rule does not depend on what the resource holds, so a merge into a
    // non-empty set answers the same way. Each row: how many acceptable entries the
    // domain is created with, and how many the merge adds beside its unacceptable entry.
    for (case, seeded, added) in [
        ("a merge into an empty set", 0, MAX_TAGS_PER_RESOURCE),
        ("a merge one short of the cap", MAX_TAGS_PER_RESOURCE - 1, 0),
        ("a merge the acceptable half of which fits", 10, 10),
        (
            "a merge the acceptable half of which is already past the cap",
            MAX_TAGS_PER_RESOURCE,
            MAX_TAGS_PER_RESOURCE,
        ),
    ] {
        for (entry_case, key, value, handler_refused) in unacceptable_entries() {
            let label = format!("{case}, carrying {entry_case}");
            let emulator = Emulator::new("eu-central-1");
            // The seeded set is keyed apart from the merge's own entries, so an added key
            // is never an overwrite.
            let seeded_tags: Value = Value::Object(
                (0..seeded)
                    .map(|index| {
                        (
                            format!("held-{index}"),
                            Value::String(format!("h{index}")),
                        )
                    })
                    .collect(),
            );
            let created = emulator
                .create_domain(&json!({ "name": "clinic", "tags": seeded_tags }))
                .await;
            let domain_id = string_at(&created, "domainId");
            let arn = string_at(&created, "arn");
            let held = domain_tags(&emulator, &domain_id).await;
            assert_eq!(
                held.len(),
                seeded,
                "case {label:?}: the seeded domain must hold {seeded} entries, got {held:?}"
            );

            let tags = with_entries(&tag_map(added, 8, 8), &[(key, value)]);
            let (status, body) = emulator
                .send("POST", &tags_path(&arn), Some(&json!({ "tags": tags })))
                .await;
            assert_answer(&label, Expected::Entry, &tags, handler_refused, status, &body);
            let reported = domain_tags(&emulator, &domain_id).await;
            assert_eq!(
                reported, held,
                "case {label:?}: a refused merge must leave the set it would have joined \
                 exactly as it was, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_entry_rule_is_decided_before_the_arn_is_resolved_and_the_count_after_it() {
    let emulator = Emulator::new("us-west-2");
    let created = emulator.create_domain(&json!({ "name": "clinic" })).await;
    let domain_id = string_at(&created, "domainId");
    let live_arn = string_at(&created, "arn");
    // A well-formed ARN of this service, in this Region and account, whose id names no
    // domain the store holds — so the only thing wrong with it is the resource it names.
    let absent_arn = live_arn.replace(&domain_id, "dom-zzzzzzzzzzzzzzzz");

    // An acceptable map against that ARN: nothing about the request is wrong, so the
    // answer is the missing resource.
    let (status, body) = emulator
        .send(
            "POST",
            &tags_path(&absent_arn),
            Some(&json!({ "tags": { "env": "prod" } })),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "an acceptable map on an ARN that names no resource must answer the modelled 404, \
         got {status} with {body}"
    );

    // A map past the cap against the same ARN: the count is counted on the set the
    // ADDRESSED resource would hold, so it cannot be decided before the ARN is — the
    // answer is still the missing resource.
    let (status, body) = emulator
        .send(
            "POST",
            &tags_path(&absent_arn),
            Some(&json!({ "tags": tag_map(MAX_TAGS_PER_RESOURCE + 1, 8, 8) })),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "a map past the cap on an ARN that names no resource must answer the modelled 404, \
         got {status} with {body}"
    );

    // An unacceptable ENTRY against the same ARN: that rule needs nothing but the request,
    // so it is decided first — whatever else the map breaks and whether or not the ARN
    // resolves.
    for (entry_case, key, value, handler_refused) in unacceptable_entries() {
        for (fill_case, acceptable_entries) in [
            ("inside the cap", 1),
            ("past the cap", MAX_TAGS_PER_RESOURCE),
        ] {
            let case = format!("{entry_case}, in a map {fill_case}, on an absent ARN");
            let tags = with_entries(
                &tag_map(acceptable_entries, 8, 8),
                &[(key.clone(), value.clone())],
            );
            let (status, body) = emulator
                .send(
                    "POST",
                    &tags_path(&absent_arn),
                    Some(&json!({ "tags": tags })),
                )
                .await;
            assert_answer(&case, Expected::Entry, &tags, handler_refused, status, &body);
        }
    }

    // The same precedence for the keys an `UntagResource` names for REMOVAL: that
    // operation bounds no count, but its per-key rule needs nothing but the request
    // either, so it is decided before the ARN is resolved — while an acceptable key on
    // the same ARN still answers the missing resource.
    let (status, body) = emulator
        .send(
            "DELETE",
            &untag_path(&absent_arn, &["env".to_string()]),
            None,
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "an acceptable key on an ARN that names no resource must answer the modelled 404, \
         got {status} with {body}"
    );
    for (case, key) in [
        (
            "a key one character past the modelled maximum",
            "k".repeat(MAX_TAG_KEY_LEN + 1),
        ),
        (
            "a key in the reserved namespace",
            format!("{RESERVED_TAG_KEY_PREFIX}owner"),
        ),
    ] {
        let case = format!("{case}, named for removal on an absent ARN");
        let (status, body) = emulator
            .send(
                "DELETE",
                &untag_path(&absent_arn, &["env".to_string(), key.clone()]),
                None,
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a key this service does not accept must be refused as the \
             modelled 400 before the ARN is resolved, got {status} with {body}"
        );
        let message = detail(&case, &body);
        assert!(
            message.contains("tagKeys"),
            "case {case:?}: the refusal must name the member that carried the key, \
             got {message:?}"
        );
    }
}

#[test]
fn the_shared_entry_check_reports_one_map_one_way_however_the_request_ordered_it() {
    // The entries of a map arrive in no particular order, so the check orders them by key
    // before walking them and names every entry it refuses: one map earns one refusal,
    // stating every clause its entries earn in key order, whatever order the request
    // listed them in.
    let unacceptable = unacceptable_entries();
    let forwards: Vec<(String, String)> = unacceptable
        .iter()
        .map(|(_, key, value, _)| (key.clone(), value.clone()))
        .collect();
    let mut backwards = forwards.clone();
    backwards.reverse();

    let first_by_key = forwards
        .iter()
        .min_by(|left, right| left.0.cmp(&right.0))
        .cloned()
        .expect("the catalog is not empty");
    let one_entry_refusal = accepted_entries([first_by_key.clone()])
        .expect_err("the catalog's entries are unacceptable");
    // And that sentence itself, written out here rather than only compared to the check's
    // own output: the framework's envelope over the clause naming the value, the member
    // the map is bound to and the bound that was violated. The catalog's first key by
    // order is the one below the modelled minimum, so this is the sentence a map of that
    // entry alone earns.
    let worded = format!(
        "1 validation error detected: Value '' at 'tags' failed to satisfy constraint: \
         Member must have length greater than or equal to {MIN_TAG_KEY_LEN}"
    );
    assert_eq!(
        one_entry_refusal, worded,
        "the refusal must state the violated bound the way a boundary refusal does, \
         got {one_entry_refusal:?}"
    );

    // The refusal a map of ALL of them earns, worded here from the bounds themselves
    // rather than taken from the check's own output: one clause per entry outside a
    // modelled length, in key order, under a count that agrees with how many are stated.
    // The reserved-namespace entry contributes none — its sentence is the service's own
    // rather than one of AWS's envelope clauses, so it is not mixed into an envelope the
    // map already earns.
    let mut key_ordered = forwards.clone();
    key_ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let clauses: Vec<String> = key_ordered
        .iter()
        .filter_map(|(key, value)| {
            if key.chars().count() < MIN_TAG_KEY_LEN {
                Some(clause(key, "tags", &at_least(MIN_TAG_KEY_LEN)))
            } else if key.chars().count() > MAX_TAG_KEY_LEN {
                Some(clause(key, "tags", &at_most(MAX_TAG_KEY_LEN)))
            } else if value.chars().count() > MAX_TAG_VALUE_LEN {
                Some(clause(
                    value,
                    &format!("tags.{key}.member"),
                    &at_most(MAX_TAG_VALUE_LEN),
                ))
            } else {
                // The reserved namespace: refused, but by no modelled constraint.
                None
            }
        })
        .collect();
    assert!(
        clauses.len() > 1,
        "the catalog must carry several entries outside a modelled length, so the \
         composed refusal is a multi-clause one, got {clauses:?}"
    );
    let expected_refusal = envelope(&clauses);

    for (case, pairs) in [
        ("the catalog in order", forwards.clone()),
        ("the catalog reversed", backwards),
    ] {
        let reported = accepted_entries(pairs).expect_err("an unacceptable entry");
        assert_eq!(
            reported, expected_refusal,
            "case {case:?}: the refusal must name every unacceptable entry in key order, \
             starting with {:?}, got {reported:?}",
            first_by_key.0
        );
    }

    // And a map whose every entry is acceptable comes back as the set the request asked
    // for — a round trip through the check, in key order.
    let acceptable: Vec<(String, String)> = vec![
        ("env".to_string(), "prod".to_string()),
        ("k".repeat(MAX_TAG_KEY_LEN), "v".to_string()),
        ("retired".to_string(), String::new()),
        ("long".to_string(), "v".repeat(MAX_TAG_VALUE_LEN)),
    ];
    let expected_set: BTreeMap<String, String> = acceptable.iter().cloned().collect();
    let mut reversed = acceptable.clone();
    reversed.reverse();
    for (case, pairs) in [("in order", acceptable), ("reversed", reversed)] {
        let accepted = accepted_entries(pairs).expect("every entry is acceptable");
        assert_eq!(
            accepted, expected_set,
            "case {case:?}: an acceptable map must come back as the set it asked for, \
             got {accepted:?}"
        );
    }
}

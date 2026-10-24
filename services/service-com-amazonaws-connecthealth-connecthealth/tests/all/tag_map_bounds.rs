// SPDX-License-Identifier: CC0-1.0

//! The modelled bounds of a `TagMap` entry, over EVERY operation that accepts one.
//!
//! The RULE under test, asserted over tables and a sweep rather than one recorded
//! request: a `tags` entry is accepted only when its key and its value both lie INSIDE
//! the modelled lengths of `TagKey` (1..128 characters) and `TagValue` (0..256), at both
//! ends of each; an entry outside either bound is refused with HTTP 400
//! `ValidationException` naming the constraint, and nothing the refused request carried
//! is persisted. The rule belongs to the `TagMap` shape, not to one operation, so every
//! operation that accepts one — `CreateDomain`'s `tags` and `TagResource`'s `tags` —
//! enforces it identically, and the two word the same violation the same way.
//!
//! The constraint reaches a handler unchecked: the framework's boundary walk captures a
//! map's KEY as the following value's path segment rather than validating it, and
//! suppresses the per-element check on a map's VALUE
//! (`vendor/smithy-schema-server/src/constraints.rs`,
//! `ValidatingSerializer::write_leaf`), so the handlers are what must refuse it and
//! these are the tests that pin them.
//!
//! The rows differ along what the rule generalises over: which operation carries the map,
//! which end of which bound the entry falls outside, the LENGTH of the key and of the
//! value (swept over every interesting length around each published bound), the FORM of
//! the key and the value (ASCII, multi-byte, whitespace, empty), and how many entries one
//! map carries (so a legal entry beside an illegal one is not persisted either).
//!
//! Sources for every bound written by hand here:
//!
//! * `smithy_query(query="shape",shape="TagMap,TagKey,TagValue",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `TagMap`'s key targets `TagKey` (`@length` min 1, max 128) and its value targets
//!   `TagValue` (`@length` min 0, max 256); the crate states those four numbers as
//!   `state::MIN_TAG_KEY_LEN` / `MAX_TAG_KEY_LEN` / `MIN_TAG_VALUE_LEN` /
//!   `MAX_TAG_VALUE_LEN`, which is what this suite imports rather than restating, and
//!   `service::tag_bounds::violation` is the one check both handlers run.
//! * `smithy_query(query="digest")` — the operations whose input carries a `TagMap` are
//!   `CreateDomain` (`POST /domain`, 201) and `TagResource` (`POST /tags/{resourceArn}`,
//!   204); `UntagResource` carries a `TagKeyList` and is still a 501 stub, so it has no
//!   row here.
//!
//! The length sweep is EXHAUSTIVE over the lengths that can distinguish the rule (every
//! length from 0 to two past each published bound) rather than randomly sampled: the rule
//! generalises over one ordered dimension, so walking it leaves no gap for a generator to
//! find.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::service::tag_bounds::violation as tag_bounds_violation;
use service_com_amazonaws_connecthealth_connecthealth::state::{
    MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN, MIN_TAG_KEY_LEN, MIN_TAG_VALUE_LEN,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::list_tags_for_resource::list_tags;
use crate::resource_tagging::{domain_tags, entries, tags_path};

/// One row of the multi-entry table: a label, the entries one map carries, and whether
/// the rule accepts every one of them.
type EntriesCase<'t> = (&'t str, Vec<(&'t String, &'t String)>, bool);

/// A `tags` map of exactly one entry, built from a key and a value this suite computes.
fn one_tag(key: &str, value: &str) -> Value {
    json!({ key.to_string(): value })
}

/// `character` repeated until the string is `length` CHARACTERS long — the unit
/// `@length` on a string counts, which is why a multi-byte row is not the same test as
/// an ASCII one.
fn of_length(character: char, length: usize) -> String {
    character.to_string().repeat(length)
}

/// The ids `ListDomains` reports, so a refused `CreateDomain` can be shown to have
/// persisted nothing at all.
async fn listed_domain_ids(emulator: &Emulator) -> Vec<String> {
    let (status, listed) = emulator.send("GET", "/domain", None).await;
    assert_eq!(status, 200, "ListDomains got {status} {listed}");
    listed
        .get("domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListDomains must report a domains list, got {listed}"))
        .iter()
        .map(|summary| string_at(summary, "domainId"))
        .collect()
}

/// The detail text of a refusal, in the member this service reports it in.
fn detail(case: &str, body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal must carry a Message, got {body}"))
        .to_string()
}

/// A refusal of a tag entry: 400, the service's `ValidationException`, and a message
/// naming the length constraint that was violated. The text itself is not the
/// requirement — the status, the code and the fact that the sentence states the violated
/// constraint are.
fn assert_refused(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: got {body}"
    );
    let message = detail(case, body);
    assert!(
        message.contains("failed to satisfy constraint")
            && message.contains("Member must have length"),
        "case {case:?}: the refusal must name the length constraint, got {message:?}"
    );
    message
}

/// Every length around each published bound, and whether an entry of that length is
/// inside the bound. One row per length, so acceptance is computed from the bound rather
/// than restated per case.
fn length_sweep(min: usize, max: usize) -> Vec<(usize, bool)> {
    let mut lengths: Vec<usize> = (0..=min.saturating_add(2)).collect();
    lengths.extend(max.saturating_sub(2)..=max.saturating_add(2));
    lengths.sort_unstable();
    lengths.dedup();
    lengths
        .into_iter()
        .map(|length| (length, (min..=max).contains(&length)))
        .collect()
}

#[test]
fn the_shared_tag_bounds_check_refuses_exactly_the_entries_outside_the_modelled_lengths() {
    // The one check both handlers run, swept over every length that can distinguish the
    // rule, in an ASCII form and in a multi-byte form (so a byte count cannot pass for a
    // character count).
    for (form, character) in [("ASCII", 'k'), ("multi-byte", 'é'), ("wide", '日')] {
        for (length, inside) in length_sweep(MIN_TAG_KEY_LEN, MAX_TAG_KEY_LEN) {
            let key = of_length(character, length);
            let value = "v".to_string();
            let violation = tag_bounds_violation([(&key, &value)]);
            let case = format!("a {form} key of {length} characters");
            assert_eq!(
                violation.is_none(),
                inside,
                "case {case:?}: a key is {MIN_TAG_KEY_LEN}..={MAX_TAG_KEY_LEN} characters, \
                 got {violation:?}"
            );
        }
        for (length, inside) in length_sweep(MIN_TAG_VALUE_LEN, MAX_TAG_VALUE_LEN) {
            let key = "k".to_string();
            let value = of_length(character, length);
            let violation = tag_bounds_violation([(&key, &value)]);
            let case = format!("a {form} value of {length} characters");
            assert_eq!(
                violation.is_none(),
                inside,
                "case {case:?}: a value is {MIN_TAG_VALUE_LEN}..={MAX_TAG_VALUE_LEN} \
                 characters, got {violation:?}"
            );
        }
    }

    // A map of several entries is refused for whichever entry is outside the bounds,
    // whatever the others are, and accepted only when every entry is inside them.
    let legal_key = "env".to_string();
    let legal_value = "prod".to_string();
    let empty_key = String::new();
    let long_value = of_length('v', MAX_TAG_VALUE_LEN + 1);
    let cases: Vec<EntriesCase<'_>> = vec![
        (
            "every entry inside the bounds",
            vec![(&legal_key, &legal_value), (&legal_key, &empty_key)],
            true,
        ),
        (
            "a legal entry beside an empty key",
            vec![(&legal_key, &legal_value), (&empty_key, &legal_value)],
            false,
        ),
        (
            "a legal entry beside an over-long value",
            vec![(&legal_key, &legal_value), (&legal_key, &long_value)],
            false,
        ),
        ("no entries at all", vec![], true),
    ];
    for (case, entries, inside) in cases {
        let violation = tag_bounds_violation(entries);
        assert_eq!(
            violation.is_none(),
            inside,
            "case {case:?}: expected inside={inside}, got {violation:?}"
        );
    }
}

#[tokio::test]
async fn every_operation_that_accepts_a_tag_map_enforces_the_same_entry_bounds() {
    let longest_key = of_length('k', MAX_TAG_KEY_LEN);
    let past_longest_key = of_length('k', MAX_TAG_KEY_LEN + 1);
    let longest_value = of_length('v', MAX_TAG_VALUE_LEN);
    let past_longest_value = of_length('v', MAX_TAG_VALUE_LEN + 1);
    // `@length` counts characters, so a key of 128 multi-byte characters is INSIDE the
    // bound however many bytes it occupies, and one of 129 is outside it.
    let longest_key_multibyte = of_length('é', MAX_TAG_KEY_LEN);
    let past_longest_key_multibyte = of_length('é', MAX_TAG_KEY_LEN + 1);
    let longest_value_multibyte = of_length('é', MAX_TAG_VALUE_LEN);

    // Each row: the map one call carries, and whether the rule accepts it.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("one ordinary entry", one_tag("env", "prod"), true),
        (
            "a key at the modelled minimum of 1 character",
            one_tag("k", "v"),
            true,
        ),
        (
            "an empty key, one below the modelled minimum of 1",
            one_tag("", "v"),
            false,
        ),
        (
            "an empty key and an empty value",
            one_tag("", ""),
            false,
        ),
        (
            "a one-character key that is only whitespace (the bound is on length, not form)",
            one_tag(" ", "v"),
            true,
        ),
        (
            "a key at the modelled maximum length",
            one_tag(&longest_key, "v"),
            true,
        ),
        (
            "a key one character past the maximum length",
            one_tag(&past_longest_key, "v"),
            false,
        ),
        (
            "a multi-byte key at the maximum CHARACTER length",
            one_tag(&longest_key_multibyte, "v"),
            true,
        ),
        (
            "a multi-byte key one character past the maximum",
            one_tag(&past_longest_key_multibyte, "v"),
            false,
        ),
        (
            "an empty value, which the modelled minimum of 0 allows",
            one_tag("retired", ""),
            true,
        ),
        (
            "a value at the modelled maximum length",
            one_tag("long", &longest_value),
            true,
        ),
        (
            "a value one character past the maximum length",
            one_tag("long", &past_longest_value),
            false,
        ),
        (
            "a multi-byte value at the maximum CHARACTER length",
            one_tag("long", &longest_value_multibyte),
            true,
        ),
        (
            "several entries, all inside the bounds",
            json!({ "env": "prod", "team": "cardiology", "cost": "42" }),
            true,
        ),
        (
            "several entries, one of them with an empty key",
            json!({ "env": "prod", "": "v" }),
            false,
        ),
        ("a map with no entries at all", json!({}), true),
    ];

    for (case, tags, accepted) in &rows {
        // A fresh emulator per row: one row's domains and tag sets can then never be
        // what a later row reads, and no row is near the account-wide domain cap.
        let emulator = Emulator::new("us-east-1");

        // --- CreateDomain carries the map on the create itself.
        let before = listed_domain_ids(&emulator).await;
        let (status, created) = emulator
            .send(
                "POST",
                "/domain",
                Some(&json!({ "name": "clinic", "tags": tags })),
            )
            .await;
        let create_refusal = if *accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: CreateDomain must accept {tags}, got {status} with {created}"
            );
            let domain_id = string_at(&created, "domainId");
            let arn = string_at(&created, "arn");
            let expected: BTreeMap<String, String> = entries(tags).into_iter().collect();
            // Both readers of a domain's tags report exactly what the create stored.
            let reported = domain_tags(&emulator, &domain_id).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: GetDomain must report the stored set, got {reported:?}"
            );
            let listed = list_tags(&emulator, case, &arn).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: ListTagsForResource must report the same set, got {listed:?}"
            );
            None
        } else {
            let message = assert_refused(&format!("{case} (CreateDomain)"), status, &created);
            // Nothing the refused create carried reached the store: no domain was
            // minted, so no reader can report the entry it carried.
            let after = listed_domain_ids(&emulator).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused CreateDomain must persist no domain, got {after:?}"
            );
            Some(message)
        };

        // --- TagResource carries the same map against a domain that already exists.
        let created = emulator
            .create_domain(&json!({ "name": "clinic-tagged" }))
            .await;
        let domain_id = string_at(&created, "domainId");
        let arn = string_at(&created, "arn");
        let (status, body) = emulator
            .send("POST", &tags_path(&arn), Some(&json!({ "tags": tags })))
            .await;
        if *accepted {
            assert_eq!(
                status, 204,
                "case {case:?}: TagResource must accept {tags}, got {status} with {body}"
            );
            let expected: BTreeMap<String, String> = entries(tags).into_iter().collect();
            let reported = domain_tags(&emulator, &domain_id).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: the merged set must be exactly the accepted entries, \
                 got {reported:?}"
            );
        } else {
            let message = assert_refused(&format!("{case} (TagResource)"), status, &body);
            let reported = domain_tags(&emulator, &domain_id).await;
            assert!(
                reported.is_empty(),
                "case {case:?}: a refused TagResource must write nothing, got {reported:?}"
            );
            let listed = list_tags(&emulator, case, &arn).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: no reader may report a refused entry, got {listed:?}"
            );
            // The bound belongs to the shape, so the two operations cannot word the same
            // violation differently.
            assert_eq!(
                create_refusal.as_deref(),
                Some(message.as_str()),
                "case {case:?}: both operations must refuse the entry identically"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! What ONE refusal of the tag entries a request names states, when the request breaks
//! the rules of SEVERAL of them.
//!
//! The RULE under test, asserted over tables rather than one recorded request: a request
//! refused for the tag entries it carries is answered with ONE refusal that names EVERY
//! entry it refuses, chosen and ordered by the request's own content rather than by the
//! iteration order of the collection that carried the entries — so the refusal states as
//! many clauses as there are refused entries, its count agrees with that number
//! (`1 validation error detected: <clause>`, `N validation errors detected: <clause>;
//! <clause>`), and two byte-identical requests, like the two operations that accept a
//! `TagMap`, are answered with the identical body every time.
//!
//! A `TagMap` reaches a handler as an unordered map, so its entries fix no order of their
//! own but their KEYS do, and that is the order the clauses are stated in; a `TagKeyList`
//! arrives ordered by the request, so its clauses keep the order the caller spelled the
//! keys in. The modelled-constraint clauses and the service's own reserved-namespace
//! sentence are two classes, so one refusal states one of them: a request that breaks a
//! modelled length gets the constraint envelope, and a request whose only fault is the
//! namespace gets the sentence, naming every key it spells in that namespace.
//!
//! The rows differ along what the rule generalises over: how many entries are refused
//! (one, two, three), which bound each one falls outside (a key below its minimum, a key
//! past its maximum, a value past its maximum), which class each fault belongs to (a
//! modelled length, the reserved namespace, and both at once), the FORM of the offending
//! value (ASCII and multi-byte, elided and not), how many acceptable entries stand beside
//! the refused ones, the ORDER the entries are handed over in (each row is also driven
//! reversed and rotated), and which collection carried them (a `TagMap`'s entries and a
//! `TagKeyList`'s elements). One row carries more refused entries than a refusal names, so
//! the ceiling that keeps one request from sizing its own response is covered too.
//!
//! The wire rows carry faults of the KEYS: a map VALUE past its modelled length is
//! refused by the request boundary itself (its walk carries the value shape's own
//! constraints and reports them at the entry's key), so such a request never reaches a
//! handler, while a map's KEY is captured as a path segment rather than checked there and
//! the namespace rule is stated by no modelled constraint at all — the faults the shared
//! check is the only refuser of
//! (`vendor/smithy-schema-server/src/constraints.rs`, `ValidatingSerializer::write_leaf`).
//! Every class, values included, is driven through the shared check directly.
//!
//! Sources for every bound and sentence written by hand here:
//!
//! * `smithy_query(query="shape",shape="TagMap,TagKeyList,TagKey,TagValue",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `TagMap`'s key and `TagKeyList`'s element target `TagKey` (`@length` 1..128) and
//!   `TagMap`'s value targets `TagValue` (`@length` 0..256); the crate states those
//!   numbers as `state::MIN_TAG_KEY_LEN` / `MAX_TAG_KEY_LEN` / `MAX_TAG_VALUE_LEN` and the
//!   reserved namespace as `state::RESERVED_TAG_KEY_PREFIX`, which is what this suite
//!   imports rather than restating.
//! * `smithy_query(query="digest")` — the operations that accept a `TagMap` are
//!   `CreateDomain` (`POST /domain`, 201) and `TagResource` (`POST /tags/{resourceArn}`,
//!   204); `UntagResource` (`DELETE /tags/{resourceArn}`, 204) carries the `TagKeyList`.
//! * The constraint clause and its envelope are AWS's standard form for a refused input
//!   (`N validation error(s) detected: Value '<value>' at '<member>' failed to satisfy
//!   constraint: <requirement>`), and a value longer than what AWS echoes appears as a
//!   byte prefix with its full byte count stated; both are written out below rather than
//!   borrowed from the emulator's renderer, so an expectation here is independent of the
//!   code under test. A refused map VALUE is reported at its entry's key followed by the
//!   collection step, the spelling the framework gives a map entry (`constraints::path`).

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_connecthealth_connecthealth::service::tag_bounds::{
    removed_key_violation, violation as tag_entry_violation,
};
use service_com_amazonaws_connecthealth_connecthealth::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN, MIN_TAG_KEY_LEN,
    RESERVED_TAG_KEY_PREFIX,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::resource_tagging::{domain_tags, tags_path};

/// How many times one byte-identical request is repeated to show that its answer does not
/// depend on how the map that carried its entries happened to be laid out. Each request is
/// deserialized into a map of its own, so repetition is what exposes an answer chosen by
/// iteration order.
const REPEATS: usize = 12;

/// The ceiling an echoed value is cut at inside a constraint clause: a value longer than
/// this many BYTES appears as a prefix of that many bytes, cut on a character boundary,
/// followed by its full byte count.
///
/// Source: the elision AWS applies to the value it echoes back, the same ceiling the
/// framework renders its own refusals with
/// (`vendor/smithy-schema-server/src/constraints.rs`).
const MAX_ECHOED_VALUE_BYTES: usize = 256;

/// `value` as a refusal echoes it.
pub(crate) fn echoed(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE_BYTES {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// AWS's clause for one refused value: what was sent, where it was sent, and what it had
/// to satisfy.
///
/// `pub(crate)` so the sibling suite that asserts WHICH refusal a map of several bad
/// entries earns (`tests/all/tag_bound_precedence.rs`) words its expectation from the
/// same hand-written form rather than from the emulator's renderer.
pub(crate) fn clause(value: &str, at: &str, requirement: &str) -> String {
    format!(
        "Value '{}' at '{at}' failed to satisfy constraint: {requirement}",
        echoed(value)
    )
}

/// The requirement a value below a `@length` minimum failed.
pub(crate) fn at_least(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement a value past a `@length` maximum failed.
pub(crate) fn at_most(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// AWS's envelope over the clauses one refusal states — its count is the number of
/// clauses, which is what a refusal that stops at one offender cannot state.
pub(crate) fn envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// The service's own sentence for the keys a request spells in the reserved namespace,
/// reported at the member that carried them.
fn reserved_sentence(member: &str, keys: &[&str]) -> String {
    let (noun, verb) = if keys.len() == 1 {
        ("key", "uses")
    } else {
        ("keys", "use")
    };
    let named: Vec<String> = keys.iter().map(|key| format!("'{key}'")).collect();
    format!(
        "Tag keys cannot start with '{RESERVED_TAG_KEY_PREFIX}': the tag {noun} {} at \
         '{member}' {verb} the prefix reserved for tags this service sets",
        named.join(", ")
    )
}

/// One row: a label, the entries one request carries IN THE ORDER the row spells them,
/// and the refusal the rule owes that request (`None` when every entry is acceptable).
type Row = (String, Vec<(String, String)>, Option<String>);

/// One entry, as a row spells it.
fn entry(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One character past `MAX_TAG_KEY_LEN`, the shortest key outside the modelled bound.
fn past_longest_key() -> String {
    "k".repeat(MAX_TAG_KEY_LEN + 1)
}

/// The reserved namespace as a caller might spell it, in each ASCII case the rule
/// disregards.
fn reserved_keys() -> (String, String) {
    (
        format!("{RESERVED_TAG_KEY_PREFIX}owner"),
        format!("{}billing", RESERVED_TAG_KEY_PREFIX.to_uppercase()),
    )
}

/// The table every class of fault a `TagMap` entry can carry, driven through the shared
/// check.
fn rows() -> Vec<Row> {
    // One character past each published bound, in an ASCII form and in a multi-byte one
    // (`@length` counts characters while a refusal echoes BYTES, so the two are different
    // rows).
    let past_longest_key = past_longest_key();
    let past_longest_value = "v".repeat(MAX_TAG_VALUE_LEN + 1);
    let past_longest_value_multibyte = "é".repeat(MAX_TAG_VALUE_LEN + 1);
    let (reserved_lower, _) = reserved_keys();

    let mut rows: Vec<Row> = vec![
        ("no entries at all".to_string(), vec![], None),
        (
            "every entry inside both bounds".to_string(),
            vec![entry("env", "prod"), entry("retired", "")],
            None,
        ),
        (
            // Two faults of two different bounds in one request: the clauses are stated
            // in the order of the keys that carried them ("" before "k").
            "an empty key and an over-long value".to_string(),
            vec![entry("", "v"), entry("k", &past_longest_value)],
            Some(envelope(&[
                clause("", "tags", &at_least(MIN_TAG_KEY_LEN)),
                clause(
                    &past_longest_value,
                    "tags.k.member",
                    &at_most(MAX_TAG_VALUE_LEN),
                ),
            ])),
        ),
        (
            // An over-long KEY is reported at the member the map is bound to; a key of
            // "k"s sorts before "zebra".
            "an over-long key and an over-long value".to_string(),
            vec![
                entry("zebra", &past_longest_value),
                entry(&past_longest_key, "v"),
            ],
            Some(envelope(&[
                clause(&past_longest_key, "tags", &at_most(MAX_TAG_KEY_LEN)),
                clause(
                    &past_longest_value,
                    "tags.zebra.member",
                    &at_most(MAX_TAG_VALUE_LEN),
                ),
            ])),
        ),
        (
            "three refused entries beside two acceptable ones".to_string(),
            vec![
                entry("env", "prod"),
                entry("", "v"),
                entry("alpha", &past_longest_value),
                entry(&past_longest_key, "v"),
                entry("retired", ""),
            ],
            Some(envelope(&[
                clause("", "tags", &at_least(MIN_TAG_KEY_LEN)),
                clause(
                    &past_longest_value,
                    "tags.alpha.member",
                    &at_most(MAX_TAG_VALUE_LEN),
                ),
                clause(&past_longest_key, "tags", &at_most(MAX_TAG_KEY_LEN)),
            ])),
        ),
        (
            "a multi-byte over-long value, echoed as a byte prefix".to_string(),
            vec![
                entry("héritage", &past_longest_value_multibyte),
                entry("env", "prod"),
            ],
            Some(envelope(&[clause(
                &past_longest_value_multibyte,
                "tags.héritage.member",
                &at_most(MAX_TAG_VALUE_LEN),
            )])),
        ),
        (
            // Two classes at once: the modelled constraint is what the envelope states,
            // and the service's own sentence is not mixed into it.
            "a reserved key beside an over-long value".to_string(),
            vec![
                entry(&reserved_lower, "mine"),
                entry("k", &past_longest_value),
            ],
            Some(envelope(&[clause(
                &past_longest_value,
                "tags.k.member",
                &at_most(MAX_TAG_VALUE_LEN),
            )])),
        ),
    ];
    // The rows whose faults are the KEYS' are the ones a handler is the only refuser of,
    // so they are driven through the shared check AND over the wire.
    rows.extend(key_fault_rows());
    rows
}

/// The rows whose every fault is a KEY's — below the modelled minimum, past the modelled
/// maximum, or in the reserved namespace. These reach a handler over the wire, so they are
/// the rows the end-to-end test drives as well.
fn key_fault_rows() -> Vec<Row> {
    let past_longest_key = past_longest_key();
    let (reserved_lower, reserved_upper) = reserved_keys();
    vec![
        (
            "one entry with an empty key".to_string(),
            vec![entry("", "v"), entry("env", "prod")],
            Some(envelope(&[clause("", "tags", &at_least(MIN_TAG_KEY_LEN))])),
        ),
        (
            // Two keys outside two different ends of one bound: both clauses are stated,
            // in key order ("" before a key of "k"s).
            "an empty key and an over-long key".to_string(),
            vec![
                entry(&past_longest_key, "v"),
                entry("", "v"),
                entry("env", "prod"),
            ],
            Some(envelope(&[
                clause("", "tags", &at_least(MIN_TAG_KEY_LEN)),
                clause(&past_longest_key, "tags", &at_most(MAX_TAG_KEY_LEN)),
            ])),
        ),
        (
            "one key in the reserved namespace".to_string(),
            vec![entry(&reserved_lower, "mine"), entry("env", "prod")],
            Some(reserved_sentence("tags", &[&reserved_lower])),
        ),
        (
            // Both keys are in the namespace however they are spelled, and one sentence
            // names both — in key order, which puts the upper-case spelling first.
            "two keys in the reserved namespace".to_string(),
            vec![
                entry(&reserved_lower, "mine"),
                entry("env", "prod"),
                entry(&reserved_upper, "mine"),
            ],
            Some(reserved_sentence(
                "tags",
                &[&reserved_upper, &reserved_lower],
            )),
        ),
        (
            "a reserved key beside an over-long key".to_string(),
            vec![
                entry(&reserved_lower, "mine"),
                entry(&past_longest_key, "v"),
            ],
            Some(envelope(&[clause(
                &past_longest_key,
                "tags",
                &at_most(MAX_TAG_KEY_LEN),
            )])),
        ),
    ]
}

/// `entries` handed to the shared check, in the order given.
fn checked(entries: &[(String, String)]) -> Option<String> {
    let borrowed: Vec<(&String, &String)> =
        entries.iter().map(|(key, value)| (key, value)).collect();
    tag_entry_violation(borrowed)
}

/// `entries` as the `tags` member of a request body.
fn tags_object(entries: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.clone(), json!(value));
    }
    Value::Object(map)
}

/// The detail text of a refusal, in the member this service reports it in.
fn detail(case: &str, body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal must carry a Message, got {body}"))
        .to_string()
}

#[test]
fn the_shared_tag_entry_check_names_every_refused_entry_whatever_order_it_meets_them_in() {
    for (case, entries, owed) in rows() {
        // The same entries handed over three ways: as the row spells them, reversed, and
        // rotated by one. The rule is on the entries, not on the sequence they arrive in,
        // so all three must be answered identically.
        let reversed: Vec<(String, String)> = entries.iter().rev().cloned().collect();
        let mut rotated = entries.clone();
        if !rotated.is_empty() {
            rotated.rotate_left(1);
        }
        for (order, handed) in [
            ("as spelled", &entries),
            ("reversed", &reversed),
            ("rotated", &rotated),
        ] {
            let reported = checked(handed);
            assert_eq!(
                reported.as_deref(),
                owed.as_deref(),
                "case {case:?} handed over {order}: expected {owed:?}, got {reported:?}"
            );
        }
    }
}

/// A request may name far more entries than a resource may hold, and a refusal names at
/// most that many of them — the most a caller could act on — so one request cannot size
/// the response it is answered with. Which ones are named is the first by key, so the
/// ceiling does not reintroduce an arbitrary choice.
#[test]
fn a_refusal_names_no_more_entries_than_a_resource_may_hold() {
    let past_longest_value = "v".repeat(MAX_TAG_VALUE_LEN + 1);
    // Twice as many refused entries as a refusal may name, keyed so that sorting by key
    // is the same as sorting by index.
    let entries: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE * 2)
        .map(|index| entry(&format!("key-{index:04}"), &past_longest_value))
        .collect();
    let reversed: Vec<(String, String)> = entries.iter().rev().cloned().collect();

    let named: Vec<String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| {
            clause(
                &past_longest_value,
                &format!("tags.key-{index:04}.member"),
                &at_most(MAX_TAG_VALUE_LEN),
            )
        })
        .collect();
    let owed = envelope(&named);
    for (order, handed) in [("as spelled", &entries), ("reversed", &reversed)] {
        let reported = checked(handed);
        assert_eq!(
            reported.as_deref(),
            Some(owed.as_str()),
            "case {order:?}: a refusal must name the first {MAX_TAGS_PER_RESOURCE} entries \
             by key, got {reported:?}"
        );
    }
}

#[test]
fn the_shared_removed_key_check_names_every_refused_key_in_the_order_the_list_spells_them() {
    let past_longest_key = past_longest_key();
    let (reserved_lower, reserved_upper) = reserved_keys();

    // A `TagKeyList` is ordered by the request, so each clause names the element's own
    // 1-based position and the clauses keep that order.
    let rows: Vec<(&str, Vec<String>, Option<String>)> = vec![
        (
            "every key acceptable",
            vec!["env".to_string(), "team".to_string()],
            None,
        ),
        (
            "one key past the maximum length",
            vec!["env".to_string(), past_longest_key.clone()],
            Some(envelope(&[clause(
                &past_longest_key,
                "tagKeys.2.member",
                &at_most(MAX_TAG_KEY_LEN),
            )])),
        ),
        (
            "an over-long key and an empty one",
            vec![
                "env".to_string(),
                past_longest_key.clone(),
                String::new(),
                "team".to_string(),
            ],
            Some(envelope(&[
                clause(
                    &past_longest_key,
                    "tagKeys.2.member",
                    &at_most(MAX_TAG_KEY_LEN),
                ),
                clause("", "tagKeys.3.member", &at_least(MIN_TAG_KEY_LEN)),
            ])),
        ),
        (
            "two keys in the reserved namespace",
            vec![
                reserved_lower.clone(),
                "env".to_string(),
                reserved_upper.clone(),
            ],
            Some(reserved_sentence(
                "tagKeys",
                &[&reserved_lower, &reserved_upper],
            )),
        ),
        (
            "a reserved key beside an over-long one",
            vec![reserved_lower.clone(), past_longest_key.clone()],
            Some(envelope(&[clause(
                &past_longest_key,
                "tagKeys.2.member",
                &at_most(MAX_TAG_KEY_LEN),
            )])),
        ),
    ];

    for (case, keys, owed) in rows {
        let reported = removed_key_violation(keys.iter());
        assert_eq!(
            reported.as_deref(),
            owed.as_deref(),
            "case {case:?}: expected {owed:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn both_operations_answer_one_request_with_the_identical_refusal_every_time() {
    // Each request is deserialized into a map of its own, so a refusal chosen by that
    // map's layout would differ between repetitions and between the two operations.
    for (case, entries, owed) in key_fault_rows() {
        let owed = owed.unwrap_or_else(|| panic!("case {case:?}: a refused row owes a refusal"));
        let tags = tags_object(&entries);
        let emulator = Emulator::new("us-east-1");
        let created = emulator
            .create_domain(&json!({ "name": "clinic-tagged" }))
            .await;
        let domain_id = string_at(&created, "domainId");
        let arn = string_at(&created, "arn");

        for repeat in 1..=REPEATS {
            let label = format!("{case} (TagResource, call {repeat})");
            let (status, body) = emulator
                .send("POST", &tags_path(&arn), Some(&json!({ "tags": tags })))
                .await;
            assert_eq!(
                status, 400,
                "case {label:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}: got {body}"
            );
            assert_eq!(
                detail(&label, &body),
                owed,
                "case {label:?}: expected {owed:?}"
            );

            // The same entries on a CREATE are refused with the same sentence: the rule
            // belongs to the shape both operations carry.
            let label = format!("{case} (CreateDomain, call {repeat})");
            let (status, body) = emulator
                .send(
                    "POST",
                    "/domain",
                    Some(&json!({ "name": "clinic", "tags": tags })),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {label:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}: got {body}"
            );
            assert_eq!(
                detail(&label, &body),
                owed,
                "case {label:?}: expected {owed:?}"
            );
        }

        // And every one of those refused requests wrote nothing at all.
        let reported = domain_tags(&emulator, &domain_id).await;
        assert_eq!(
            reported,
            BTreeMap::new(),
            "case {case:?}: a refused TagResource must write nothing, got {reported:?}"
        );
    }
}

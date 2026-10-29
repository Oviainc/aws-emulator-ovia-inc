// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for HOW this service names a LIST ELEMENT in a refusal.
//!
//! RULE: a `ValidationException` about one element of an input list names that element
//! with the dotted path the framework's own validating walk produces — the list member,
//! the element's 1-BASED position, and the literal `member` segment the element's schema
//! contributes (`vendor/smithy-schema-server/src/constraints.rs`, whose walk spells
//! `names.2.member` and `keySchema.1.member.keyType`) — with the faulting member of a
//! STRUCTURE element appended to it. So the position a refusal reports counts from one,
//! never from zero, and it is never bracketed; and because every input list of this
//! service answers to the same rule, two operations refusing their respective second
//! element both report position `2`.
//!
//! The rows differ along what the rule generalises over: which input list the element
//! belongs to (`SearchImageSets`'s `searchCriteria.filters`, a list of STRUCTURES, and
//! `UntagResource`'s `tagKeys`, a list of strings), the position the element holds, which
//! member of a structure element is at fault (`values`, `operator`), and which constraint
//! class it failed. Every case drives real HTTP through the production front door, so what
//! is asserted is the wire text a `medical-imaging` SDK client sees.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::TAG_KEY_RESERVED_PREFIX;

use crate::datastore_lifecycle::{Answer, Emulator};
use crate::image_set_search::search;
use crate::resource_tagging::{untag, VALIDATION_ENVELOPE};
use crate::untag_key_validation::held_datastore_arn;

/// Where the refused element sat, and which member of it was at fault.
#[derive(Debug, Clone, Copy)]
struct Element {
    /// The input list member the element belongs to, in the lowerCamel spelling a
    /// validation path uses.
    list_member: &'static str,
    /// The element's 0-based offset in that list — what the refusal must NOT report.
    position: usize,
    /// The member of a STRUCTURE element at fault, or `None` when the element itself is
    /// the leaf that failed.
    member: Option<&'static str>,
}

impl Element {
    /// The path a refusal about this element must name.
    fn expected_path(&self) -> String {
        let element = format!("{}.{}.member", self.list_member, self.position + 1);
        match self.member {
            Some(member) => format!("{element}.{member}"),
            None => element,
        }
    }

    /// The spellings a refusal about this element must NOT carry: the raw 0-based dotted
    /// position (which for any element past the first names the element BEFORE it), and
    /// the bracketed form in either numbering.
    fn forbidden_paths(&self) -> Vec<String> {
        vec![
            format!("{}.{}.member", self.list_member, self.position),
            format!("{}[{}]", self.list_member, self.position),
            format!("{}[{}]", self.list_member, self.position + 1),
        ]
    }
}

/// Assert `answer` is a refusal in this service's one validation envelope that names
/// `element` the way the rule spells it, and spells it no other way.
fn assert_names_element(case: &str, answer: &Answer, element: Element) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected error code ValidationException, got {answer:?}"
    );
    let message = answer.string(&["message"]);
    assert!(
        message.starts_with(VALIDATION_ENVELOPE),
        "case {case:?}: expected the message to open with {VALIDATION_ENVELOPE:?}, got {message:?}"
    );
    assert!(
        message.contains("failed to satisfy constraint"),
        "case {case:?}: expected a constraint clause, got {message:?}"
    );
    let expected = element.expected_path();
    assert!(
        message.contains(&expected),
        "case {case:?}: expected the refusal to name {expected:?}, got {message:?}"
    );
    for forbidden in element.forbidden_paths() {
        assert!(
            !message.contains(&forbidden),
            "case {case:?}: expected the refusal never to spell the position {forbidden:?}, got {message:?}"
        );
    }
}

/// The index segment a refusal reported for the element of `list_member` it refused, read
/// back out of the message — so two operations can be compared on the position they
/// report rather than on the whole path.
fn reported_position(answer: &Answer, list_member: &str) -> String {
    let message = answer.string(&["message"]);
    let marker = format!("{list_member}.");
    let tail = message
        .split_once(&marker)
        .unwrap_or_else(|| panic!("a refusal naming {list_member}, got {message:?}"))
        .1;
    tail.split('.')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A `searchCriteria` payload holding `filters` verbatim.
fn criteria(filters: Value) -> Value {
    json!({ "filters": filters })
}

/// A filter this service evaluates without complaint, used to OCCUPY a position so the
/// faulting filter sits at the next one.
fn sound_filter() -> Value {
    json!({ "operator": "EQUAL", "values": [{ "DICOMPatientId": "PID-1" }] })
}

/// One filter whose `values` member cannot be evaluated, by constraint class. Each class
/// is one the handler checks itself past the boundary validator
/// (`src/ops/search_image_sets.rs`), so each exercises the refusal path under test.
fn unevaluable_filters() -> Vec<(&'static str, Value)> {
    vec![
        (
            "EQUAL given two values",
            json!({
                "operator": "EQUAL",
                "values": [{ "DICOMPatientId": "PID-1" }, { "DICOMPatientId": "PID-2" }],
            }),
        ),
        (
            "BETWEEN given one bound",
            json!({ "operator": "BETWEEN", "values": [{ "createdAt": 1_767_225_600.0 }] }),
        ),
        (
            "BETWEEN over an attribute that is not a date or a time",
            json!({
                "operator": "BETWEEN",
                "values": [{ "DICOMPatientId": "PID-1" }, { "DICOMPatientId": "PID-2" }],
            }),
        ),
        (
            "BETWEEN bounds naming two different attributes",
            json!({
                "operator": "BETWEEN",
                "values": [{ "createdAt": 1_767_225_600.0 }, { "updatedAt": 1_767_225_900.0 }],
            }),
        ),
        (
            "BETWEEN over createdAt with the bounds the wrong way round",
            json!({
                "operator": "BETWEEN",
                "values": [{ "createdAt": 1_767_225_900.0 }, { "createdAt": 1_767_225_600.0 }],
            }),
        ),
        (
            "BETWEEN over study dates with the bounds the wrong way round",
            json!({
                "operator": "BETWEEN",
                "values": [
                    { "DICOMStudyDateAndTime": { "DICOMStudyDate": "20240101" } },
                    { "DICOMStudyDateAndTime": { "DICOMStudyDate": "19850412" } },
                ],
            }),
        ),
    ]
}

/// RULE: a refusal about the `values` member of a filter names that filter by its 1-based
/// position in `searchCriteria.filters` plus the `member.values` segments. The rows span
/// every constraint class the member can fail, at each position the list admits —
/// `SearchCriteria.filters` is `@length(min: 1, max: 2)`
/// (`smithy_query(query="shape",shape="SearchCriteria")`), so the positions are the first
/// and the second.
#[tokio::test]
async fn a_filter_values_refusal_names_the_filters_one_based_position() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("filter.path.values", "filter-path-values")
        .await;

    for (case, filter) in unevaluable_filters() {
        // Alone, so the faulting filter is the FIRST element.
        let first = criteria(json!([filter.clone()]));
        let answer = search(&emulator, &datastore_id, Some(first), "").await;
        assert_names_element(
            &format!("{case}, as the only filter"),
            &answer,
            Element {
                list_member: "searchCriteria.filters",
                position: 0,
                member: Some("values"),
            },
        );

        // Behind a filter this service evaluates, so it is the SECOND element — the
        // position a 0-based report would name the first one at.
        let second = criteria(json!([sound_filter(), filter]));
        let answer = search(&emulator, &datastore_id, Some(second), "").await;
        assert_names_element(
            &format!("{case}, behind a sound filter"),
            &answer,
            Element {
                list_member: "searchCriteria.filters",
                position: 1,
                member: Some("values"),
            },
        );
    }
}

/// RULE: the same spelling names the filter whose `operator` is outside the modelled enum
/// — a DIFFERENT member of the same structure element, so the path is the element's plus
/// that member's own segment. `Operator` declares exactly `EQUAL` and `BETWEEN`
/// (`smithy_query(query="shape",shape="Operator")`); the rows are values outside that
/// set, including one that differs from a modelled value only in case, at each position.
#[tokio::test]
async fn a_filter_operator_refusal_names_the_filters_one_based_position() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("filter.path.operator", "filter-path-operator")
        .await;

    let operators = ["LESS_THAN", "equal", "", "BETWEEN "];
    for operator in operators {
        let filter = json!({ "operator": operator, "values": [{ "DICOMPatientId": "PID-1" }] });

        let alone = criteria(json!([filter.clone()]));
        let answer = search(&emulator, &datastore_id, Some(alone), "").await;
        assert_names_element(
            &format!("operator {operator:?}, as the only filter"),
            &answer,
            Element {
                list_member: "searchCriteria.filters",
                position: 0,
                member: Some("operator"),
            },
        );

        let answer = search(
            &emulator,
            &datastore_id,
            Some(criteria(json!([sound_filter(), filter]))),
            "",
        )
        .await;
        assert_names_element(
            &format!("operator {operator:?}, behind a sound filter"),
            &answer,
            Element {
                list_member: "searchCriteria.filters",
                position: 1,
                member: Some("operator"),
            },
        );
    }
}

/// RULE: the same rule governs a list of STRINGS, where the element itself is the leaf
/// that failed and the path therefore ends at the `member` segment — `UntagResource`'s
/// `tagKeys`. Rows differ along the position the illegal key holds and the length of the
/// list around it.
#[tokio::test]
async fn an_untag_key_refusal_names_the_keys_one_based_position() {
    let emulator = Emulator::new();
    let arn = held_datastore_arn(&emulator, "key.path", "key-path-token").await;
    let illegal = format!("{TAG_KEY_RESERVED_PREFIX}owner");

    // (how many keys the list carries, the 0-based offset the illegal key holds)
    let rows: [(usize, usize); 5] = [(1, 0), (2, 1), (3, 1), (4, 3), (4, 2)];
    for (length, position) in rows {
        let case = format!("{length} keys, the illegal one at offset {position}");
        let mut keys: Vec<String> = (0..length).map(|n| format!("key{n}")).collect();
        keys[position] = illegal.clone();
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();

        let refused = untag(&emulator, &arn, &borrowed).await;
        assert_names_element(
            &case,
            &refused,
            Element {
                list_member: "tagKeys",
                position,
                member: None,
            },
        );
    }
}

/// RULE (one service, one convention): the position is a property of the LIST ELEMENT, not
/// of the operation — so the two operations that have to name one report the SAME index
/// for an element at the same offset. Asserted by reading the index segment back out of
/// each refusal.
#[tokio::test]
async fn both_operations_report_the_same_index_for_the_same_offset() {
    let emulator = Emulator::new();
    let arn = held_datastore_arn(&emulator, "index.agreement", "index-agreement").await;
    let datastore_id = emulator
        .active_datastore("index.agreement.search", "index-agreement-search")
        .await;
    let illegal = format!("{TAG_KEY_RESERVED_PREFIX}owner");

    // offset 0 is reported as "1", offset 1 as "2": the first two offsets `filters`
    // admits, which is also where `tagKeys` can be compared against it.
    for (offset, expected) in [(0usize, "1"), (1usize, "2")] {
        let mut keys: Vec<String> = vec!["keep".to_string(), "keep-too".to_string()];
        keys[offset] = illegal.clone();
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
        let untagged = untag(&emulator, &arn, &borrowed).await;
        let from_untag = reported_position(&untagged, "tagKeys");

        let mut filters: Vec<Value> = vec![sound_filter(), sound_filter()];
        filters[offset] = json!({
            "operator": "EQUAL",
            "values": [{ "DICOMPatientId": "PID-1" }, { "DICOMPatientId": "PID-2" }],
        });
        let searched = search(&emulator, &datastore_id, Some(criteria(json!(filters))), "").await;
        let from_search = reported_position(&searched, "searchCriteria.filters");

        assert_eq!(
            from_untag, expected,
            "offset {offset}: expected UntagResource to report index {expected:?}, got {from_untag:?}"
        );
        assert_eq!(
            from_search, expected,
            "offset {offset}: expected SearchImageSets to report index {expected:?}, got {from_search:?}"
        );
        assert_eq!(
            from_untag, from_search,
            "offset {offset}: expected both operations to report one index, got {from_untag:?} and {from_search:?}"
        );
    }
}

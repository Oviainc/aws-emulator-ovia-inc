// SPDX-License-Identifier: CC0-1.0

//! The UNIT the length bounds on a `Filter` are measured in.
//!
//! Every wire case goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint validation, the handler and the response serialization all run exactly as
//! they do in the emulator binary; the in-crate cases drive the one validation every
//! filtered listing shares, because a value past the modelled bound never reaches a
//! handler over the wire.
//!
//! RULE under test: a modelled string `@length` bound counts CHARACTERS, so a
//! `Filter$Name`/`Filter$Value` within the bound in characters is served however many
//! UTF-8 bytes it encodes to, and a value past the bound is refused with the count
//! stated in characters.
//!
//! The rows span what the rule generalises over: every UTF-8 width above one byte
//! (two-byte, three-byte and four-byte characters) and a base character followed by a
//! combining mark, at a length inside the bound and at the bound exactly, for both
//! bounded members (`Filter$Name` at 32, `Filter$Value` at 64), over all four filtered
//! listings — with ASCII, where bytes and characters agree, as the control row.

use serde_json::Value;
use service_com_amazonaws_ssmsap_ssmsap::ops::types::Filter;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    filters_within_bounds, validate_filters, FilterOperators, FILTER_NAMES,
    FILTER_NAME_MAX_LENGTH, FILTER_OPERATOR_EQUALS, FILTER_VALUE_MAX_LENGTH,
};

use crate::application_lifecycle::Emulator;
use crate::filter_validation::{listings, APPLICATION};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The scripts a `Filter` value may be written in. `FilterName`/`FilterValue` constrain
/// only length (`smithy_query(query="shape", shape="FilterName,FilterValue")`: `@length`
/// and no `@pattern`), so every one of these is a value the model admits.
///
/// Each row is (label, a sample whose characters are cycled to build a string of a given
/// length). Everything but the first encodes to more BYTES than it holds characters.
const SCRIPTS: &[(&str, &str)] = &[
    ("ASCII, where bytes and characters agree", "filter-value"),
    ("two-byte characters", "ключ-фильтра"),
    ("three-byte characters", "検索値"),
    ("four-byte characters", "🛠🚀🧪"),
    ("a base character and a combining mark", "e\u{301}"),
];

/// A string of exactly `count` characters, built by cycling `sample`'s characters — so
/// the row's character count is fixed and its byte length follows from the script.
fn chars_of(sample: &str, count: usize) -> String {
    sample.chars().cycle().take(count).collect()
}

/// One filter value under test: its label and the string itself.
fn value_rows(count: usize) -> Vec<(String, String)> {
    SCRIPTS
        .iter()
        .map(|(label, sample)| {
            let text = chars_of(sample, count);
            (format!("{label} x{count}"), text)
        })
        .collect()
}

/// RULE: a `Filter$Value` within the modelled bound in CHARACTERS is served by every
/// filtered listing, whatever it encodes to in bytes — the boundary validator accepts it
/// from the model, and the bound this crate re-states must agree with it.
#[tokio::test]
async fn every_listing_serves_a_multi_byte_filter_value_inside_the_bound() {
    let emulator = Emulator::new();
    let listings = listings(&emulator).await;

    // Inside the bound, and at the bound exactly.
    for count in [40, FILTER_VALUE_MAX_LENGTH] {
        for (label, text) in value_rows(count) {
            assert_eq!(
                text.chars().count(),
                count,
                "case {label:?}: the row must hold {count} characters, got {:?}",
                text.chars().count()
            );
            for listing in &listings {
                let name = listing.names.first().expect("a listing filters by a name");
                let case = format!("{} {label}", listing.label);
                let filter = serde_json::json!({
                    "Name": name,
                    "Value": text,
                    "Operator": FILTER_OPERATOR_EQUALS,
                });
                let reply = emulator.send(listing.path, listing.body(vec![filter])).await;
                assert_eq!(
                    reply.status,
                    200,
                    "case {case:?}: a value of {count} characters ({} bytes) is inside the \
                     modelled bound of {FILTER_VALUE_MAX_LENGTH}, expected 200, got {}: {:?}",
                    text.len(),
                    reply.status,
                    reply.body
                );
            }
        }
    }
}

/// RULE: the value is APPLIED, not waved through — a multi-byte value matches nothing
/// when no record carries it, and the same value joined with a record's own attribute
/// still selects that record (`Filter$Value`'s OR semantics).
#[tokio::test]
async fn a_multi_byte_value_is_compared_rather_than_ignored() {
    let emulator = Emulator::new();
    let listings = listings(&emulator).await;
    let applications = listings
        .iter()
        .find(|listing| listing.label == "ListApplications")
        .expect("the harness lists applications");

    for (label, text) in value_rows(40) {
        // Alone: nothing carries this id.
        let reply = emulator
            .send(
                applications.path,
                applications.body(vec![serde_json::json!({
                    "Name": "Id",
                    "Value": text,
                    "Operator": FILTER_OPERATOR_EQUALS,
                })]),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200, got {}: {:?}",
            reply.status, reply.body
        );
        let listed = reply
            .member("Applications")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            listed.is_empty(),
            "case {label:?}: no application carries that id, got {listed:?}"
        );

        // Joined with the registered id: the record is selected, so the multi-byte
        // value took part in the comparison instead of being dropped.
        let joined = format!("{text},{APPLICATION}");
        let reply = emulator
            .send(
                applications.path,
                applications.body(vec![serde_json::json!({
                    "Name": "Id",
                    "Value": joined,
                    "Operator": FILTER_OPERATOR_EQUALS,
                })]),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200, got {}: {:?}",
            reply.status, reply.body
        );
        let ids: Vec<&str> = reply
            .member("Applications")
            .and_then(Value::as_array)
            .map(|summaries| {
                summaries
                    .iter()
                    .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids,
            vec![APPLICATION],
            "case {label:?}: the OR of the multi-byte value and the registered id selects \
             it, got {ids:?}"
        );
    }
}

/// RULE: a `Filter$Name` within the bound in characters is past the LENGTH check, so the
/// refusal it draws is the one about the catalogue of names the listing filters by —
/// never a length refusal.
#[tokio::test]
async fn a_multi_byte_filter_name_inside_the_bound_is_refused_for_its_name_only() {
    let emulator = Emulator::new();
    let listings = listings(&emulator).await;

    for count in [16, FILTER_NAME_MAX_LENGTH] {
        for (label, text) in value_rows(count) {
            for listing in &listings {
                let case = format!("{} {label}", listing.label);
                let reply = emulator
                    .send(
                        listing.path,
                        listing.body(vec![serde_json::json!({
                            "Name": text,
                            "Value": "alpha",
                            "Operator": FILTER_OPERATOR_EQUALS,
                        })]),
                    )
                    .await;
                assert_eq!(
                    reply.status, 400,
                    "case {case:?}: expected 400, got {}: {:?}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.code(),
                    VALIDATION_EXCEPTION,
                    "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
                );
                let actual = message(&reply);
                let expected = listing.name_refusal(&text);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: a name of {count} characters ({} bytes) is inside the \
                     modelled bound of {FILTER_NAME_MAX_LENGTH}, expected {expected:?}, got \
                     {actual:?}",
                    text.len()
                );
            }
        }
    }
}

/// RULE: the bound this crate re-states is measured in characters and REPORTS the
/// character count — the same unit the model declares `@length` in, for both bounded
/// members.
#[test]
fn the_shared_bound_counts_and_reports_characters() {
    // (the member, its bound, how the refusal names it)
    let members: [(&str, usize, &str); 2] = [
        ("Filter$Name", FILTER_NAME_MAX_LENGTH, "Filter Name"),
        ("Filter$Value", FILTER_VALUE_MAX_LENGTH, "Filter Value"),
    ];

    for (member, bound, wording) in members {
        // At the bound: accepted, whatever the script.
        for (label, text) in value_rows(bound) {
            let case = format!("{member} {label}");
            let filter = if wording == "Filter Name" {
                filter_with(&text, "alpha")
            } else {
                filter_with("Id", &text)
            };
            assert!(
                filters_within_bounds(std::slice::from_ref(&filter)).is_ok(),
                "case {case:?}: {} characters ({} bytes) is inside the bound of {bound}, so it \
                 is accepted",
                text.chars().count(),
                text.len()
            );
        }

        // One past the bound, and well past it: refused, with the count in characters.
        for count in [bound + 1, bound + 37] {
            for (label, text) in value_rows(count) {
                let case = format!("{member} {label}");
                let filter = if wording == "Filter Name" {
                    filter_with(&text, "alpha")
                } else {
                    filter_with("Id", &text)
                };
                let refusal = filters_within_bounds(std::slice::from_ref(&filter))
                    .expect_err("a value past the bound is refused")
                    .message
                    .unwrap_or_default();
                let expected =
                    format!("{wording} is at most {bound} characters, but {count} were given");
                assert_eq!(
                    refusal, expected,
                    "case {case:?}: the count is in characters, not in the {} bytes it encodes \
                     to, expected {expected:?}, got {refusal:?}",
                    text.len()
                );
            }
        }
    }

    // The bound is checked before the catalogue of names, so an over-long name drawn
    // through the whole shared validation is refused for its length.
    let long_name = chars_of("ключ-фильтра", FILTER_NAME_MAX_LENGTH + 1);
    let refusal = validate_filters(
        std::slice::from_ref(&filter_with(&long_name, "alpha")),
        FilterOperators::Modelled,
        FILTER_NAMES,
    )
    .expect_err("a name past the bound is refused")
    .message
    .unwrap_or_default();
    let expected = format!(
        "Filter Name is at most {FILTER_NAME_MAX_LENGTH} characters, but {} were given",
        FILTER_NAME_MAX_LENGTH + 1
    );
    assert_eq!(
        refusal, expected,
        "expected {expected:?}, got {refusal:?}"
    );
}

/// RULE: the boundary validator the handler's bound re-states counts characters too — a
/// value one character past the modelled maximum is refused there, whatever its script,
/// and the requirement it reports is the modelled bound.
#[tokio::test]
async fn the_boundary_refuses_one_character_past_the_modelled_bound() {
    let emulator = Emulator::new();
    let listings = listings(&emulator).await;

    for (label, text) in value_rows(FILTER_VALUE_MAX_LENGTH + 1) {
        for listing in &listings {
            let case = format!("{} {label}", listing.label);
            let reply = emulator
                .send(
                    listing.path,
                    listing.body(vec![serde_json::json!({
                        "Name": listing.names.first().expect("a listing filters by a name"),
                        "Value": text,
                        "Operator": FILTER_OPERATOR_EQUALS,
                    })]),
                )
                .await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected 400, got {}: {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.code(),
                VALIDATION_EXCEPTION,
                "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
            );
            let actual = message(&reply);
            let requirement =
                format!("Member must have length less than or equal to {FILTER_VALUE_MAX_LENGTH}");
            assert!(
                actual.contains(&requirement),
                "case {case:?}: expected the message to state {requirement:?}, got {actual:?}"
            );
        }
    }
}

/// One filter carrying `name` and `value`, with an operator every listing offers.
fn filter_with(name: &str, value: &str) -> Filter {
    Filter {
        name: name.to_string(),
        value: value.to_string(),
        operator: FILTER_OPERATOR_EQUALS.to_string(),
    }
}

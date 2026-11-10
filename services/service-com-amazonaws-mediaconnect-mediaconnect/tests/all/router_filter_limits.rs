// SPDX-License-Identifier: CC0-1.0

//! The `Filters` size rule, asserted to be ONE rule for every router listing.
//!
//! ListRouterInputs (`POST /v1/routerInputs`), ListRouterOutputs (`POST /v1/routerOutputs`)
//! and ListRouterNetworkInterfaces (`POST /v1/routerNetworkInterfaces`) each narrow their
//! listing with a `Filters` list whose elements are unions carrying a list of accepted values
//! (`smithy_query(query="shape",shape="RouterInputFilter")` and its two siblings). All three
//! bound how many criteria a request may state and how many values any one of them may name,
//! and all three refuse a request past either bound with `BadRequestException` — the single
//! shared checker `state::check_router_filters` is what they apply, so the ceilings and the
//! refusal wording cannot differ between them.
//!
//! The rows below therefore vary what that rule generalises over: which operation is called,
//! which bound is exceeded, which `Filters` element exceeds it (the first of several, the last
//! of several, the only one), which member of each filter union the request uses, and the
//! absence of `Filters` altogether. The ceilings themselves come from the shared constants
//! rather than from literals, so a ceiling that moves moves every row with it. Every call
//! goes over the same front door a MediaConnect client reaches, on the harness
//! `flow_lifecycle` owns.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;

/// One router listing: the path it is reached at, the member of its filter union this suite
/// builds filters from (named as the model declares it, which is the key the sibling listing
/// suites of this crate send too), and the resource phrase its refusal names.
struct Listing {
    path: &'static str,
    /// A filter member that takes free-form strings, so a row's value count is what decides
    /// the outcome rather than an enum constraint.
    member: &'static str,
    /// A second member of the same union, so the rule is shown not to be keyed to one member.
    other_member: &'static str,
    listing: &'static str,
}

/// Every listing that applies the rule — one per former copy of it.
const LISTINGS: &[Listing] = &[
    Listing {
        path: "/v1/routerInputs",
        member: "RegionNames",
        other_member: "NameContains",
        listing: "router inputs",
    },
    Listing {
        path: "/v1/routerOutputs",
        member: "NameContains",
        other_member: "RegionNames",
        listing: "router outputs",
    },
    Listing {
        path: "/v1/routerNetworkInterfaces",
        member: "RegionNames",
        other_member: "NameContains",
        listing: "router network interfaces",
    },
];

/// `count` distinct free-form filter values.
fn values(count: usize) -> Value {
    (0..count)
        .map(|index| Value::String(format!("value-{index}")))
        .collect()
}

/// A `Filters` list of `value_counts.len()` entries, the entry at index *i* naming
/// `value_counts[i]` values. Every entry but the one at `other_at` uses `member`.
fn filters(listing: &Listing, value_counts: &[usize], other_at: Option<usize>) -> Value {
    value_counts
        .iter()
        .enumerate()
        .map(|(index, count)| {
            let member = if other_at == Some(index) {
                listing.other_member
            } else {
                listing.member
            };
            json!({ member: values(*count) })
        })
        .collect()
}

/// Send one listing request carrying `filters`.
async fn list(emulator: &Emulator, listing: &Listing, body: Value) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", listing.path, Some(json!({ "filters": body })))
        .await
}

/// The refusal a request naming too many `Filters` entries earns, as the service words it:
/// the detail inside this service's validation envelope
/// (`state::validation_failed_message`).
fn too_many_filters_message(listing: &str, stated: usize) -> String {
    mc::validation_failed_message(&format!(
        "A list {listing} request may apply at most {} Filters entries; this one has {stated}.",
        mc::MAX_ROUTER_FILTERS
    ))
}

/// The refusal a `Filters` entry naming too many values earns — the same wording whichever
/// listing was called, because the entry, not the listing, is what is too large.
fn too_many_values_message(named: usize) -> String {
    mc::validation_failed_message(&format!(
        "A Filters entry may name at most {} values; this one names {named}.",
        mc::MAX_ROUTER_FILTER_VALUES
    ))
}

/// What one row expects of a listing.
enum Expected {
    /// The request is answered.
    Listed,
    /// The request is refused, with this message.
    Refused(fn(&Listing) -> String),
}

/// One row: a description, the `Filters` to send, and what the listing must answer.
struct Row {
    case: &'static str,
    /// How many values each `Filters` entry names; its length is how many entries there are.
    value_counts: Vec<usize>,
    /// The entry that uses the union's OTHER member, if any.
    other_at: Option<usize>,
    expected: Expected,
}

/// The rows, built from the shared ceilings so they follow a ceiling that moves.
fn rows() -> Vec<Row> {
    let max_filters = mc::MAX_ROUTER_FILTERS;
    let max_values = mc::MAX_ROUTER_FILTER_VALUES;
    vec![
        Row {
            case: "no Filters entries at all",
            value_counts: Vec::new(),
            other_at: None,
            expected: Expected::Listed,
        },
        Row {
            case: "as many Filters entries as the ceiling allows",
            value_counts: vec![1; max_filters],
            other_at: Some(0),
            expected: Expected::Listed,
        },
        Row {
            case: "one Filters entry past the ceiling",
            value_counts: vec![1; max_filters + 1],
            other_at: None,
            expected: Expected::Refused(|listing| {
                too_many_filters_message(listing.listing, mc::MAX_ROUTER_FILTERS + 1)
            }),
        },
        Row {
            case: "far past the Filters ceiling",
            value_counts: vec![1; max_filters * 3],
            other_at: Some(max_filters),
            expected: Expected::Refused(|listing| {
                too_many_filters_message(listing.listing, mc::MAX_ROUTER_FILTERS * 3)
            }),
        },
        Row {
            case: "one entry naming as many values as the ceiling allows",
            value_counts: vec![max_values],
            other_at: None,
            expected: Expected::Listed,
        },
        Row {
            case: "every entry naming as many values as the ceiling allows",
            value_counts: vec![max_values; max_filters],
            other_at: Some(max_filters - 1),
            expected: Expected::Listed,
        },
        Row {
            case: "the only entry naming one value past the ceiling",
            value_counts: vec![max_values + 1],
            other_at: None,
            expected: Expected::Refused(|_| {
                too_many_values_message(mc::MAX_ROUTER_FILTER_VALUES + 1)
            }),
        },
        Row {
            case: "the FIRST of several entries naming one value past the ceiling",
            value_counts: vec![max_values + 1, 1, 1],
            other_at: None,
            expected: Expected::Refused(|_| {
                too_many_values_message(mc::MAX_ROUTER_FILTER_VALUES + 1)
            }),
        },
        Row {
            case: "the LAST of several entries naming values past the ceiling",
            value_counts: vec![1, 1, max_values + 7],
            other_at: None,
            expected: Expected::Refused(|_| too_many_values_message(mc::MAX_ROUTER_FILTER_VALUES + 7)),
        },
        Row {
            case: "an entry on the union's other member naming one value past the ceiling",
            value_counts: vec![1, max_values + 1],
            other_at: Some(1),
            expected: Expected::Refused(|_| {
                too_many_values_message(mc::MAX_ROUTER_FILTER_VALUES + 1)
            }),
        },
        Row {
            case: "past BOTH ceilings at once: the entry count is reported first",
            value_counts: vec![max_values + 1; max_filters + 1],
            other_at: None,
            expected: Expected::Refused(|listing| {
                too_many_filters_message(listing.listing, mc::MAX_ROUTER_FILTERS + 1)
            }),
        },
    ]
}

#[tokio::test]
async fn every_router_listing_applies_one_filters_size_rule() {
    let emulator = Emulator::new();

    // The ceilings themselves, pinned once: the rows below are built from the shared
    // constants, so this is what keeps those constants at the numbers the three
    // per-listing suites exercise (200 at ten filters and at a hundred values, 400 one
    // past each; those suites assert the status, the wording is asserted here).
    assert_eq!(
        (mc::MAX_ROUTER_FILTERS, mc::MAX_ROUTER_FILTER_VALUES),
        (10, 100),
        "the shared Filters ceilings, got {:?}",
        (mc::MAX_ROUTER_FILTERS, mc::MAX_ROUTER_FILTER_VALUES)
    );
    for listing in LISTINGS {
        for Row {
            case,
            value_counts,
            other_at,
            expected,
        } in rows()
        {
            let body = filters(listing, &value_counts, other_at);
            let (status, code, answer) = list(&emulator, listing, body).await;
            match expected {
                Expected::Listed => {
                    assert_eq!(
                        (status, code.as_deref()),
                        (200, None),
                        "case {case:?} on {:?}: expected 200, got {status} {code:?}: {answer}",
                        listing.path
                    );
                }
                Expected::Refused(message) => {
                    assert_eq!(
                        (status, code.as_deref()),
                        (400, Some("BadRequestException")),
                        "case {case:?} on {:?}: expected 400 BadRequestException, got {status} \
                         {code:?}: {answer}",
                        listing.path
                    );
                    let expected_message = message(listing);
                    assert_eq!(
                        answer["message"].as_str(),
                        Some(expected_message.as_str()),
                        "case {case:?} on {:?}: expected the refusal {expected_message:?}, got \
                         {answer}",
                        listing.path
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn the_values_ceiling_is_refused_in_the_same_words_by_every_listing() {
    let emulator = Emulator::new();

    // The entry is what is too large, not the resource being listed, so this refusal must be
    // byte-identical across the three listings: that is what makes it one rule rather than
    // three that happen to agree today.
    let mut refusals: Vec<(&str, String)> = Vec::new();
    for listing in LISTINGS {
        let body = filters(listing, &[mc::MAX_ROUTER_FILTER_VALUES + 1], None);
        let (status, code, answer) = list(&emulator, listing, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {:?}: expected 400 BadRequestException, got {status} {code:?}: {answer}",
            listing.path
        );
        refusals.push((
            listing.path,
            answer["message"].as_str().unwrap_or_default().to_string(),
        ));
    }

    let (first_path, first_message) = refusals[0].clone();
    for (path, message) in &refusals[1..] {
        assert_eq!(
            message, &first_message,
            "case {path:?}: the values-ceiling refusal must read exactly as {first_path:?} words \
             it, {first_message:?}, got {message:?}"
        );
    }
    assert_eq!(
        first_message,
        too_many_values_message(mc::MAX_ROUTER_FILTER_VALUES + 1),
        "case {first_path:?}: the shared refusal names the ceiling and the count, got \
         {first_message:?}"
    );
}

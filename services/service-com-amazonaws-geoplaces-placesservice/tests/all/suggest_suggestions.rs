// SPDX-License-Identifier: CC0-1.0

//! Suggest: what a partially typed query draws out of the seeded place catalog —
//! the places its words name, the query TERMS (a category, a business chain) those
//! places are found by, and the refinements of the words it typed only in part —
//! and how each kind of suggestion resolves back through GetPlace and SearchText.
//!
//! Each test is a TABLE whose rows differ along what its rule generalises over:
//! how much of a word the query typed, how many words it typed, which part of a
//! place the words name (its own name, an alternate or localized name, a street and
//! number, a postal code, the category vocabulary), which area filter narrows the
//! answer, the page size asked for, and the number of refinements asked for —
//! rather than along one recorded request. Every bound and vocabulary written out
//! here names the Smithy member it comes from. The emulator is driven as real HTTP
//! through the same router the front door serves, reusing the harness in
//! [`crate::places_catalog`].

use serde_json::{json, Value};
use service_com_amazonaws_geoplaces_placesservice::state::{encode_query_id, QUERY_TEXT_MAX};

use crate::places_catalog::{
    assert_success, distance, items, position, text, Emulator, PRICING_BUCKET_HEADER,
};

/// The pricing tier every Places response is charged in by default.
const PRICING_BUCKET_CORE: &str = "Core";

/// Suggest's route (`POST /v2/suggest`, this crate's `OPERATIONS.md`).
const SUGGEST: &str = "/v2/suggest";

/// SearchText's route, which a `QueryId` from Suggest is handed to.
const SEARCH_TEXT: &str = "/v2/search-text";

/// GetPlace's route (`GET /v2/place/{PlaceId}`).
const PLACE: &str = "/v2/place";

/// `SuggestResultItemType` values, from the model's enum of the same name.
const ITEM_PLACE: &str = "Place";
/// See [`ITEM_PLACE`].
const ITEM_QUERY: &str = "Query";

/// `QueryType` values, from the model's enum of the same name.
const TYPE_CATEGORY: &str = "Category";
/// See [`TYPE_CATEGORY`].
const TYPE_BUSINESS_CHAIN: &str = "BusinessChain";

/// The published bounds of `SuggestRequest$MaxResults` (`range 1..100`); the upper
/// one is also the maximum length of `SuggestResultItemList` (`len 0..100`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 100;

/// The published bounds of `SuggestRequest$MaxQueryRefinements` (`range 1..10`);
/// the upper one is also the maximum length of `QueryRefinementList`
/// (`len 0..10`).
const MAX_REFINEMENTS_MIN: i64 = 1;
/// See [`MAX_REFINEMENTS_MIN`].
const MAX_REFINEMENTS_MAX: i64 = 10;

/// The wire error code of every refusal this operation models for a bad request
/// (`ValidationException`, HTTP 400).
const VALIDATION_EXCEPTION: &str = "ValidationException";

// ---------------------------------------------------------------------------
// Reading a response
// ---------------------------------------------------------------------------

/// A member of a JSON object as a value, or `Null` when absent.
fn member(value: &Value, name: &str) -> Value {
    value.get(name).cloned().unwrap_or(Value::Null)
}

/// The suggestions of one `SuggestResultItemType`, in the order reported.
fn of_kind(body: &Value, kind: &str) -> Vec<Value> {
    items(body)
        .into_iter()
        .filter(|item| text(item, "SuggestResultItemType") == kind)
        .collect()
}

/// The titles of the place suggestions, in the order reported.
fn place_titles(body: &Value) -> Vec<String> {
    of_kind(body, ITEM_PLACE)
        .iter()
        .map(|item| text(item, "Title"))
        .collect()
}

/// The titles of the place suggestions, sorted: the SET a query names, for a rule
/// about which places answer it rather than about their order.
fn sorted_place_titles(body: &Value) -> Vec<String> {
    let mut titles = place_titles(body);
    titles.sort();
    titles
}

/// `(QueryType, Title)` of each query suggestion, in the order reported.
fn query_terms(body: &Value) -> Vec<(String, String)> {
    of_kind(body, ITEM_QUERY)
        .iter()
        .map(|item| {
            (
                text(&member(item, "Query"), "QueryType"),
                text(item, "Title"),
            )
        })
        .collect()
}

/// `(OriginalTerm, RefinedTerm, StartIndex, EndIndex)` of each refinement, in the
/// order reported. An absent index reads as -1, which no assertion accepts.
fn refinements(body: &Value) -> Vec<(String, String, i64, i64)> {
    body.get("QueryRefinements")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|refinement| {
                    (
                        text(refinement, "OriginalTerm"),
                        text(refinement, "RefinedTerm"),
                        refinement
                            .get("StartIndex")
                            .and_then(Value::as_i64)
                            .unwrap_or(-1),
                        refinement
                            .get("EndIndex")
                            .and_then(Value::as_i64)
                            .unwrap_or(-1),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `[start, end)` CHAR span of a piece of text, as a `Highlight` and a
/// `QueryRefinement` index it.
fn char_span(text: &str, start: i64, end: i64) -> String {
    let start = start.max(0) as usize;
    let length = (end - start as i64).max(0) as usize;
    text.chars().skip(start).take(length).collect()
}

/// The category ids of one place suggestion.
fn category_ids(place: &Value) -> Vec<String> {
    place
        .get("Categories")
        .and_then(Value::as_array)
        .map(|categories| {
            categories
                .iter()
                .map(|category| text(category, "Id"))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Which places a query names
// ---------------------------------------------------------------------------

/// A row of the place-suggestion table: a query and the catalog titles it names.
struct PlaceRow {
    case: &'static str,
    query: &'static str,
    /// The titles the place suggestions must be, sorted. Empty means the query
    /// names nothing, which is a 200 with no suggestions — never an invented one.
    expected: &'static [&'static str],
}

/// A query's words name the records whose own reported text, or whose category and
/// chain vocabulary, those words match — a partially typed word matching the
/// leading part of a term — and a query that names nothing is answered with no
/// suggestions.
///
/// The rows differ along how much of a word was typed, how many words, and which
/// part of a record the words name: its own name, a localized or alternate name, a
/// street and number, a postal code, or what the place IS.
#[tokio::test]
async fn suggest_names_the_places_a_partially_typed_query_matches() {
    let emulator = Emulator::new();
    let rows = [
        PlaceRow {
            case: "whole-words",
            query: "Space Needle",
            expected: &["Space Needle"],
        },
        PlaceRow {
            case: "partial-last-word",
            query: "Space Nee",
            expected: &["Space Needle"],
        },
        PlaceRow {
            case: "partial-first-word",
            query: "Starb",
            expected: &[
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS RESERVE ROASTERY",
            ],
        },
        PlaceRow {
            case: "street-and-number",
            query: "1600 Pennsylvania",
            expected: &[
                "1600 Pennsylvania Ave NW, Washington, DC 20500, United States",
                "White House",
            ],
        },
        PlaceRow {
            case: "postal-code",
            query: "75007",
            expected: &["Tour Eiffel"],
        },
        PlaceRow {
            case: "localized-name",
            query: "Eiffel",
            expected: &["Tour Eiffel"],
        },
        PlaceRow {
            case: "category-vocabulary",
            query: "coffee shop",
            expected: &[
                "BLUE BOTTLE COFFEE",
                "COMPASS COFFEE",
                "Cafe de Flore",
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS RESERVE ROASTERY",
                "Seattle Coffee Works",
                "Victrola Coffee Roasters",
            ],
        },
        PlaceRow {
            case: "business-chain-vocabulary",
            query: "Blue Bottle",
            expected: &["BLUE BOTTLE COFFEE"],
        },
        PlaceRow {
            case: "no-match",
            query: "Zzzzqqq",
            expected: &[],
        },
        PlaceRow {
            case: "separators-only",
            query: ",,, ---",
            expected: &[],
        },
    ];

    for row in rows {
        let body = emulator
            .post_ok(row.case, SUGGEST, &json!({"QueryText": row.query}))
            .await;
        let found = sorted_place_titles(&body);
        assert_eq!(
            found, row.expected,
            "case {}: expected place suggestions {:?} for {:?}, got {found:?}",
            row.case, row.expected, row.query
        );
    }
}

// ---------------------------------------------------------------------------
// Which query terms a query names
// ---------------------------------------------------------------------------

/// A row of the query-term table: a query and the `(QueryType, Title)` pairs the
/// catalog's vocabulary answers it with.
struct TermRow {
    case: &'static str,
    query: &'static str,
    expected: &'static [(&'static str, &'static str)],
}

/// A query whose every word is accounted for by the words of a category or a
/// business chain the matched places carry is also answered with that term as a
/// `Query` suggestion, carrying a `QueryId`; a query that names only a place's own
/// text suggests no term.
///
/// The rows differ along the KIND of term named (a category, a chain, both, none)
/// and along how much of its name was typed.
#[tokio::test]
async fn suggest_names_the_query_terms_the_catalog_is_searched_by() {
    let emulator = Emulator::new();
    let rows = [
        TermRow {
            case: "category-whole-word",
            query: "coffee",
            expected: &[
                (TYPE_BUSINESS_CHAIN, "Blue Bottle Coffee"),
                (TYPE_CATEGORY, "Coffee Shop"),
                (TYPE_BUSINESS_CHAIN, "Compass Coffee"),
            ],
        },
        TermRow {
            case: "category-two-words",
            query: "coffee shop",
            expected: &[(TYPE_CATEGORY, "Coffee Shop")],
        },
        TermRow {
            case: "chain-partial-word",
            query: "Starb",
            expected: &[(TYPE_BUSINESS_CHAIN, "Starbucks")],
        },
        TermRow {
            case: "category-partial-word",
            query: "museu",
            expected: &[(TYPE_CATEGORY, "Museum")],
        },
        TermRow {
            case: "place-name-only",
            query: "Space Needle",
            expected: &[],
        },
        TermRow {
            case: "no-match",
            query: "Zzzzqqq",
            expected: &[],
        },
    ];

    for row in rows {
        let body = emulator
            .post_ok(row.case, SUGGEST, &json!({"QueryText": row.query}))
            .await;
        let found = query_terms(&body);
        let expected: Vec<(String, String)> = row
            .expected
            .iter()
            .map(|(query_type, term)| ((*query_type).to_string(), (*term).to_string()))
            .collect();
        assert_eq!(
            found, expected,
            "case {}: expected query suggestions {expected:?} for {:?}, got {found:?}",
            row.case, row.query
        );
        // Every query suggestion carries the id that runs it; no place suggestion
        // does, and no query suggestion carries a place.
        for item in of_kind(&body, ITEM_QUERY) {
            let query_id = text(&member(&item, "Query"), "QueryId");
            assert!(
                !query_id.is_empty(),
                "case {}: expected a QueryId on {item:?}",
                row.case
            );
            assert!(
                item.get("Place").is_none(),
                "case {}: expected no Place on a query suggestion, got {item:?}",
                row.case
            );
        }
        for item in of_kind(&body, ITEM_PLACE) {
            assert!(
                item.get("Query").is_none(),
                "case {}: expected no Query on a place suggestion, got {item:?}",
                row.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refinements
// ---------------------------------------------------------------------------

/// A row of the refinement table: a query, the number of refinements asked for,
/// and the `(OriginalTerm, RefinedTerm, StartIndex, EndIndex)` entries expected.
struct RefinementRow {
    case: &'static str,
    query: &'static str,
    max_refinements: Option<i64>,
    expected: &'static [(&'static str, &'static str, i64, i64)],
}

/// Each word the caller typed only in part is refined to the word of the matched
/// records it completes to, reported with the span it replaces; a word that
/// already spells a term of those records is complete and refined no further.
///
/// The rows differ along which word of the query is partial (the first, the last,
/// both, none), whether anything matched at all, and how many refinements the
/// caller asked for.
#[tokio::test]
async fn suggest_refines_the_words_a_query_typed_only_in_part() {
    let emulator = Emulator::new();
    let rows = [
        RefinementRow {
            case: "partial-last-word",
            query: "Space Nee",
            max_refinements: None,
            expected: &[("Nee", "Needle", 6, 9)],
        },
        RefinementRow {
            case: "both-words-partial",
            query: "Spac Nee",
            max_refinements: None,
            expected: &[("Spac", "Space", 0, 4), ("Nee", "Needle", 5, 8)],
        },
        RefinementRow {
            case: "bounded-to-one",
            query: "Spac Nee",
            max_refinements: Some(MAX_REFINEMENTS_MIN),
            expected: &[("Spac", "Space", 0, 4)],
        },
        RefinementRow {
            case: "bounded-to-the-modeled-maximum",
            query: "Spac Nee",
            max_refinements: Some(MAX_REFINEMENTS_MAX),
            expected: &[("Spac", "Space", 0, 4), ("Nee", "Needle", 5, 8)],
        },
        RefinementRow {
            case: "chain-name-partial",
            query: "Starb",
            max_refinements: None,
            expected: &[("Starb", "STARBUCKS", 0, 5)],
        },
        RefinementRow {
            case: "every-word-complete",
            query: "Space Needle",
            max_refinements: None,
            expected: &[],
        },
        RefinementRow {
            case: "no-match",
            query: "Zzzzqqq",
            max_refinements: None,
            expected: &[],
        },
    ];

    for row in rows {
        let mut request = json!({"QueryText": row.query});
        if let Some(limit) = row.max_refinements {
            request["MaxQueryRefinements"] = json!(limit);
        }
        let body = emulator.post_ok(row.case, SUGGEST, &request).await;
        let found = refinements(&body);
        let expected: Vec<(String, String, i64, i64)> = row
            .expected
            .iter()
            .map(|(original, refined, start, end)| {
                ((*original).to_string(), (*refined).to_string(), *start, *end)
            })
            .collect();
        assert_eq!(
            found, expected,
            "case {}: expected refinements {expected:?} for {:?}, got {found:?}",
            row.case, row.query
        );
        // The span a refinement reports is the span of the query text it replaces.
        for (original, _, start, end) in &found {
            assert_eq!(
                char_span(row.query, *start, *end),
                *original,
                "case {}: expected [{start}, {end}) of {:?} to be {original:?}",
                row.case,
                row.query
            );
        }
    }
}

// ---------------------------------------------------------------------------
// How many suggestions one call reports
// ---------------------------------------------------------------------------

/// `MaxResults` bounds the whole suggestion list, and a value outside the modeled
/// range — as does a `MaxQueryRefinements` outside its own — is refused with the
/// modeled `ValidationException` rather than silently clamped.
///
/// The rows differ along the page size asked for: each published bound, one past
/// each, and no limit at all (the documented default of 20).
#[tokio::test]
async fn suggest_bounds_the_number_of_suggestions_it_reports() {
    let emulator = Emulator::new();
    // A query that names more places than the smallest page can carry.
    let query = "coffee";

    let unlimited = emulator
        .post_ok("default-page", SUGGEST, &json!({"QueryText": query}))
        .await;
    let total = items(&unlimited).len();
    assert!(
        total > 1,
        "case default-page: expected {query:?} to name more than one suggestion, got {total}"
    );
    // "Default value: 20" (`SuggestRequest$MaxResults`).
    assert!(
        total <= 20,
        "case default-page: expected at most the documented default of 20 suggestions, got {total}"
    );

    for limit in [MAX_RESULTS_MIN, 2, MAX_RESULTS_MAX] {
        let case = format!("max-results-{limit}");
        let body = emulator
            .post_ok(
                &case,
                SUGGEST,
                &json!({"QueryText": query, "MaxResults": limit}),
            )
            .await;
        let found = items(&body).len();
        let expected = total.min(limit as usize);
        assert_eq!(
            found, expected,
            "case {case}: expected {expected} suggestions, got {found}"
        );
    }

    // One past each published bound, on both counts.
    let refusals = [
        ("max-results-below", json!({"MaxResults": MAX_RESULTS_MIN - 1})),
        ("max-results-above", json!({"MaxResults": MAX_RESULTS_MAX + 1})),
        (
            "max-refinements-below",
            json!({"MaxQueryRefinements": MAX_REFINEMENTS_MIN - 1}),
        ),
        (
            "max-refinements-above",
            json!({"MaxQueryRefinements": MAX_REFINEMENTS_MAX + 1}),
        ),
    ];
    for (case, extra) in refusals {
        let mut request = json!({"QueryText": query});
        for (member, value) in extra.as_object().expect("a JSON object row") {
            request[member] = value.clone();
        }
        let call = emulator.post(SUGGEST, &request).await;
        assert_eq!(
            call.status, 400,
            "case {case}: expected 400 for {request}, got {} with {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            VALIDATION_EXCEPTION,
            "case {case}: expected {VALIDATION_EXCEPTION}, got {:?}",
            call.error_code()
        );
    }
}

// ---------------------------------------------------------------------------
// Where the query asks for
// ---------------------------------------------------------------------------

/// A filter narrows the suggestions to the places it admits, a `BiasPosition`
/// measures each one's `Distance` from the biased position and orders the equally
/// matching ones nearest first, and two ways of saying WHERE at once are refused.
#[tokio::test]
async fn suggest_answers_within_the_area_the_request_names() {
    let emulator = Emulator::new();
    let query = "coffee";

    // With no reference position there is nothing to measure from, so `Distance`
    // keeps its modeled default of 0.
    let unbiased = emulator
        .post_ok("unbiased", SUGGEST, &json!({"QueryText": query}))
        .await;
    for item in of_kind(&unbiased, ITEM_PLACE) {
        let place = member(&item, "Place");
        assert_eq!(
            distance(&place),
            0,
            "case unbiased: expected Distance 0 for {:?}, got {}",
            text(&item, "Title"),
            distance(&place)
        );
    }

    // Pike Place Market, from the seeded catalog: a position with several of the
    // matching places around it.
    let bias = [-122.340_810, 47.608_849];
    let biased = emulator
        .post_ok(
            "biased",
            SUGGEST,
            &json!({"QueryText": query, "BiasPosition": bias}),
        )
        .await;
    let distances: Vec<i64> = of_kind(&biased, ITEM_PLACE)
        .iter()
        .map(|item| distance(&member(item, "Place")))
        .collect();
    assert!(
        !distances.is_empty(),
        "case biased: expected suggestions for {query:?}, got none"
    );
    assert!(
        distances.windows(2).all(|pair| pair[0] <= pair[1]),
        "case biased: expected non-decreasing distances, got {distances:?}"
    );
    assert!(
        distances.iter().any(|metres| *metres > 0),
        "case biased: expected a measured distance from {bias:?}, got {distances:?}"
    );

    // A bounding box admits the places inside it and nothing else.
    let area = [-122.5, 47.4, -122.2, 47.7];
    let boxed = emulator
        .post_ok(
            "bounding-box",
            SUGGEST,
            &json!({"QueryText": query, "Filter": {"BoundingBox": area}}),
        )
        .await;
    let inside = of_kind(&boxed, ITEM_PLACE);
    assert!(
        !inside.is_empty(),
        "case bounding-box: expected suggestions inside {area:?}, got none"
    );
    for item in &inside {
        let found = position(&member(item, "Place"));
        assert!(
            found.len() == 2
                && (area[0]..=area[2]).contains(&found[0])
                && (area[1]..=area[3]).contains(&found[1]),
            "case bounding-box: expected {:?} at {found:?} inside {area:?}",
            text(item, "Title")
        );
    }
    assert!(
        inside.len() < of_kind(&unbiased, ITEM_PLACE).len(),
        "case bounding-box: expected the box to narrow the {} unfiltered suggestions",
        of_kind(&unbiased, ITEM_PLACE).len()
    );

    // A country filter admits the places in that country alone.
    let french = emulator
        .post_ok(
            "include-countries",
            SUGGEST,
            &json!({"QueryText": query, "Filter": {"IncludeCountries": ["FRA"]}}),
        )
        .await;
    assert_eq!(
        sorted_place_titles(&french),
        vec!["Cafe de Flore".to_string()],
        "case include-countries: expected the French place alone, got {:?}",
        sorted_place_titles(&french)
    );

    // "The fields BiasPosition, FilterBoundingBox, and FilterCircle are mutually
    // exclusive" (`SuggestRequest$BiasPosition`).
    let conflicts = [
        (
            "bias-and-box",
            json!({"QueryText": query, "BiasPosition": bias, "Filter": {"BoundingBox": area}}),
        ),
        (
            "bias-and-circle",
            json!({
                "QueryText": query,
                "BiasPosition": bias,
                "Filter": {"Circle": {"Center": bias, "Radius": 2000}},
            }),
        ),
        (
            "box-and-circle",
            json!({
                "QueryText": query,
                "Filter": {"BoundingBox": area, "Circle": {"Center": bias, "Radius": 2000}},
            }),
        ),
    ];
    for (case, request) in conflicts {
        let call = emulator.post(SUGGEST, &request).await;
        assert_eq!(
            call.status, 400,
            "case {case}: expected 400 for {request}, got {} with {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            VALIDATION_EXCEPTION,
            "case {case}: expected {VALIDATION_EXCEPTION}, got {:?}",
            call.error_code()
        );
    }
}

// ---------------------------------------------------------------------------
// What each suggestion reports
// ---------------------------------------------------------------------------

/// A suggestion reports only the sections of ITS OWN text that the query's terms
/// accounted for, and those sections are spans of the text the response carries.
///
/// The rows differ along which piece of a suggestion the query names: its title,
/// its address label, both, and neither (a match on vocabulary no reported member
/// spells).
#[tokio::test]
async fn suggest_marks_the_sections_of_its_own_text_the_query_named() {
    let emulator = Emulator::new();
    let rows = [
        ("title", "Space Nee"),
        ("address-label", "1600 Pennsylvania"),
        ("postal-code", "75007"),
        ("category-only", "coffee"),
    ];

    for (case, query) in rows {
        let body = emulator
            .post_ok(case, SUGGEST, &json!({"QueryText": query}))
            .await;
        for item in items(&body) {
            let title = text(&item, "Title");
            let label = text(&member(&member(&item, "Place"), "Address"), "Label");
            let highlights = member(&item, "Highlights");
            let marked = [
                ("Title", title.clone(), member(&highlights, "Title")),
                (
                    "Address.Label",
                    label.clone(),
                    member(&member(&highlights, "Address"), "Label"),
                ),
            ];
            for (name, text_of, sections) in marked {
                let Some(sections) = sections.as_array() else {
                    continue;
                };
                assert!(
                    !sections.is_empty(),
                    "case {case}: expected {name} highlights to be absent rather than empty \
                     on {item:?}"
                );
                let mut previous_end = -1;
                for section in sections {
                    let start = section
                        .get("StartIndex")
                        .and_then(Value::as_i64)
                        .unwrap_or(-1);
                    let end = section.get("EndIndex").and_then(Value::as_i64).unwrap_or(-1);
                    let value = text(section, "Value");
                    assert!(
                        start >= 0 && end > start && end <= text_of.chars().count() as i64,
                        "case {case}: expected [{start}, {end}) inside {name} {text_of:?}"
                    );
                    assert!(
                        start > previous_end,
                        "case {case}: expected ordered, non-overlapping {name} sections, \
                         got {start} after {previous_end} in {text_of:?}"
                    );
                    previous_end = end;
                    assert_eq!(
                        value,
                        char_span(&text_of, start, end),
                        "case {case}: expected the {name} section [{start}, {end}) of \
                         {text_of:?}, got {value:?}"
                    );
                }
            }
            // A query naming only what a place IS spells none of the text the
            // suggestion reports, so it marks nothing there.
            if query == "coffee" && text(&item, "SuggestResultItemType") == ITEM_PLACE {
                let names_the_title = title.to_lowercase().contains("coffee");
                assert_eq!(
                    member(&highlights, "Title").is_null(),
                    !names_the_title,
                    "case {case}: expected Title highlights on {title:?} only when its own \
                     text spells the query, got {highlights:?}"
                );
            }
        }
    }
}

/// An `AdditionalFeatures` value is reported only when the request asks for it,
/// the suggestion's text is rendered in the requested `Language`, and what a place
/// suggestion reports agrees with what GetPlace reports for the same `PlaceId`.
#[tokio::test]
async fn suggest_reports_requested_features_and_renders_in_the_requested_language() {
    let emulator = Emulator::new();

    // `SuggestAdditionalFeature` values this catalog holds data for, with the
    // result member each one gates (the model's enum of the same name also carries
    // `Core`, the fields every suggestion reports, and `Phonemes` and
    // `CrossReferences`, which no catalog record holds).
    let features = [("TimeZone", "TimeZone"), ("Access", "AccessPoints")];
    for (feature, gated) in features {
        let without = emulator
            .post_ok(feature, SUGGEST, &json!({"QueryText": "Space Needle"}))
            .await;
        let place = member(&of_kind(&without, ITEM_PLACE)[0], "Place");
        assert!(
            place.get(gated).is_none(),
            "case {feature}: expected no {gated} when the request does not ask for it, \
             got {place:?}"
        );

        let with = emulator
            .post_ok(
                feature,
                SUGGEST,
                &json!({"QueryText": "Space Needle", "AdditionalFeatures": [feature]}),
            )
            .await;
        let place = member(&of_kind(&with, ITEM_PLACE)[0], "Place");
        assert!(
            place.get(gated).is_some(),
            "case {feature}: expected {gated} when the request asks for {feature}, \
             got {place:?}"
        );
    }

    // "If there is no data for the result in the requested language, data will be
    // returned in the default language for the entry"
    // (`SuggestRequest$Language`): the catalog's Eiffel Tower record holds an
    // English name, its Louvre record holds one too, and its Cafe de Flore record
    // holds none.
    let localized = [
        ("default-language", None, "Tour Eiffel", "Tour Eiffel"),
        ("english", Some("en"), "Tour Eiffel", "Eiffel Tower"),
        ("unheld-language", Some("ja"), "Tour Eiffel", "Tour Eiffel"),
        ("english-louvre", Some("en"), "Louvre", "Louvre Museum"),
        ("english-unheld", Some("en"), "Cafe de Flore", "Cafe de Flore"),
    ];
    for (case, language, query, expected) in localized {
        let mut request = json!({"QueryText": query});
        if let Some(language) = language {
            request["Language"] = json!(language);
        }
        let body = emulator.post_ok(case, SUGGEST, &request).await;
        let titles = place_titles(&body);
        assert!(
            titles.iter().any(|title| title == expected),
            "case {case}: expected {expected:?} among the suggestions for {query:?}, \
             got {titles:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Resolving a suggestion
// ---------------------------------------------------------------------------

/// Every place suggestion resolves through GetPlace to the same record: the
/// `PlaceId` it hands out is the catalog's own, and the title, position and
/// address label the two operations report agree.
///
/// The rows differ along the query form, so the round trip is exercised on
/// suggestions reached by a name, by a street and number, by a postal code and by
/// the category vocabulary.
#[tokio::test]
async fn suggest_place_suggestions_resolve_through_get_place() {
    let emulator = Emulator::new();
    for query in ["Space Nee", "1600 Pennsylvania", "75007", "coffee"] {
        let body = emulator
            .post_ok(query, SUGGEST, &json!({"QueryText": query}))
            .await;
        let suggestions = of_kind(&body, ITEM_PLACE);
        assert!(
            !suggestions.is_empty(),
            "case {query}: expected place suggestions, got none"
        );
        for item in suggestions {
            let place = member(&item, "Place");
            let place_id = text(&place, "PlaceId");
            assert!(
                !place_id.is_empty(),
                "case {query}: expected a PlaceId on {item:?}"
            );
            let read = emulator
                .get_ok(query, &format!("{PLACE}/{place_id}"))
                .await;
            assert_eq!(
                text(&read, "Title"),
                text(&item, "Title"),
                "case {query}: expected GetPlace to report the suggested title for {place_id}"
            );
            assert_eq!(
                position(&read),
                position(&place),
                "case {query}: expected GetPlace to report the suggested position for {place_id}"
            );
            assert_eq!(
                text(&member(&read, "Address"), "Label"),
                text(&member(&place, "Address"), "Label"),
                "case {query}: expected GetPlace to report the suggested label for {place_id}"
            );
            assert_eq!(
                text(&read, "PlaceType"),
                text(&place, "PlaceType"),
                "case {query}: expected GetPlace to report the suggested type for {place_id}"
            );
        }
    }
}

/// A query suggestion's `QueryId` is "the query Id returned by the suggest API"
/// SearchText takes: handing it back runs the query the term names, and every
/// place it answers with carries that term — while an id this service never minted
/// is refused.
///
/// The rows differ along the KIND of term the id stands for (a category, a
/// business chain) and along how much of its name the query typed.
#[tokio::test]
async fn suggest_query_suggestions_resolve_through_search_text() {
    let emulator = Emulator::new();
    let rows = [
        ("category", "coffee shop", TYPE_CATEGORY, "coffee_shop"),
        ("chain", "Starb", TYPE_BUSINESS_CHAIN, "Starbucks"),
    ];

    for (case, query, expected_type, expected_mark) in rows {
        let body = emulator
            .post_ok(case, SUGGEST, &json!({"QueryText": query}))
            .await;
        let suggestion = of_kind(&body, ITEM_QUERY)
            .into_iter()
            .find(|item| text(&member(item, "Query"), "QueryType") == expected_type)
            .unwrap_or_else(|| {
                panic!("case {case}: expected a {expected_type} suggestion for {query:?}")
            });
        let query_id = text(&member(&suggestion, "Query"), "QueryId");

        let searched = emulator
            .post_ok(case, SEARCH_TEXT, &json!({"QueryId": query_id}))
            .await;
        let found = items(&searched);
        assert!(
            !found.is_empty(),
            "case {case}: expected SearchText to answer the suggested query {query_id:?}, \
             got no items"
        );
        for item in &found {
            let marks: Vec<String> = match expected_type {
                TYPE_CATEGORY => category_ids(item),
                _ => item
                    .get("BusinessChains")
                    .and_then(Value::as_array)
                    .map(|chains| chains.iter().map(|chain| text(chain, "Name")).collect())
                    .unwrap_or_default(),
            };
            assert!(
                marks.iter().any(|mark| mark == expected_mark),
                "case {case}: expected every result of the suggested query to carry \
                 {expected_mark:?}, got {marks:?} for {:?}",
                text(item, "Title")
            );
        }

        // The places it answers with resolve through GetPlace, which is the
        // workflow a suggestion is for: suggest a follow-on query, search it, read
        // the place.
        let place_id = text(&found[0], "PlaceId");
        let read = emulator
            .get_ok(case, &format!("{PLACE}/{place_id}"))
            .await;
        assert_eq!(
            text(&read, "Title"),
            text(&found[0], "Title"),
            "case {case}: expected GetPlace to report the searched title for {place_id}"
        );
    }

    // An id this service never minted names no query. The last row is WELL-FORMED
    // but carries more query text than a free-form query may itself hold
    // (`SearchTextRequest$QueryText`, `len 1..200`), which no mint produces; the
    // row before it sits exactly on that ceiling and is answered.
    let at_the_ceiling = encode_query_id(TYPE_CATEGORY, &"a".repeat(QUERY_TEXT_MAX));
    let past_the_ceiling = encode_query_id(TYPE_CATEGORY, &"a".repeat(QUERY_TEXT_MAX + 1));
    let answered = emulator
        .post_ok("term-at-the-ceiling", SEARCH_TEXT, &json!({"QueryId": at_the_ceiling}))
        .await;
    assert!(
        items(&answered).is_empty(),
        "case term-at-the-ceiling: expected a query no place matches to answer with no \
         items, got {:?}",
        items(&answered)
    );
    for (case, query_id) in [
        ("not-a-token", "not-a-query-id".to_string()),
        ("empty-hex", String::new()),
        ("foreign-hex", "6e6f7065".to_string()),
        ("term-past-the-ceiling", past_the_ceiling),
    ] {
        let call = emulator
            .post(SEARCH_TEXT, &json!({"QueryId": query_id}))
            .await;
        assert_eq!(
            call.status, 400,
            "case {case}: expected 400 for QueryId {query_id:?}, got {} with {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            VALIDATION_EXCEPTION,
            "case {case}: expected {VALIDATION_EXCEPTION}, got {:?}",
            call.error_code()
        );
    }
}

/// Suggest is a REPORTING operation: the same query answered twice reports the
/// same suggestions, and a suggestion handed to SearchText and GetPlace leaves
/// what a later caller sees unchanged.
#[tokio::test]
async fn suggest_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let request = json!({"QueryText": "coffee"});

    let first = emulator.post_ok("first", SUGGEST, &request).await;
    let query_id = of_kind(&first, ITEM_QUERY)
        .first()
        .map(|item| text(&member(item, "Query"), "QueryId"))
        .unwrap_or_default();
    assert!(
        !query_id.is_empty(),
        "case first: expected a query suggestion to resolve, got {first}"
    );
    let place_id = text(&member(&of_kind(&first, ITEM_PLACE)[0], "Place"), "PlaceId");

    let _ = emulator
        .post_ok("replay-search", SEARCH_TEXT, &json!({"QueryId": query_id}))
        .await;
    let _ = emulator
        .get_ok("replay-place", &format!("{PLACE}/{place_id}"))
        .await;

    let second = emulator.post_ok("second", SUGGEST, &request).await;
    assert_eq!(
        first, second,
        "case second: expected the same query to report the same suggestions"
    );
}

/// Every success carries the `@required` `PricingBucket`, and `IntendedUse` models
/// only `SingleUse` ("currently, Suggest does not support storage of results"), so
/// every call is charged in the same tier.
#[tokio::test]
async fn suggest_reports_the_pricing_bucket_every_response_requires() {
    let emulator = Emulator::new();
    for (case, request) in [
        ("default", json!({"QueryText": "coffee"})),
        (
            "single-use",
            json!({"QueryText": "coffee", "IntendedUse": "SingleUse"}),
        ),
        ("no-match", json!({"QueryText": "Zzzzqqq"})),
    ] {
        let call = emulator.post(SUGGEST, &request).await;
        assert_success(case, &call);
        // `PricingBucket` is bound to a header
        // (`@httpHeader(x-amz-geo-pricing-bucket)`), not to the body.
        assert_eq!(
            call.header(PRICING_BUCKET_HEADER),
            PRICING_BUCKET_CORE,
            "case {case}: expected the default pricing tier, got {:?}",
            call.header(PRICING_BUCKET_HEADER)
        );
    }
}

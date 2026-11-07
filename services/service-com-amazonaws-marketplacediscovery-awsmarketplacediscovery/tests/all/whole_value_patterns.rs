// SPDX-License-Identifier: CC0-1.0

//! A member's modelled `@pattern` is satisfied only by a value that matches it over its
//! WHOLE length.
//!
//! The rule under test: a string input member carrying a `@pattern` is accepted only when
//! the whole of its value matches that expression, and a value that merely CONTAINS a
//! match is refused with HTTP 400 `ValidationException` naming the member. Smithy
//! specifies the trait as a find (match somewhere in the value), which coincides with the
//! whole-value reading for every expression a model anchors with `^` / `$` and is weaker
//! for an unanchored one — and this service models exactly one unanchored pattern,
//! `SearchText`'s `[^\n]*` (`smithy_query(query="shape",shape="SearchText")`), carried by
//! `SearchFacetsInput$searchText` and `SearchListingsInput$searchText`.
//!
//! So the suites below span:
//!
//! * the unanchored member, over a set of values differing in WHERE the line feed sits,
//!   how many there are, and what else the value holds — on BOTH operations that carry it;
//!   how that refusal is WORDED is this service's own sentence, pinned in
//!   `tests/all/refusal_wording.rs`;
//! * every other character the same pattern admits, swept over the whole ASCII control
//!   range plus the sizes and scripts a search query can carry, which must still be
//!   answered from the catalog;
//! * an anchored member (`ListingId`'s `^[\w\-]+$`), which the framework's own input walk
//!   already enforces over the whole value — the rule holds there with no second check.
//!
//! Every request goes through the `MultiServiceRouter`, so the protocol, the routing table,
//! the framework's input walk and the handlers are all exercised end to end.

use serde_json::{json, Value};

// The one-service router this crate's suites share, declared once in `catalog_drilldown`,
// and the refusal harness (`call` reporting the `x-amzn-errortype` header, `refused`
// asserting the status and the error code) declared once in `validation_message_rules` —
// reused here rather than copied.
use crate::catalog_drilldown::router;
use crate::refusal_wording::NEWLINE_REFUSAL;
use crate::validation_message_rules::{call, refused, VALIDATION_STATUS};

// The generated input schemas and the model-pattern lookup the service's whole-value check
// reads, so the test asserts against the MODEL rather than a transcribed expression.
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::ops::types::{
    SEARCH_FACETS_INPUT_SCHEMA, SEARCH_LISTINGS_INPUT_SCHEMA,
};
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state::modelled_member_pattern;

/// The sentence this service refuses a `searchText` outside its modelled pattern with: the
/// member's name and the rule it broke, declared once in `tests/all/refusal_wording.rs`
/// (which pins it as a wording rule, over the same class of values and the parts the
/// sentence may not carry). The pattern excludes one character, the line feed, which is
/// what the sentence states.
///
/// Source: `smithy_query(query="shape",shape="SearchText")` — `@length 1..512`,
/// `@pattern "[^\n]*"`, which the model spells with a LITERAL line feed inside the
/// character class.
const SEARCH_TEXT_PATTERN: &str = r"[^\n]*";

/// The two operations that carry a `searchText`, with the member each one's successful
/// answer reports its matches through.
const SEARCH_OPERATIONS: [(&str, &str); 2] = [
    ("searchFacets", "listingFacets"),
    ("searchListings", "listingSummaries"),
];

/// A `searchText` request for one operation: `SearchListings` is paged by `maxResults`,
/// `SearchFacets` models none, and neither needs anything else.
fn search_input(operation: &str, search_text: &str) -> Value {
    let mut input = json!({ "searchText": search_text });
    if operation == "searchListings" {
        input["maxResults"] = json!(100);
    }
    input
}

// ---------------------------------------------------------------------------
// The unanchored member: a value carrying a line feed is outside `[^\n]*`.
// ---------------------------------------------------------------------------

/// Values the `SearchText` pattern does not admit, differing in where the line feed sits,
/// how many there are, and what surrounds them. Each row is a `searchText` the pattern IS
/// found in (every one of them contains a run of non-line-feed characters, which is what
/// makes a find-semantics check accept them) while the whole value does not match it.
fn line_feed_values() -> Vec<(&'static str, String)> {
    vec![
        ("a line feed between two words", "data\nanalytics".to_string()),
        ("a leading line feed", "\nanalytics".to_string()),
        ("a trailing line feed", "analytics\n".to_string()),
        ("nothing but a line feed", "\n".to_string()),
        ("two line feeds", "data\nanalytics\nfirewall".to_string()),
        (
            "a line feed among punctuation and markup",
            "data & <analytics>\n(firewall)".to_string(),
        ),
        (
            "a carriage return beside the line feed",
            "data\r\nanalytics".to_string(),
        ),
        (
            "a line feed after non-ASCII text",
            "données\nanalytiques".to_string(),
        ),
        (
            "a line feed at the member's maximum length",
            format!("{}\n", "x".repeat(511)),
        ),
        (
            "a line feed in a value that is only whitespace",
            " \t\n\t ".to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_search_text_outside_its_pattern_is_refused_on_every_operation_carrying_it() {
    let router = router();
    for (operation, _) in SEARCH_OPERATIONS {
        for (case, value) in line_feed_values() {
            let case = format!("{operation} searchText with {case}");
            // `refused` pins HTTP 400, the `ValidationException` code in the body's
            // `__type` and in `x-amzn-errortype`, and hands back the message.
            let message = refused(&router, &case, operation, &search_input(operation, &value)).await;
            assert_eq!(
                message, NEWLINE_REFUSAL,
                "case {case:?}: expected this service's own refusal {NEWLINE_REFUSAL:?}, \
                 got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the pattern DOES admit is still answered from the catalog.
// ---------------------------------------------------------------------------

/// Values inside `SearchText`'s `@length 1..512` and `@pattern [^\n]*`: the pattern
/// forbids one character and nothing else, so each of these is a query the service has to
/// run. Rows differ in size (one character, the maximum 512), in script, and in which
/// otherwise-suspicious characters they carry.
fn admitted_values() -> Vec<(&'static str, String)> {
    vec![
        ("a plain term", "analytics".to_string()),
        ("a single character", "a".to_string()),
        ("two words", "data analytics".to_string()),
        ("a tab", "data\tanalytics".to_string()),
        (
            "a carriage return, which the pattern does not forbid",
            "data\ranalytics".to_string(),
        ),
        ("markup characters", "data & <analytics>".to_string()),
        ("a quote and a backslash", "data \"a\\b\"".to_string()),
        ("non-ASCII text", "données analytiques".to_string()),
        ("an emoji", "analytics 📊".to_string()),
        ("the maximum length", "x".repeat(512)),
        ("a term nothing matches", "no-such-listing".to_string()),
    ]
}

#[tokio::test]
async fn a_search_text_inside_its_pattern_is_answered_from_the_catalog() {
    let router = router();
    for (operation, member) in SEARCH_OPERATIONS {
        for (case, value) in admitted_values() {
            let case = format!("{operation} searchText with {case}");
            let (status, _, body) = call(&router, operation, &search_input(operation, &value)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert!(
                body.get(member).is_some(),
                "case {case:?}: expected the response to carry {member:?}, got {body}"
            );
            assert!(
                body.get("totalResults").and_then(Value::as_i64).is_some(),
                "case {case:?}: expected totalResults counted over the catalog, got {body}"
            );
        }
    }
}

/// The pattern names ONE forbidden character, so the line feed is the only control
/// character a `searchText` is refused for. The sweep is the whole ASCII control range plus
/// `DEL`, which is the published character class `[^\n]*` read literally rather than a
/// hand-picked list.
#[tokio::test]
async fn the_line_feed_is_the_only_character_the_pattern_forbids() {
    let router = router();
    for code in (0x00u8..=0x1f).chain(std::iter::once(0x7f)) {
        let value = format!("data{}analytics", char::from(code));
        let case = format!("searchFacets searchText carrying control character {code:#04x}");
        let (status, _, body) =
            call(&router, "searchFacets", &search_input("searchFacets", &value)).await;
        let expected = if code == b'\n' { VALIDATION_STATUS } else { 200 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} with body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The expression enforced is the one the MODEL declares, for both operations.
// ---------------------------------------------------------------------------

/// The two inputs carrying `searchText` declare the SAME `@pattern`, which is what makes
/// one whole-value check serve both operations. Read off the generated input schemas, so a
/// model update that gave either member its own expression fails here instead of silently
/// leaving one operation checked against the other's.
#[test]
fn both_search_inputs_declare_the_same_search_text_pattern() {
    let facets = modelled_member_pattern(SEARCH_FACETS_INPUT_SCHEMA, "searchText");
    let listings = modelled_member_pattern(SEARCH_LISTINGS_INPUT_SCHEMA, "searchText");
    assert_eq!(
        facets, listings,
        "case the searchText member of both search inputs: expected one modelled pattern, \
         got {facets:?} and {listings:?}"
    );
    // The expression the service enforces is this one once its literal control characters
    // are written as escapes, which is the class the refusals above span.
    assert_eq!(
        facets.map(|pattern| pattern.replace('\n', r"\n")),
        Some(SEARCH_TEXT_PATTERN.to_string()),
        "case the modelled searchText pattern: expected {SEARCH_TEXT_PATTERN:?} once its \
         literal control characters are written as escapes, got {facets:?}"
    );
}

// ---------------------------------------------------------------------------
// An anchored pattern: the same rule, already enforced by the input walk.
// ---------------------------------------------------------------------------

/// A member whose modelled pattern IS anchored holds to the same rule: a value carrying a
/// line feed is refused, because `^…$` and the whole-value reading agree. Pinned on
/// `ListingId` (`^[\w\-]+$`), and on the id a search reports, which still reads back.
#[tokio::test]
async fn an_anchored_pattern_is_enforced_over_the_whole_value_too() {
    let router = router();

    let listings = call(
        &router,
        "searchListings",
        &json!({ "maxResults": 1, "searchText": "a" }),
    )
    .await
    .2;
    let listing_id = listings
        .get("listingSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| summaries.first())
        .and_then(|summary| summary.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !listing_id.is_empty(),
        "case a seeded listing id: expected SearchListings to report one, got {listings}"
    );

    // Rows: the reported id with a line feed spliced in at each position, and the id
    // itself, which the same check must still admit.
    let cases: Vec<(String, String, u16)> = vec![
        (
            "listingId with an embedded line feed".to_string(),
            format!("{listing_id}\n{listing_id}"),
            VALIDATION_STATUS,
        ),
        (
            "listingId with a trailing line feed".to_string(),
            format!("{listing_id}\n"),
            VALIDATION_STATUS,
        ),
        (
            "listingId with a leading line feed".to_string(),
            format!("\n{listing_id}"),
            VALIDATION_STATUS,
        ),
        (
            "the listing id the search reported".to_string(),
            listing_id.clone(),
            200,
        ),
    ];
    for (case, value, expected) in cases {
        let input = json!({ "listingId": value });
        if expected == VALIDATION_STATUS {
            let message = refused(&router, &case, "getListing", &input).await;
            assert!(
                message.contains("listingId"),
                "case {case:?}: expected the message to name the member listingId, \
                 got {message:?}"
            );
        } else {
            let (status, _, body) = call(&router, "getListing", &input).await;
            assert_eq!(
                status, expected,
                "case {case:?}: expected HTTP {expected}, got {status} with body {body}"
            );
            assert_eq!(
                body.get("listingId").and_then(Value::as_str),
                Some(listing_id.as_str()),
                "case {case:?}: expected the listing to read back, got {body}"
            );
        }
    }
}

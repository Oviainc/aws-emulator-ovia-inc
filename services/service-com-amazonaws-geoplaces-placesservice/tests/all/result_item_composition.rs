// SPDX-License-Identifier: CC0-1.0

//! How a Places result ITEM is composed: the assembled address label and display
//! title, the address components (present ones only, with the street split into
//! components), the match scores and parsed query a geocode result reports, the
//! granularity a structured query resolves at, which places a text query may
//! answer with, and the pricing tier every response is charged in.
//!
//! Each test is a TABLE whose rows differ along what its rule generalises over —
//! the place's granularity (country down to point address), the postal convention
//! of its country, the street's shape, the query form, the operation — rather than
//! along one recorded request. The emulator is driven as real HTTP through the same
//! router the front door serves, reusing the harness in [`crate::places_catalog`].

use serde_json::{json, Value};

use crate::places_catalog::{
    assert_success, distance, items, text, Emulator, Place, PRICING_BUCKET_HEADER,
};
/// The pricing tier every Places response is charged in by default: a tier token
/// shared by the operations, never the name of the operation that served the call.
const PRICING_BUCKET_CORE: &str = "Core";

/// The tier a request that declares `IntendedUse: Storage` is charged in: the
/// per-operation docs say such requests "will be charged at a higher rate".
const PRICING_BUCKET_STORAGE: &str = "Storage";

/// The catalog record a test names, found by the place's own name and its
/// `PlaceType` (the catalog holds "Washington" both as a locality and as a region).
fn place(emulator: &Emulator, name: &str, place_type: &str) -> Place {
    emulator
        .catalog()
        .into_iter()
        .find(|place| place.name == name && place.place_type == place_type)
        .unwrap_or_else(|| panic!("the catalog seeds {name:?} as a {place_type}"))
}

/// One result item's `Address`, or `Value::Null` when it carries none.
pub(crate) fn address(item: &Value) -> Value {
    item.get("Address").cloned().unwrap_or(Value::Null)
}

/// Every string anywhere inside a JSON value, with the member path it sits at.
fn strings(value: &Value, path: &str, found: &mut Vec<(String, String)>) {
    match value {
        Value::String(text) => found.push((path.to_string(), text.clone())),
        Value::Object(members) => {
            for (member, value) in members {
                strings(value, &format!("{path}.{member}"), found);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                strings(value, &format!("{path}[{index}]"), found);
            }
        }
        _ => {}
    }
}

/// A `MatchScore` member, which carries a modeled `@default` of 0, so an absent
/// member reads as "no tokens were matched".
fn score(value: &Value, member: &str) -> f64 {
    value.get(member).and_then(Value::as_f64).unwrap_or(0.0)
}

/// The `ParsedQuery.Address` components of a result item.
fn parsed_address(item: &Value) -> Value {
    item.get("ParsedQuery")
        .and_then(|parsed| parsed.get("Address"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The titles a response returned, in order.
pub(crate) fn titles(body: &Value) -> Vec<String> {
    items(body)
        .iter()
        .map(|item| text(item, "Title"))
        .collect()
}

// ---------------------------------------------------------------------------
// Address.Label and Title
// ---------------------------------------------------------------------------

/// A row of the label table: a seeded place, and the `Title` and `Address.Label`
/// the service must assemble for it.
struct LabelRow {
    name: &'static str,
    place_type: &'static str,
    title: &'static str,
    label: &'static str,
}

/// `Address.Label` is "assembled address value built out of the address
/// components, according to the regional postal rules", and `Title` is "the
/// localized display name of this result item" — the place's own name where it has
/// one, and that assembled label where it has not.
///
/// The rows span the granularities the catalog holds (country, region, sub-region,
/// locality, district, point address, point of interest) in BOTH postal
/// conventions it holds: the US one puts the state abbreviation and the ZIP on the
/// settlement line, the French one puts the postal code before the locality. The
/// read-back goes through GetPlace, so a label handed out by a search resolves to
/// the same label by `PlaceId`.
#[tokio::test]
async fn an_address_label_is_assembled_from_the_components_per_regional_rules() {
    let emulator = Emulator::new();
    let rows = [
        LabelRow {
            name: "United States",
            place_type: "Country",
            title: "United States",
            label: "United States",
        },
        LabelRow {
            name: "France",
            place_type: "Country",
            title: "France",
            label: "France",
        },
        LabelRow {
            name: "Washington",
            place_type: "Region",
            title: "Washington, United States",
            label: "Washington, United States",
        },
        LabelRow {
            name: "Ile-de-France",
            place_type: "Region",
            title: "Ile-de-France, France",
            label: "Ile-de-France, France",
        },
        LabelRow {
            name: "King County",
            place_type: "SubRegion",
            title: "King County, WA, United States",
            label: "King County, WA, United States",
        },
        LabelRow {
            // A city's label stops at the settlement line: the record holds a
            // postal code, which belongs to a street address and not to the city.
            name: "Seattle",
            place_type: "Locality",
            title: "Seattle, WA, United States",
            label: "Seattle, WA, United States",
        },
        LabelRow {
            name: "Paris",
            place_type: "Locality",
            title: "Paris, Ile-de-France, France",
            label: "Paris, Ile-de-France, France",
        },
        LabelRow {
            name: "Capitol Hill",
            place_type: "District",
            title: "Capitol Hill, Seattle, WA, United States",
            label: "Capitol Hill, Seattle, WA, United States",
        },
        LabelRow {
            name: "400 Broad Street",
            place_type: "PointAddress",
            title: "400 Broad St, Seattle, WA 98109, United States",
            label: "400 Broad St, Seattle, WA 98109, United States",
        },
        LabelRow {
            // A point of interest has a name of its own, and it leads the label.
            name: "Space Needle",
            place_type: "PointOfInterest",
            title: "Space Needle",
            label: "Space Needle, 400 Broad St, Seattle, WA 98109, United States",
        },
        LabelRow {
            // The French convention: the postal code precedes the locality, and
            // the region is not part of a street address.
            name: "Tour Eiffel",
            place_type: "PointOfInterest",
            title: "Tour Eiffel",
            label: "Tour Eiffel, 5 Av. Anatole France, 75007 Paris, France",
        },
    ];

    for row in rows {
        let seeded = place(&emulator, row.name, row.place_type);
        let detail = emulator
            .get_ok(row.name, &format!("/v2/place/{}", seeded.place_id))
            .await;
        assert_eq!(
            text(&detail, "Title"),
            row.title,
            "case {}: expected the assembled display title, got {:?}",
            row.name,
            text(&detail, "Title")
        );
        assert_eq!(
            text(&address(&detail), "Label"),
            row.label,
            "case {}: expected the assembled label, got {:?}",
            row.name,
            text(&address(&detail), "Label")
        );

        // An address component this catalog does not hold is ABSENT, never an
        // empty string: `""` would be a postal code of no characters.
        let mut found = Vec::new();
        strings(&address(&detail), "Address", &mut found);
        for (path, value) in found {
            assert!(
                !value.is_empty(),
                "case {}: {path} must be absent rather than empty, got {value:?}",
                row.name
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Address.StreetComponents
// ---------------------------------------------------------------------------

/// A row of the street-components table: a seeded place, and the components its
/// street name must split into.
struct StreetRow {
    name: &'static str,
    place_type: &'static str,
    /// `None` where the place names no street, so it has no street components.
    components: Option<(
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static str,
    )>,
}

/// `Address.StreetComponents` ("Components of the street") splits the street name
/// into its base name, the street type and the side of the base name the regional
/// convention places it on, the separator between them, and the directional suffix
/// that is "not included in the base name".
///
/// The rows differ along the street's SHAPE: a bare `base type`, a `base type
/// directional`, a multi-word base name, a French `type base`, and a place with no
/// street at all. The street types and directionals are the standard
/// abbreviations (USPS Publication 28, appendices B and C1) the catalog's street
/// names use.
#[tokio::test]
async fn the_street_name_is_split_into_street_components() {
    let emulator = Emulator::new();
    let rows = [
        StreetRow {
            name: "Space Needle",
            place_type: "PointOfInterest",
            components: Some(("Broad", "St", "AfterBaseName", None, "en")),
        },
        StreetRow {
            name: "White House",
            place_type: "PointOfInterest",
            components: Some(("Pennsylvania", "Ave", "AfterBaseName", Some("NW"), "en")),
        },
        StreetRow {
            name: "Lincoln Memorial",
            place_type: "PointOfInterest",
            components: Some((
                "Lincoln Memorial",
                "Cir",
                "AfterBaseName",
                Some("NW"),
                "en",
            )),
        },
        StreetRow {
            name: "Tour Eiffel",
            place_type: "PointOfInterest",
            components: Some(("Anatole France", "Av.", "BeforeBaseName", None, "fr")),
        },
        StreetRow {
            name: "Seattle",
            place_type: "Locality",
            components: None,
        },
    ];

    for row in rows {
        let seeded = place(&emulator, row.name, row.place_type);
        let detail = emulator
            .get_ok(row.name, &format!("/v2/place/{}", seeded.place_id))
            .await;
        let components = address(&detail)
            .get("StreetComponents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let Some((base_name, street_type, placement, suffix, language)) = row.components else {
            assert!(
                components.is_empty(),
                "case {}: a place with no street has no street components, got {components:?}",
                row.name
            );
            continue;
        };
        let component = components.first().cloned().unwrap_or(Value::Null);
        for (member, expected) in [
            ("BaseName", base_name),
            ("Type", street_type),
            ("TypePlacement", placement),
            // The modeled `TypeSeparator` is `""` or `" "`.
            ("TypeSeparator", " "),
            ("Language", language),
        ] {
            assert_eq!(
                text(&component, member),
                expected,
                "case {}: expected {member} {expected:?}, got {:?}",
                row.name,
                text(&component, member)
            );
        }
        assert_eq!(
            component.get("Suffix").and_then(Value::as_str),
            suffix,
            "case {}: expected the directional suffix {suffix:?}, got {:?}",
            row.name,
            component.get("Suffix")
        );
    }
}

// ---------------------------------------------------------------------------
// MatchScores and ParsedQuery
// ---------------------------------------------------------------------------

/// Every geocode result reports `MatchScores` — neither it nor `ParsedQuery` is
/// gated behind `AdditionalFeatures`, whose enum is `TimeZone | Access |
/// SecondaryAddresses | Intersections` — and `Overall` stays inside the modeled
/// `MatchScore` range of 0..1.
///
/// The rows differ along the query form (free text at several granularities, a
/// place name, a structured query, a query that matches by category only).
#[tokio::test]
async fn every_geocode_result_reports_match_scores() {
    let emulator = Emulator::new();
    let rows = [
        ("free-text-locality", json!({"QueryText": "Seattle"})),
        ("free-text-name", json!({"QueryText": "Pike Place Market"})),
        ("free-text-postal", json!({"QueryText": "98109 Seattle"})),
        ("free-text-category", json!({"QueryText": "Museum"})),
        (
            "structured",
            json!({"QueryComponents": {"Country": "USA", "Locality": "Seattle"}}),
        ),
    ];

    for (case, body) in rows {
        let response = emulator.post_ok(case, "/v2/geocode", &body).await;
        let returned = items(&response);
        assert!(
            !returned.is_empty(),
            "case {case}: the query must match at least one seeded place"
        );
        for item in returned {
            let scores = item.get("MatchScores").cloned().unwrap_or(Value::Null);
            assert!(
                scores.is_object(),
                "case {case}: every result reports MatchScores, got {scores:?}"
            );
            let overall = score(&scores, "Overall");
            assert!(
                (0.0..=1.0).contains(&overall),
                "case {case}: Overall must be inside the modeled range 0..1, got {overall}"
            );
        }
    }
}

/// A free-form geocode query's components are parsed out of the TEXT: each span
/// the service recognized is reported under the address component it was
/// recognized as, with the indices it occupies in the query, the text as the
/// caller typed it, and `QueryComponent` "Query" — and the same component scores 1
/// in `MatchScores.Components`, while a component the query never named scores 0.
#[tokio::test]
async fn a_free_form_query_is_parsed_into_the_components_it_named() {
    let emulator = Emulator::new();
    // (case, query, component, start, end, value) — the rows differ along WHERE in
    // the query the span sits and which component it is.
    let rows = [
        ("leading-locality", "Seattle", "Locality", 0, 7, "Seattle"),
        ("trailing-locality", "98109 Seattle", "Locality", 6, 13, "Seattle"),
        ("leading-postal", "98109 Seattle", "PostalCode", 0, 5, "98109"),
        ("lower-cased-input", "seattle", "Locality", 0, 7, "seattle"),
        ("region-code", "Seattle WA", "Region", 8, 10, "WA"),
    ];

    for (case, query, component, start, end, value) in rows {
        let response = emulator
            .post_ok(
                case,
                "/v2/geocode",
                &json!({"QueryText": query, "MaxResults": 100}),
            )
            .await;
        let parsed = items(&response)
            .into_iter()
            .map(|item| parsed_address(&item))
            .find(|parsed| parsed.get(component).is_some())
            .unwrap_or_else(|| {
                panic!("case {case}: expected a result parsing {component} out of {query:?}")
            });
        let spans = parsed
            .get(component)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let span = spans.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            (
                span.get("StartIndex").and_then(Value::as_i64),
                span.get("EndIndex").and_then(Value::as_i64),
                text(&span, "Value"),
                text(&span, "QueryComponent"),
            ),
            (Some(start), Some(end), value.to_string(), "Query".to_string()),
            "case {case}: expected {component} parsed at {start}..{end} as {value:?} from the \
             free-form query, got {span:?}"
        );
    }
}

/// A structured geocode query echoes every component the caller supplied as parsed,
/// each under its own name, and scores each of them in `MatchScores.Components`;
/// components the caller did not supply score 0.
#[tokio::test]
async fn a_structured_query_echoes_and_scores_the_components_it_was_given() {
    let emulator = Emulator::new();
    let response = emulator
        .post_ok(
            "structured",
            "/v2/geocode",
            &json!({
                "QueryComponents": {
                    "Country": "USA",
                    "Region": "Washington",
                    "Locality": "Seattle",
                },
            }),
        )
        .await;

    let returned = items(&response);
    assert_eq!(
        titles(&response),
        vec!["Seattle, WA, United States".to_string()],
        "case structured: a country/region/locality query resolves to that locality \
         alone, got {:?}",
        titles(&response)
    );
    let item = returned.first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        text(&item, "PlaceType"),
        "Locality",
        "case structured: expected the locality's own PlaceType, got {:?}",
        text(&item, "PlaceType")
    );

    let scores = item
        .get("MatchScores")
        .and_then(|scores| scores.get("Components"))
        .and_then(|components| components.get("Address"))
        .cloned()
        .unwrap_or(Value::Null);
    for (component, expected) in [
        ("Country", 1.0),
        ("Region", 1.0),
        ("Locality", 1.0),
        // Never supplied, so never matched.
        ("SubRegion", 0.0),
        ("PostalCode", 0.0),
        ("AddressNumber", 0.0),
    ] {
        let actual = score(&scores, component);
        assert!(
            (actual - expected).abs() < 1e-9,
            "case structured: expected {component} to score {expected}, got {actual}"
        );
    }

    let parsed = parsed_address(&item);
    for (component, value) in [
        ("Country", "USA"),
        ("Region", "Washington"),
        ("Locality", "Seattle"),
    ] {
        let spans = parsed
            .get(component)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let span = spans.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            (
                text(&span, "Value"),
                text(&span, "QueryComponent"),
                span.get("StartIndex").and_then(Value::as_i64),
                span.get("EndIndex").and_then(Value::as_i64),
            ),
            (
                value.to_string(),
                component.to_string(),
                Some(0),
                Some(value.chars().count() as i64),
            ),
            "case structured: expected {component} echoed as the supplied {value:?}, \
             got {span:?}"
        );
    }
    assert!(
        parsed.get("District").is_none() && parsed.get("Street").is_none(),
        "case structured: a component the caller never supplied is not parsed, got {parsed:?}"
    );
}

/// A free-form query that names the PLACE rather than its address is parsed as the
/// title, and scores there: `MatchScores.Components.Title` and `ParsedQuery.Title`
/// are the title's own members, so a name match is not reported as an address one.
#[tokio::test]
async fn a_place_name_the_query_named_is_parsed_as_the_title() {
    let emulator = Emulator::new();
    for (case, query, expected) in [
        ("exact-name", "Pike Place Market", "Pike Place Market"),
        ("lower-cased-name", "space needle", "space needle"),
    ] {
        let response = emulator
            .post_ok(case, "/v2/geocode", &json!({"QueryText": query}))
            .await;
        let item = items(&response).first().cloned().unwrap_or(Value::Null);
        let parsed = item
            .get("ParsedQuery")
            .and_then(|parsed| parsed.get("Title"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let span = parsed.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            (
                text(&span, "Value"),
                text(&span, "QueryComponent"),
                span.get("StartIndex").and_then(Value::as_i64),
                span.get("EndIndex").and_then(Value::as_i64),
            ),
            (
                expected.to_string(),
                "Query".to_string(),
                Some(0),
                Some(expected.chars().count() as i64),
            ),
            "case {case}: expected the place's name parsed as the title, got {span:?}"
        );
        let title_score = item
            .get("MatchScores")
            .and_then(|scores| scores.get("Components"))
            .map_or(0.0, |components| score(components, "Title"));
        assert!(
            (title_score - 1.0).abs() < 1e-9,
            "case {case}: a name recognized in full scores 1, got {title_score}"
        );
    }
}

/// A result the query matched on something that is neither its name nor an address
/// component (a category) parses no component at all, rather than reporting an
/// invented span.
#[tokio::test]
async fn a_match_that_parses_nothing_reports_no_parsed_query() {
    let emulator = Emulator::new();
    let response = emulator
        .post_ok("category-only", "/v2/geocode", &json!({"QueryText": "Museum"}))
        .await;
    let returned = items(&response);
    assert!(
        !returned.is_empty(),
        "case category-only: the catalog holds a museum, so the query matches"
    );
    for item in returned {
        assert!(
            item.get("ParsedQuery").is_none(),
            "case category-only: nothing of the query names the place's title or \
             address, so there is nothing to echo, got {:?}",
            item.get("ParsedQuery")
        );
        assert!(
            item.get("MatchScores").is_some(),
            "case category-only: MatchScores is reported even so"
        );
    }
}

// ---------------------------------------------------------------------------
// Structured granularity
// ---------------------------------------------------------------------------

/// A structured query resolves at the granularity of the FINEST component it
/// supplies: every result carries the `PlaceType` that component names, and the
/// finer places inside the matched area are not returned.
///
/// The rows are every granularity the `GeocodeQueryComponents` members name, from
/// a country down to an address number, plus one the catalog holds no place at.
#[tokio::test]
async fn a_structured_query_resolves_at_its_finest_components_granularity() {
    let emulator = Emulator::new();
    let rows: Vec<(&str, Value, &[&str])> = vec![
        ("country", json!({"Country": "USA"}), &["Country"]),
        (
            "region",
            json!({"Country": "USA", "Region": "Washington"}),
            &["Region"],
        ),
        (
            "sub-region",
            json!({"Country": "USA", "SubRegion": "King County"}),
            &["SubRegion"],
        ),
        (
            "locality",
            json!({"Country": "USA", "Region": "WA", "Locality": "Seattle"}),
            &["Locality"],
        ),
        (
            "district",
            json!({"Locality": "Seattle", "District": "Capitol Hill"}),
            &["District"],
        ),
        (
            "street",
            json!({"Locality": "Seattle", "Street": "Broad St"}),
            &["Street", "PointAddress"],
        ),
        (
            "address-number",
            json!({"Locality": "Seattle", "Street": "Broad St", "AddressNumber": "400"}),
            &["PointAddress", "InterpolatedAddress", "SecondaryAddress"],
        ),
        // The catalog holds no place at postal-code granularity, so the honest
        // answer is no items rather than a place of another granularity.
        ("postal-code", json!({"PostalCode": "98109"}), &[]),
    ];

    for (case, components, place_types) in rows {
        let response = emulator
            .post_ok(
                case,
                "/v2/geocode",
                &json!({"QueryComponents": components, "MaxResults": 100}),
            )
            .await;
        let returned = items(&response);
        if place_types.is_empty() {
            assert!(
                returned.is_empty(),
                "case {case}: expected no items at a granularity the catalog holds no \
                 place at, got {:?}",
                titles(&response)
            );
            continue;
        }
        assert!(
            !returned.is_empty(),
            "case {case}: the catalog holds a place at this granularity"
        );
        for item in returned {
            let place_type = text(&item, "PlaceType");
            assert!(
                place_types.contains(&place_type.as_str()),
                "case {case}: expected a result of type {place_types:?}, got {place_type:?} \
                 ({:?})",
                text(&item, "Title")
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SearchText
// ---------------------------------------------------------------------------

/// SearchText answers with the places its `QueryText` names, and with no others: a
/// brand query returns that brand's places, not the neighbours of one.
///
/// The rows differ along what the query names (a brand, a landmark's name, a
/// category, a street written with the type spelled out, a street type alone, text
/// naming nothing) — and each row pins the WHOLE result set, so an unrelated place
/// in any slot fails.
#[tokio::test]
async fn search_text_answers_only_the_places_its_query_names() {
    let emulator = Emulator::new();
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            // The chain has several stores, and the query names the chain: every
            // store it names, and no neighbour of one. A chain store is titled
            // with the brand name the place data writes.
            "brand",
            "Starbucks",
            vec![
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS",
                "STARBUCKS RESERVE ROASTERY",
            ],
        ),
        ("name", "Space Needle", vec!["Space Needle"]),
        (
            // A category word is a query term: every place that IS a coffee shop,
            // in every city the catalog holds one in.
            "category",
            "Coffee",
            vec![
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
        ),
        (
            // "Street" is the written-out form of the catalog's "St".
            "street-spelled-out",
            "Pike Street Seattle",
            vec![
                "Pike Place Market",
                "STARBUCKS",
                "STARBUCKS RESERVE ROASTERY",
                "Seattle Coffee Works",
                "Victrola Coffee Roasters",
            ],
        ),
        (
            "street-and-number",
            "400 Broad Street Seattle",
            vec![
                "400 Broad St, Seattle, WA 98109, United States",
                "Space Needle",
            ],
        ),
        ("no-match", "Zzyzx Undersea Resort", vec![]),
    ];

    for (case, query, expected) in rows {
        let response = emulator
            .post_ok(
                case,
                "/v2/search-text",
                &json!({"QueryText": query, "MaxResults": 100}),
            )
            .await;
        let mut returned = titles(&response);
        returned.sort();
        let mut expected: Vec<String> = expected.iter().map(|title| (*title).to_string()).collect();
        expected.sort();
        assert_eq!(
            returned, expected,
            "case {case}: expected exactly the places {query:?} names, got {returned:?}"
        );
    }
}

/// A business chain's `Id` is reported as the catalog holds it, not case-folded: a
/// client filtering on `BusinessChains.Id` compares it as a string.
#[tokio::test]
async fn a_business_chain_keeps_the_identifier_the_catalog_holds() {
    let emulator = Emulator::new();
    let response = emulator
        .post_ok(
            "chain",
            "/v2/search-text",
            &json!({"QueryText": "Starbucks"}),
        )
        .await;
    let item = items(&response).first().cloned().unwrap_or(Value::Null);
    let chains = item
        .get("BusinessChains")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let chain = chains.first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        (text(&chain, "Id"), text(&chain, "Name")),
        ("Starbucks".to_string(), "Starbucks".to_string()),
        "case chain: expected the stored chain identifier and name, got {chain:?}"
    );
}

/// `Distance` is measured from the `BiasPosition`, and equally relevant results
/// come back nearest first. The rows are two different bias positions, so no single
/// ordering satisfies both.
#[tokio::test]
async fn search_text_measures_distance_from_the_bias_position() {
    let emulator = Emulator::new();
    for name in ["Space Needle", "Pike Place Market"] {
        let origin = place(&emulator, name, "PointOfInterest");
        let response = emulator
            .post_ok(
                name,
                "/v2/search-text",
                &json!({
                    "QueryText": "Seattle",
                    "BiasPosition": [origin.position[0], origin.position[1]],
                    "MaxResults": 100,
                }),
            )
            .await;
        let returned = items(&response);
        assert!(
            returned.len() > 1,
            "case {name}: the catalog holds several places in Seattle"
        );
        let distances: Vec<i64> = returned.iter().map(distance).collect();
        let scores: Vec<f64> = returned
            .iter()
            .map(|item| {
                item.get("MatchScores")
                    .map_or(1.0, |scores| score(scores, "Overall"))
            })
            .collect();
        for window in 1..returned.len() {
            if (scores[window] - scores[window - 1]).abs() < 1e-9 {
                assert!(
                    distances[window] >= distances[window - 1],
                    "case {name}: equally relevant results come back nearest first, got \
                     {distances:?}"
                );
            }
        }
        // The bias position is one of the results, so the nearest is zero metres
        // away and the farthest is not.
        assert_eq!(
            distances.iter().copied().min(),
            Some(0),
            "case {name}: the place at the bias position is zero metres from it, got \
             {distances:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Bounded queries
// ---------------------------------------------------------------------------

/// A free-form query is BOUNDED: one of the published maximum length is answered,
/// one character past it is refused with `ValidationException` and never matched,
/// so the matching work a single request can ask for has a fixed ceiling.
///
/// The rows are both operations that take a free-form query, each at the bound and
/// one past it.
#[tokio::test]
async fn a_free_form_query_past_the_published_ceiling_is_refused() {
    // `SearchTextRequest$QueryText` is modeled `len 1..200`; Geocode's `QueryText`
    // carries no length trait, so the service holds it to its sibling's ceiling.
    const QUERY_TEXT_MAX: usize = 200;

    let emulator = Emulator::new();
    for path in ["/v2/geocode", "/v2/search-text"] {
        for (case, length, answered) in [
            ("at-the-bound", QUERY_TEXT_MAX, true),
            ("past-the-bound", QUERY_TEXT_MAX + 1, false),
        ] {
            let case = format!("{path}-{case}");
            let query = "w ".repeat(length / 2 + 1)[..length].to_string();
            let call = emulator.post(path, &json!({"QueryText": query})).await;
            if answered {
                assert_eq!(
                    call.status, 200,
                    "case {case}: a query of {length} characters is inside the ceiling, \
                     got HTTP {}",
                    call.status
                );
                continue;
            }
            assert_eq!(
                call.status, 400,
                "case {case}: a query of {length} characters is past the ceiling, got \
                 HTTP {}",
                call.status
            );
            assert_eq!(
                call.error_code(),
                "ValidationException",
                "case {case}: expected ValidationException, got {:?}",
                call.error_code()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// PricingBucket
// ---------------------------------------------------------------------------
/// `PricingBucket` is a pricing TIER, and never the name of the operation that
/// served the request: a consumer attributing requests to a billing tier would
/// otherwise see a different "tier" per operation and never a tier change.
///
/// The rows are every implemented operation, each called twice: at the default
/// `IntendedUse` and at `Storage`, which the model documents as "charged at a
/// higher rate" and so a tier of its own.
#[tokio::test]
async fn the_pricing_bucket_is_a_tier_and_not_the_operation_name() {
    let emulator = Emulator::new();
    let needle = place(&emulator, "Space Needle", "PointOfInterest");
    let position = json!([needle.position[0], needle.position[1]]);
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "Geocode",
            "/v2/geocode",
            json!({"QueryText": "Space Needle"}),
        ),
        (
            "SearchText",
            "/v2/search-text",
            json!({"QueryText": "Space Needle"}),
        ),
        (
            "ReverseGeocode",
            "/v2/reverse-geocode",
            json!({"QueryPosition": position.clone()}),
        ),
        (
            "SearchNearby",
            "/v2/search-nearby",
            json!({"QueryPosition": position.clone(), "QueryRadius": 2000}),
        ),
    ];

    for (operation, path, body) in rows {
        for (case, intended_use, expected) in [
            ("default", None, PRICING_BUCKET_CORE),
            ("storage", Some("Storage"), PRICING_BUCKET_STORAGE),
        ] {
            let mut request = body.clone();
            if let (Some(members), Some(intended_use)) = (request.as_object_mut(), intended_use) {
                members.insert("IntendedUse".to_string(), json!(intended_use));
            }
            let case = format!("{operation}-{case}");
            let call = emulator.post(path, &request).await;
            assert_success(&case, &call);
            let bucket = call.header(PRICING_BUCKET_HEADER);
            assert_eq!(
                bucket, expected,
                "case {case}: expected the {expected:?} pricing tier, got {bucket:?}"
            );
            assert_ne!(
                bucket, operation,
                "case {case}: the pricing bucket must not be the operation name"
            );
        }
    }

    // GetPlace is a GET taking its parameters in the query string
    // (`intended-use`), so it is driven apart from the POST rows above, under the
    // same rule.
    for (case, query, expected) in [
        ("GetPlace-default", "", PRICING_BUCKET_CORE),
        (
            "GetPlace-storage",
            "?intended-use=Storage",
            PRICING_BUCKET_STORAGE,
        ),
    ] {
        let call = emulator
            .get(&format!("/v2/place/{}{query}", needle.place_id))
            .await;
        assert_success(case, &call);
        let bucket = call.header(PRICING_BUCKET_HEADER);
        assert_eq!(
            bucket, expected,
            "case {case}: expected the {expected:?} pricing tier, got {bucket:?}"
        );
    }
}

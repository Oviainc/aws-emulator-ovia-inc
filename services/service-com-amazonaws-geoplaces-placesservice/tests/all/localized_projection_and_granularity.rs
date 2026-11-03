// SPDX-License-Identifier: CC0-1.0

//! How a place RECORD is projected into a result item: the naming of the address
//! components, the language the display text is rendered in, and the granularity a
//! queried position resolves at.
//!
//! Three rules, each a table whose rows differ along what the rule generalises over
//! rather than along one recorded request:
//!
//! 1. `Address.SubRegion.Name` is "name for the county or sub-region" — the area's
//!    NAME, with no administrative division word, at every level and in every
//!    operation that reports an address. The division word belongs to the `Title`
//!    and `Label` of the place that IS that sub-region.
//! 2. Result text is rendered in the language the request's `Language` names when
//!    the entry holds text in it, and in the entry's own language otherwise ("if
//!    there is no data for the result in the requested language, data will be
//!    returned in the default language for the entry").
//! 3. A queried position resolves to the places that cover it, FINEST FIRST, so
//!    ReverseGeocode's documented default of one result is the most specific place
//!    containing the point.

use serde_json::{json, Value};

use crate::places_catalog::{distance, items, text, Emulator};
use crate::result_item_composition::address;

/// The operations that report an `Address` out of the catalog, as
/// `(case, path, body)` — a request each one answers over the Seattle area, where
/// the catalog holds a county.
fn seattle_requests() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "geocode-free-text",
            "/v2/geocode",
            json!({"QueryText": "Space Needle", "MaxResults": 10}),
        ),
        (
            "geocode-structured",
            "/v2/geocode",
            json!({"QueryComponents": {"Region": "WA", "Locality": "Seattle"}}),
        ),
        (
            "search-text",
            "/v2/search-text",
            json!({"QueryText": "Seattle", "MaxResults": 10}),
        ),
        (
            "search-nearby",
            "/v2/search-nearby",
            json!({"QueryPosition": [-122.349_358, 47.620_506], "QueryRadius": 20_000}),
        ),
        (
            "reverse-geocode",
            "/v2/reverse-geocode",
            json!({"QueryPosition": [-122.349_358, 47.620_506], "MaxResults": 100}),
        ),
    ]
}

/// A nested string member of an address, e.g. `SubRegion`/`Name`.
fn group_member(item: &Value, group: &str, member: &str) -> String {
    address(item)
        .get(group)
        .map(|group| text(group, member))
        .unwrap_or_default()
}

/// The division words a US sub-region's display name may carry: the words a county
/// equivalent is written with, which `Address.SubRegion.Name` must NOT contain.
///
/// SOURCE: the catalog's own sub-region division words (`county()` /
/// `bare_sub_region()` in `crate::state`), which is what a display title appends.
const DIVISION_WORDS: [&str; 3] = ["County", "Parish", "Borough"];

/// `Address.SubRegion.Name` carries the sub-region's NAME alone — never the
/// administrative division word a display title writes beside it — in EVERY
/// operation that projects an address, and the place that IS the sub-region is
/// still titled and labelled with that word.
///
/// The rows are the five implemented operations that report an address (free-form
/// and structured geocode, text search, nearby search, reverse geocode), crossed
/// with every result each one returns, plus every sub-region record the catalog
/// holds read back through GetPlace.
#[tokio::test]
async fn a_sub_region_component_is_the_bare_name_and_only_a_title_carries_the_division() {
    let emulator = Emulator::new();
    let mut checked = 0_usize;
    for (case, path, body) in seattle_requests() {
        let returned = items(&emulator.post_ok(case, path, &body).await);
        assert!(
            !returned.is_empty(),
            "case {case}: the catalog answers this request, or the rule is never reached"
        );
        for item in returned {
            let name = group_member(&item, "SubRegion", "Name");
            if name.is_empty() {
                continue;
            }
            checked += 1;
            for word in DIVISION_WORDS {
                assert!(
                    !name.split_whitespace().any(|part| part == word),
                    "case {case}: SubRegion.Name is the county's name alone, so it must \
                     not carry {word:?}; got {name:?} for {:?}",
                    text(&item, "Title")
                );
            }
        }
    }
    assert!(
        checked > 0,
        "case fixture: no result carried a SubRegion, so the table never tested the rule"
    );

    // The place that IS a sub-region keeps the division word in the text that
    // NAMES it, while its own component stays the bare name.
    let sub_regions: Vec<_> = emulator
        .catalog()
        .into_iter()
        .filter(|place| place.place_type == "SubRegion")
        .collect();
    assert!(
        sub_regions.len() > 1,
        "case fixture: the catalog must seed more than one sub-region, got {:?}",
        sub_regions.len()
    );
    for place in sub_regions {
        let detail = emulator
            .get_ok(&place.name, &format!("/v2/place/{}", place.place_id))
            .await;
        let name = group_member(&detail, "SubRegion", "Name");
        assert!(
            text(&detail, "Title").starts_with(&place.name)
                && text(&address(&detail), "Label").starts_with(&place.name),
            "case {}: the sub-region's own Title and Label name it in full, got {:?} / {:?}",
            place.name,
            text(&detail, "Title"),
            text(&address(&detail), "Label")
        );
        assert!(
            place.name.starts_with(&name) && !name.is_empty(),
            "case {}: the SubRegion component must be the bare name inside the display \
             name, got {name:?}",
            place.name
        );
    }
}

/// A row of the language table: the `Language` asked for, and the language tag the
/// text must come back in.
struct LanguageRow {
    case: &'static str,
    /// `Language`, a `LanguageTag` (`len 2..35`), or `None` for a request that
    /// asks for none.
    asked: Option<&'static str>,
    /// The tag of the language the result text must be rendered in.
    rendered: &'static str,
}

/// The text of the Crimean raion the recorded fixture resolves to, by language
/// tag: `(Title, Address.Country.Name)`.
///
/// SOURCE: the catalog's own entries for that territory (`crate::state`'s seeded
/// records), which hold the raion's name in Ukrainian, Russian and English.
fn pervomaiske_text(tag: &str) -> (&'static str, &'static str) {
    match tag {
        "uk" => ("Первомайський район, Україна", "Україна"),
        "en" => ("Pervomaiske Raion, Ukraine", "Ukraine"),
        other => panic!("case oracle: the catalog holds no text in {other:?}"),
    }
}

/// `Title` is "the localized display name of this result item based on request
/// parameter language", and `Language` is "a list of BCP 47 compliant language
/// codes for the results to be rendered in. If there is no data for the result in
/// the requested language, data will be returned in the default language for the
/// entry" — so the text of a result follows the request's language where the entry
/// has it, and the entry's own language where it has not.
///
/// The rows differ along the TAG asked for: a tag the entry holds, one it does not,
/// a tag with a region subtag (RFC 4647 lookup drops it), a differently cased tag,
/// and no tag at all. Every row is asserted on the operations that render text, and
/// the reported `StreetComponents.Language` must name the language the text is
/// actually in.
#[tokio::test]
async fn result_text_is_rendered_in_the_requested_language_or_the_entry_s_own() {
    /// The recorded fixture's position: a point in the Crimean raion the catalog
    /// holds in Ukrainian, Russian and English.
    const FIXTURE: [f64; 2] = [33.859_39, 45.714_2];

    let emulator = Emulator::new();
    let rows = [
        LanguageRow {
            case: "no-language-falls-back-to-the-entry",
            asked: None,
            rendered: "uk",
        },
        LanguageRow {
            case: "the-entry-s-own-language",
            asked: Some("uk"),
            rendered: "uk",
        },
        LanguageRow {
            case: "another-language-the-entry-holds",
            asked: Some("en"),
            rendered: "en",
        },
        LanguageRow {
            case: "a-regional-variant-of-a-held-language",
            asked: Some("en-GB"),
            rendered: "en",
        },
        LanguageRow {
            case: "a-tag-cased-differently",
            asked: Some("EN"),
            rendered: "en",
        },
        LanguageRow {
            case: "a-language-the-entry-does-not-hold",
            asked: Some("ja"),
            rendered: "uk",
        },
        LanguageRow {
            case: "an-unheld-language-with-a-region-subtag",
            asked: Some("zh-Hans-CN"),
            rendered: "uk",
        },
    ];

    for row in rows {
        let mut request = json!({"QueryPosition": [FIXTURE[0], FIXTURE[1]]});
        if let Some(asked) = row.asked {
            request["Language"] = json!(asked);
        }
        let body = emulator
            .post_ok(row.case, "/v2/reverse-geocode", &request)
            .await;
        let item = items(&body).first().cloned().unwrap_or(Value::Null);
        let (title, country) = pervomaiske_text(row.rendered);
        assert_eq!(
            (
                text(&item, "Title"),
                text(&address(&item), "Label"),
                group_member(&item, "Country", "Name"),
            ),
            (title.to_string(), title.to_string(), country.to_string()),
            "case {}: expected the result rendered in {:?}, got {item:?}",
            row.case,
            row.rendered
        );

        // The same language selection on the other operations that render text,
        // over a place whose own name is localized (a point of interest) and one
        // whose name is its assembled label.
        for (operation, path, mut body) in [
            (
                "Geocode",
                "/v2/geocode",
                json!({"QueryText": "Eiffel Tower", "MaxResults": 1}),
            ),
            (
                "SearchText",
                "/v2/search-text",
                json!({"QueryText": "Eiffel Tower", "MaxResults": 1}),
            ),
        ] {
            if let Some(asked) = row.asked {
                body["Language"] = json!(asked);
            }
            let case = format!("{}-{operation}", row.case);
            let returned = emulator.post_ok(&case, path, &body).await;
            let item = items(&returned).first().cloned().unwrap_or(Value::Null);
            // The entry's own language is French. An English request renames it;
            // every other tag asked for falls back to the French name. The request
            // that asks for NO language is answered under the name its query used —
            // the rule `a_place_found_by_the_name_one_language_gives_it_is_named_in
            // _that_language` covers, exercised here only as the row that asks for
            // nothing.
            let expected = if row.rendered == "en" || row.asked.is_none() {
                "Eiffel Tower"
            } else {
                "Tour Eiffel"
            };
            assert_eq!(
                text(&item, "Title"),
                expected,
                "case {case}: expected the place's name in the rendered language, got {:?}",
                text(&item, "Title")
            );
            let language = address(&item)
                .get("StreetComponents")
                .and_then(Value::as_array)
                .and_then(|components| components.first().cloned())
                .map(|component| text(&component, "Language"))
                .unwrap_or_default();
            // `StreetComponents$Language` names the language of the TEXT it
            // accompanies, not the one the request asked for: this entry's street
            // is held in French alone, so it stays French even where the place's
            // own name is answered in English.
            assert_eq!(
                language, "fr",
                "case {case}: the reported street-component Language must name the \
                 language the street text is in, got {language:?}"
            );
        }
    }
}

/// A row of the granularity table: a position, and the `PlaceType` of the single
/// place the service must resolve it to.
struct GranularityRow {
    case: &'static str,
    /// `QueryPosition`, `[longitude, latitude]`.
    at: [f64; 2],
    /// The `PlaceType` of the finest place covering that position.
    finest: &'static str,
}

/// A queried position resolves to the places that COVER it, most SPECIFIC first,
/// so the documented default of one result ("Default value: 1") is the finest place
/// containing the point and not an arbitrary level above it — and that place's own
/// address components are the ones a client renders.
///
/// The rows differ along the LEVEL the finest covering place sits at: a point of
/// interest, a city district, a settlement, a county-level territory and a country,
/// in three countries. Every level coarser than the finest must also come back
/// (every one of them contains the point), and must come back after it.
#[tokio::test]
async fn a_position_resolves_to_the_finest_place_that_covers_it() {
    use service_com_amazonaws_geoplaces_placesservice::state::granularity_rank;

    let emulator = Emulator::new();
    let rows = [
        GranularityRow {
            case: "point-of-interest",
            at: [-122.349_358, 47.620_506],
            finest: "PointOfInterest",
        },
        GranularityRow {
            case: "city-district",
            at: [-122.312_000, 47.623_000],
            finest: "District",
        },
        GranularityRow {
            case: "settlement",
            at: [33.958_76, 45.468_24],
            finest: "Locality",
        },
        GranularityRow {
            // Inside the county-level territory's extent and outside every
            // settlement the catalog holds in it.
            case: "county-level-territory",
            at: [34.600_0, 45.100_0],
            finest: "SubRegion",
        },
        GranularityRow {
            // Inland western France: inside the country's own bounding box and
            // inside no region, county or settlement the catalog holds.
            case: "country",
            at: [-1.500_0, 47.200_0],
            finest: "Country",
        },
    ];

    for row in rows {
        let body = emulator
            .post_ok(
                row.case,
                "/v2/reverse-geocode",
                &json!({"QueryPosition": [row.at[0], row.at[1]], "MaxResults": 100}),
            )
            .await;
        let returned = items(&body);
        let ranks: Vec<(String, usize)> = returned
            .iter()
            .map(|item| {
                let place_type = text(item, "PlaceType");
                (place_type.clone(), granularity_rank(&place_type))
            })
            .collect();
        assert_eq!(
            ranks.first().map(|(place_type, _)| place_type.as_str()),
            Some(row.finest),
            "case {}: expected the finest place covering the position first, got {ranks:?}",
            row.case
        );
        assert!(
            ranks.windows(2).all(|pair| pair[0].1 >= pair[1].1),
            "case {}: the places covering a position come back finest first, got {ranks:?}",
            row.case
        );
        for item in &returned {
            assert_eq!(
                distance(item),
                0,
                "case {}: every place covering the position is zero metres from it, got \
                 {} for {:?}",
                row.case,
                distance(item),
                text(item, "Title")
            );
        }

        // The default page is ONE item ("Default value: 1"), and it is that finest
        // place — the whole point of the ranking.
        let defaulted = emulator
            .post_ok(
                row.case,
                "/v2/reverse-geocode",
                &json!({"QueryPosition": [row.at[0], row.at[1]]}),
            )
            .await;
        let single = items(&defaulted);
        assert_eq!(
            single.len(),
            1,
            "case {}: ReverseGeocode defaults to one result, got {}",
            row.case,
            single.len()
        );
        assert_eq!(
            text(&single[0], "PlaceType"),
            row.finest,
            "case {}: the default single result is the finest place covering the \
             position, got {:?}",
            row.case,
            text(&single[0], "Title")
        );
    }
}

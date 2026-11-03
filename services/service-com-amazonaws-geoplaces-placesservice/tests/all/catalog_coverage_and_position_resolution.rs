// SPDX-License-Identifier: CC0-1.0

//! What the Places operations must be able to ANSWER, and how they refuse.
//!
//! Five rules, each a table whose rows differ along what the rule generalises over
//! rather than along one recorded request:
//!
//! 1. A query naming a landmark or an address is answered out of the catalog, in
//!    every country and region it covers, and the `PlaceId` it hands out resolves
//!    through GetPlace to the same place (rows: landmarks in four countries, two of
//!    them named in a language other than the record's).
//! 2. A result set is SELECTED by the filters the model declares — country,
//!    place type, bounding box, radius — so each filter has both matches to keep
//!    and matches to drop (rows: the homonym pairs, every `PlaceType` the catalog
//!    holds plus types it does not, several boxes, several radius ladders).
//! 3. `MaxResults` fills the page when the catalog holds that many matches.
//! 4. A position resolves to the places that COVER it, and to nothing else — a
//!    position no place covers answers with an empty list (rows: landmark
//!    positions on three continents, a disputed territory, open ocean).
//! 5. A `PoliticalView` is accepted for any country, echoed in alpha-3 form, and
//!    reattributes the places a claim is unresolved over; every refusal names the
//!    member at fault and the rule it broke (rows: the five validation paths the
//!    service words, plus a bound at each end).

use serde_json::{json, Value};

use crate::places_catalog::{assert_success, items, place_ids, position, text, Call, Emulator};
use crate::result_item_composition::{address, titles};

/// A nested string member of a result item's address, e.g. `Country`/`Code3`.
fn address_part(item: &Value, group: &str, member: &str) -> String {
    address(item)
        .get(group)
        .map(|group| text(group, member))
        .unwrap_or_default()
}

/// `MapView`, as `[west, south, east, north]`.
fn map_view(item: &Value) -> Vec<f64> {
    item.get("MapView")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// The message a refusal carries. `ValidationException$Message` is bound to the
/// JSON member `message` (`jsonName`), which is also where a refusal the framework
/// made before any handler ran puts it.
fn message(call: &Call) -> String {
    text(&call.json(), "message")
}

// ---------------------------------------------------------------------------
// 1. Coverage: the places these operations are expected to answer over
// ---------------------------------------------------------------------------

/// A row of the landmark table: a free-form query, and the place it must resolve
/// to.
struct LandmarkRow {
    case: &'static str,
    query: &'static str,
    /// The `Title` the service must report: a point of interest's own name, or the
    /// assembled label of a place that has no name of its own.
    title: &'static str,
    /// The `Title` GetPlace must report for the SAME record. A `PlaceId` names no
    /// place by name and this row asks for no `Language`, so a place queried by the
    /// name another language gives it is read back under the entry's own name.
    detail_title: &'static str,
    /// `Address.Country.Code3` of that place.
    country: &'static str,
}

/// A query naming a place is answered out of the catalog wherever that place is,
/// and the `PlaceId` it hands out resolves through GetPlace to the same record —
/// the chain a client doing "geocode, then fetch" walks.
///
/// The rows differ along the COUNTRY and region the place sits in, and along the
/// name it is queried by: two of them are queried by a name the record does not
/// carry as its title but is known by (`Eiffel Tower` for `Tour Eiffel`).
#[tokio::test]
async fn a_query_naming_a_place_resolves_to_it_and_round_trips_through_get_place() {
    let emulator = Emulator::new();
    let rows = [
        LandmarkRow {
            case: "golden-gate-bridge",
            query: "Golden Gate Bridge, San Francisco, CA, USA",
            title: "Golden Gate Bridge",
            detail_title: "Golden Gate Bridge",
            country: "USA",
        },
        LandmarkRow {
            case: "statue-of-liberty",
            query: "Statue of Liberty, New York",
            title: "Statue of Liberty",
            detail_title: "Statue of Liberty",
            country: "USA",
        },
        LandmarkRow {
            case: "empire-state-building",
            query: "Empire State Building",
            title: "Empire State Building",
            detail_title: "Empire State Building",
            country: "USA",
        },
        LandmarkRow {
            case: "space-needle",
            query: "Space Needle",
            title: "Space Needle",
            detail_title: "Space Needle",
            country: "USA",
        },
        LandmarkRow {
            case: "pier-39",
            query: "Pier 39, San Francisco",
            title: "Pier 39",
            detail_title: "Pier 39",
            country: "USA",
        },
        LandmarkRow {
            // Queried by its English name, which the record holds beside its own
            // French one: with no `Language` asked for, the result is named the way
            // the query named it, while the record itself keeps its own name.
            case: "eiffel-tower-by-english-name",
            query: "Eiffel Tower",
            title: "Eiffel Tower",
            detail_title: "Tour Eiffel",
            country: "FRA",
        },
        LandmarkRow {
            case: "louvre-by-english-name",
            query: "Louvre Museum",
            title: "Louvre Museum",
            detail_title: "Musee du Louvre",
            country: "FRA",
        },
        LandmarkRow {
            // A place with no name of its own: its `Title` is the assembled label.
            // Queried by its English name, which the entry holds beside its own
            // text: a place that has no name of its own has no name to answer
            // under, so the label comes back in the entry's own language, "the
            // default language for the entry".
            case: "simferopol",
            query: "Simferopol",
            title: "Сімферополь, Україна",
            detail_title: "Сімферополь, Україна",
            country: "UKR",
        },
    ];

    for row in rows {
        let geocoded = emulator
            .post_ok(
                row.case,
                "/v2/geocode",
                &json!({"QueryText": row.query, "MaxResults": 1}),
            )
            .await;
        let found = items(&geocoded);
        let item = found.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            text(&item, "Title"),
            row.title,
            "case {}: expected the query to resolve to {:?}, got {:?}",
            row.case,
            row.title,
            titles(&geocoded)
        );
        assert_eq!(
            address_part(&item, "Country", "Code3"),
            row.country,
            "case {}: expected the place in {}, got {:?}",
            row.case,
            row.country,
            address(&item)
        );

        // The same query through SearchText finds the same place.
        let searched = emulator
            .post_ok(
                row.case,
                "/v2/search-text",
                &json!({"QueryText": row.query, "MaxResults": 5}),
            )
            .await;
        assert!(
            place_ids(&searched).contains(&text(&item, "PlaceId")),
            "case {}: SearchText must find the place Geocode returned, got {:?}",
            row.case,
            titles(&searched)
        );

        // The round trip: the id resolves to the same record.
        let detail = emulator
            .get_ok(row.case, &format!("/v2/place/{}", text(&item, "PlaceId")))
            .await;
        assert_eq!(
            (text(&detail, "Title"), position(&detail)),
            (row.detail_title.to_string(), position(&item)),
            "case {}: GetPlace must report the same record at the same Position, got {:?}",
            row.case,
            (text(&detail, "Title"), position(&detail))
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Selection: a filter has both matches to keep and matches to drop
// ---------------------------------------------------------------------------

/// A row of the country-filter table: a query, the countries it is restricted to,
/// and the `Code3` every surviving item must carry.
struct CountryRow {
    case: &'static str,
    path: &'static str,
    query: &'static str,
    /// `Filter.IncludeCountries`, in the alpha-2 or alpha-3 form the member takes.
    countries: &'static [&'static str],
    /// `Address.Country.Code3` every item must carry, or `None` when the filter
    /// leaves no match at all.
    expected: Option<&'static str>,
}

/// `Filter.IncludeCountries` DISAMBIGUATES a name several countries hold: the
/// filtered page holds that country's match, not an empty page.
///
/// The rows differ along the country asked for (each form of the code), along the
/// operation, and along what the query names (a homonymous locality, a category);
/// a country the catalog holds no match in returns no items rather than a match
/// from elsewhere.
#[tokio::test]
async fn a_country_filter_selects_the_homonym_in_that_country() {
    let emulator = Emulator::new();

    // Unfiltered, the better-known homonym leads.
    let unfiltered = emulator
        .post_ok(
            "unfiltered",
            "/v2/geocode",
            &json!({"QueryText": "Paris", "MaxResults": 5}),
        )
        .await;
    let first = items(&unfiltered).first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        address_part(&first, "Country", "Code3"),
        "FRA",
        "case unfiltered: expected Paris, France to lead an unfiltered query, got {:?}",
        titles(&unfiltered)
    );

    let rows = [
        CountryRow {
            case: "geocode-paris-usa-alpha3",
            path: "/v2/geocode",
            query: "Paris",
            countries: &["USA"],
            expected: Some("USA"),
        },
        CountryRow {
            case: "geocode-paris-us-alpha2",
            path: "/v2/geocode",
            query: "Paris",
            countries: &["US"],
            expected: Some("USA"),
        },
        CountryRow {
            case: "geocode-paris-fra",
            path: "/v2/geocode",
            query: "Paris",
            countries: &["FRA"],
            expected: Some("FRA"),
        },
        CountryRow {
            case: "search-text-paris-usa",
            path: "/v2/search-text",
            query: "Paris",
            countries: &["USA"],
            expected: Some("USA"),
        },
        CountryRow {
            case: "search-text-coffee-fra",
            path: "/v2/search-text",
            query: "coffee",
            countries: &["FRA"],
            expected: Some("FRA"),
        },
        CountryRow {
            case: "search-text-coffee-usa",
            path: "/v2/search-text",
            query: "coffee",
            countries: &["USA"],
            expected: Some("USA"),
        },
        CountryRow {
            // A country the catalog holds no Paris in: no items, never a match
            // from another country.
            case: "geocode-paris-jpn",
            path: "/v2/geocode",
            query: "Paris",
            countries: &["JPN"],
            expected: None,
        },
    ];

    for row in rows {
        let body = emulator
            .post_ok(
                row.case,
                row.path,
                &json!({
                    "QueryText": row.query,
                    "MaxResults": 100,
                    "Filter": {"IncludeCountries": row.countries},
                }),
            )
            .await;
        let returned = items(&body);
        match row.expected {
            None => assert!(
                returned.is_empty(),
                "case {}: a country with no match must answer with no items, got {:?}",
                row.case,
                titles(&body)
            ),
            Some(country) => {
                assert!(
                    !returned.is_empty(),
                    "case {}: expected the {country} match, got no items",
                    row.case
                );
                for item in returned {
                    assert_eq!(
                        address_part(&item, "Country", "Code3"),
                        country,
                        "case {}: every filtered item must be in {country}, got {:?}",
                        row.case,
                        text(&item, "Title")
                    );
                }
            }
        }
    }
}

/// Every `PlaceType` the catalog holds a record at is REPORTED, every one of them
/// that `Filter.IncludePlaceTypes` can name is selectable through it, the filtered
/// page is exactly the unfiltered page's items of that type, and a type the catalog
/// holds no record at answers with no items.
///
/// The rows are the `GeocodeFilterPlaceType` values — the enum the member's own
/// list targets, which is a SUBSET of `PlaceType` — crossed with queries that match
/// places at several levels at once. An unfiltered query for an ambiguous name
/// answers at MORE than one level, so a client branching on `PlaceType` sees the
/// administrative hierarchy and not one slice of it.
#[tokio::test]
async fn every_place_type_the_catalog_holds_is_selectable_and_reported() {
    // `GeocodeFilterPlaceType`, the enum `GeocodeFilter$IncludePlaceTypes` targets,
    // split into the types this catalog seeds a record at and the types it does not.
    const SELECTABLE_AND_SEEDED: [&str; 5] = [
        "Country",
        "Region",
        "Locality",
        "PointAddress",
        "PointOfInterest",
    ];
    const SELECTABLE_AND_NOT_SEEDED: [&str; 5] = [
        "PostalCode",
        "Intersection",
        "Street",
        "InterpolatedAddress",
        "SecondaryAddress",
    ];
    /// Levels the catalog seeds and `PlaceType` names, which the geocode filter's
    /// own enum cannot name: they are REPORTED, not selected.
    const SEEDED_AND_NOT_SELECTABLE: [&str; 2] = ["SubRegion", "District"];

    let emulator = Emulator::new();
    let mut reported: Vec<String> = Vec::new();
    for query in ["Washington", "Seattle", "New York", "France"] {
        let unfiltered = emulator
            .post_ok(
                query,
                "/v2/geocode",
                &json!({"QueryText": query, "MaxResults": 100}),
            )
            .await;
        let all = items(&unfiltered);
        reported.extend(all.iter().map(|item| text(item, "PlaceType")));

        for place_type in SELECTABLE_AND_SEEDED {
            let case = format!("{query}-{place_type}");
            let expected: Vec<String> = all
                .iter()
                .filter(|item| text(item, "PlaceType") == place_type)
                .map(|item| text(item, "PlaceId"))
                .collect();
            let body = emulator
                .post_ok(
                    &case,
                    "/v2/geocode",
                    &json!({
                        "QueryText": query,
                        "MaxResults": 100,
                        "Filter": {"IncludePlaceTypes": [place_type]},
                    }),
                )
                .await;
            assert_eq!(
                place_ids(&body),
                expected,
                "case {case}: the filtered page must be exactly the unfiltered items of \
                 that type, got {:?}",
                titles(&body)
            );
            for item in items(&body) {
                assert_eq!(
                    text(&item, "PlaceType"),
                    place_type,
                    "case {case}: every item must carry the requested PlaceType, got {:?}",
                    text(&item, "Title")
                );
            }
        }

        for place_type in SELECTABLE_AND_NOT_SEEDED {
            let case = format!("{query}-{place_type}");
            let body = emulator
                .post_ok(
                    &case,
                    "/v2/geocode",
                    &json!({
                        "QueryText": query,
                        "MaxResults": 100,
                        "Filter": {"IncludePlaceTypes": [place_type]},
                    }),
                )
                .await;
            assert!(
                items(&body).is_empty(),
                "case {case}: the catalog holds no place of this type, so the honest \
                 answer is no items, got {:?}",
                titles(&body)
            );
        }
    }

    for place_type in SELECTABLE_AND_SEEDED
        .into_iter()
        .chain(SEEDED_AND_NOT_SELECTABLE)
    {
        assert!(
            reported.iter().any(|seen| seen == place_type),
            "case reported-{place_type}: the catalog must answer with a place of every \
             level it seeds, got {reported:?}"
        );
    }

    // An ambiguous name answers at several levels at once: the city, the state it
    // shares its name with, and the places inside them.
    let ambiguous = emulator
        .post_ok(
            "ambiguous-name",
            "/v2/geocode",
            &json!({"QueryText": "Washington", "MaxResults": 5}),
        )
        .await;
    let types: Vec<String> = items(&ambiguous)
        .iter()
        .map(|item| text(item, "PlaceType"))
        .collect();
    for expected in ["Region", "Locality"] {
        assert!(
            types.iter().any(|place_type| place_type == expected),
            "case ambiguous-name: expected a {expected} among the first results for an \
             ambiguous name, got {types:?}"
        );
    }
}

/// A row of the bounding-box table: a box, and the category word searched inside
/// it.
struct BoxRow {
    case: &'static str,
    /// `Filter.BoundingBox`, `[west, south, east, north]`.
    area: [f64; 4],
    query: &'static str,
    /// How many places the box must hold for this query.
    expected: usize,
}

/// `Filter.BoundingBox` keeps exactly the matches whose `Position` lies inside the
/// box: the same query answers with different places in different boxes, and with
/// none where the box holds none.
///
/// The rows differ along the box (a downtown, a neighbourhood, another country, open
/// water) and along the query, and each is cross-checked against the unfiltered
/// result set filtered by the box itself.
#[tokio::test]
async fn a_bounding_box_keeps_the_matches_inside_it() {
    let emulator = Emulator::new();
    let rows = [
        BoxRow {
            case: "downtown-seattle-coffee",
            area: [-122.36, 47.60, -122.33, 47.62],
            query: "coffee",
            expected: 4,
        },
        BoxRow {
            case: "capitol-hill-coffee",
            area: [-122.33, 47.60, -122.30, 47.63],
            query: "coffee",
            expected: 2,
        },
        BoxRow {
            case: "central-paris-coffee",
            area: [2.25, 48.83, 2.40, 48.88],
            query: "coffee",
            expected: 1,
        },
        BoxRow {
            case: "downtown-seattle-tourist-attraction",
            area: [-122.36, 47.60, -122.33, 47.62],
            query: "tourist attraction",
            expected: 1,
        },
        BoxRow {
            case: "open-water-coffee",
            area: [-40.0, -40.0, -30.0, -30.0],
            query: "coffee",
            expected: 0,
        },
    ];

    for row in rows {
        // What the query matches anywhere, so the box can be checked against the
        // set it is selecting from.
        let everywhere = emulator
            .post_ok(
                row.case,
                "/v2/search-text",
                &json!({"QueryText": row.query, "MaxResults": 100}),
            )
            .await;
        let inside: Vec<String> = items(&everywhere)
            .iter()
            .filter(|item| {
                let place = position(item);
                place.len() == 2
                    && place[0] >= row.area[0]
                    && place[0] <= row.area[2]
                    && place[1] >= row.area[1]
                    && place[1] <= row.area[3]
            })
            .map(|item| text(item, "PlaceId"))
            .collect();

        let body = emulator
            .post_ok(
                row.case,
                "/v2/search-text",
                &json!({
                    "QueryText": row.query,
                    "MaxResults": 100,
                    "Filter": {"BoundingBox": row.area},
                }),
            )
            .await;
        let returned = place_ids(&body);
        assert_eq!(
            returned.len(),
            row.expected,
            "case {}: expected {} place(s) inside the box, got {:?}",
            row.case,
            row.expected,
            titles(&body)
        );
        assert_eq!(
            returned, inside,
            "case {}: the box must keep exactly the matches inside it, got {:?}",
            row.case,
            titles(&body)
        );
    }
}

/// A row of the radius table: an anchor position, and the radii to compare.
struct RadiusRow {
    case: &'static str,
    anchor: [f64; 2],
    /// Radii in metres, narrowest first (`QueryRadius`, `range 1..21000000`).
    ladder: &'static [i64],
    /// How many places each radius of the ladder must reach: the density the
    /// catalog has to hold around this anchor for a radius to select at all.
    expected: &'static [usize],
}

/// `QueryRadius` SELECTS between the places around a position: a wider radius
/// keeps every place a narrower one returned and reaches further ones, and every
/// reported `Distance` is inside the radius asked for.
///
/// The rows differ along the anchor (a dense city block, a city centre, another
/// country) and along the scale of the ladder, so no single result set satisfies
/// them: the catalog has to hold records at varied distances around each anchor.
#[tokio::test]
async fn widening_the_query_radius_reaches_more_places() {
    let emulator = Emulator::new();
    let rows = [
        RadiusRow {
            // Around the Space Needle, metres apart.
            case: "space-needle-block",
            anchor: [-122.3493, 47.6205],
            ladder: &[20, 40, 85, 2_000],
            expected: &[2, 3, 5, 12],
        },
        RadiusRow {
            case: "seattle-city",
            anchor: [-122.330_062, 47.603_832],
            ladder: &[100, 2_000, 20_000],
            expected: &[2, 8, 16],
        },
        RadiusRow {
            case: "paris-city",
            anchor: [2.352_222, 48.856_613],
            ladder: &[500, 2_000, 10_000],
            expected: &[1, 3, 4],
        },
    ];

    for row in rows {
        let mut previous: Vec<String> = Vec::new();
        let mut counts: Vec<usize> = Vec::new();
        for radius in row.ladder {
            let case = format!("{}-radius-{radius}", row.case);
            let body = emulator
                .post_ok(
                    &case,
                    "/v2/search-nearby",
                    &json!({
                        "QueryPosition": [row.anchor[0], row.anchor[1]],
                        "QueryRadius": radius,
                        "MaxResults": 100,
                    }),
                )
                .await;
            let returned = place_ids(&body);
            for item in items(&body) {
                let reported = item.get("Distance").and_then(Value::as_i64).unwrap_or(0);
                assert!(
                    reported <= *radius,
                    "case {case}: every result must lie inside the radius, got {reported} m \
                     for {:?}",
                    text(&item, "Title")
                );
            }
            for kept in &previous {
                assert!(
                    returned.contains(kept),
                    "case {case}: a wider radius must keep every place a narrower one \
                     returned, got {:?}",
                    titles(&body)
                );
            }
            counts.push(returned.len());
            previous = returned;
        }
        assert!(
            counts.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {}: a wider radius cannot return fewer places, got {counts:?}",
            row.case
        );
        assert_eq!(
            counts,
            row.expected.to_vec(),
            "case {}: expected the radii {:?} to reach {:?} places, got {counts:?}",
            row.case,
            row.ladder,
            row.expected
        );
        assert!(
            counts.windows(2).all(|pair| pair[1] > pair[0]),
            "case {}: every widening step must reach a further place, got {counts:?}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Page size
// ---------------------------------------------------------------------------

/// `MaxResults` — "an optional limit for the number of results returned in a
/// single call" — is a LIMIT: a page holds as many matches as the catalog has, up
/// to the limit asked for, and the capped page is the prefix of the uncapped one.
///
/// The rows differ along the query (a business chain, a category, a city) and along
/// the operation, so the rule is not read off one result set; each query matches
/// several places, which is what makes a limit observable at all.
#[tokio::test]
async fn max_results_fills_the_page_with_as_many_matches_as_there_are() {
    let emulator = Emulator::new();
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "chain-biased",
            "/v2/search-text",
            json!({"QueryText": "Starbucks", "BiasPosition": [-122.3493, 47.6205]}),
        ),
        (
            "category",
            "/v2/search-text",
            json!({"QueryText": "coffee"}),
        ),
        (
            "city",
            "/v2/geocode",
            json!({"QueryText": "Seattle"}),
        ),
        (
            "nearby",
            "/v2/search-nearby",
            json!({"QueryPosition": [-122.330_062, 47.603_832], "QueryRadius": 20_000}),
        ),
    ];

    for (case, path, body) in rows {
        let mut uncapped = body.clone();
        uncapped["MaxResults"] = json!(100);
        let whole = emulator.post_ok(case, path, &uncapped).await;
        let all = place_ids(&whole);
        assert!(
            all.len() >= 3,
            "case {case}: this query must match at least three places for a page size to \
             be observable, got {:?}",
            titles(&whole)
        );

        for max_results in [1_usize, 2, 3, all.len(), all.len() + 1] {
            let case = format!("{case}-max-results-{max_results}");
            let mut capped = body.clone();
            capped["MaxResults"] = json!(max_results);
            let page = emulator.post_ok(&case, path, &capped).await;
            let returned = place_ids(&page);
            assert_eq!(
                returned.len(),
                max_results.min(all.len()),
                "case {case}: expected a page of {} items, got {:?}",
                max_results.min(all.len()),
                titles(&page)
            );
            assert_eq!(
                returned,
                all[..returned.len()].to_vec(),
                "case {case}: a capped page must be the prefix of the uncapped result"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Position resolution
// ---------------------------------------------------------------------------

/// A row of the reverse-geocode table: a position, and what it must resolve to.
struct PositionRow {
    case: &'static str,
    /// `QueryPosition`, `[longitude, latitude]`.
    at: [f64; 2],
    /// `(Title, Locality, Region.Code, Country.Code2)` of the place the position
    /// must resolve to, or `None` when no place covers it.
    expected: Option<(&'static str, &'static str, &'static str, &'static str)>,
}

/// ReverseGeocode answers out of the places that COVER the queried position: every
/// returned place's own extent (`MapView`) contains the query, and a position no
/// place covers answers with an empty `ResultItems` list rather than the nearest
/// record the catalog happens to hold.
///
/// The rows differ along where the position is (landmarks on two continents, a city
/// street, a disputed territory, open ocean at two latitudes), so a result that
/// ignored the position would answer at least one of them with an address in
/// another country.
#[tokio::test]
async fn a_position_resolves_only_to_the_places_that_cover_it() {
    let emulator = Emulator::new();
    let rows = [
        PositionRow {
            case: "statue-of-liberty",
            at: [-74.0445, 40.6892],
            expected: Some(("Statue of Liberty", "New York", "NY", "US")),
        },
        PositionRow {
            case: "space-needle",
            at: [-122.349_358, 47.620_506],
            expected: Some(("Space Needle", "Seattle", "WA", "US")),
        },
        PositionRow {
            case: "golden-gate-bridge",
            at: [-122.478_255, 37.819_929],
            expected: Some(("Golden Gate Bridge", "San Francisco", "CA", "US")),
        },
        PositionRow {
            case: "eiffel-tower",
            at: [2.294_481, 48.858_370],
            expected: Some(("Tour Eiffel", "Paris", "IDF", "FR")),
        },
        PositionRow {
            // Inland Crimea: the rural district covering it is the finest place
            // that does, named in the entry's own language.
            case: "crimea",
            at: [33.958_76, 45.468_24],
            expected: Some(("Первомайський район, Україна", "Первомайський район", "43", "UA")),
        },
        PositionRow {
            case: "south-atlantic",
            at: [-30.0, -40.0],
            expected: None,
        },
        PositionRow {
            case: "antarctic-ocean",
            at: [0.0, -85.0],
            expected: None,
        },
    ];

    for row in rows {
        let body = emulator
            .post_ok(
                row.case,
                "/v2/reverse-geocode",
                &json!({"QueryPosition": [row.at[0], row.at[1]], "MaxResults": 1}),
            )
            .await;
        let returned = items(&body);
        let Some((title, locality, region, country)) = row.expected else {
            assert!(
                returned.is_empty(),
                "case {}: a position no place covers must answer with no items, got {:?}",
                row.case,
                titles(&body)
            );
            continue;
        };
        let item = returned.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            (
                text(&item, "Title"),
                text(&address(&item), "Locality"),
                address_part(&item, "Region", "Code"),
                address_part(&item, "Country", "Code2"),
            ),
            (
                title.to_string(),
                locality.to_string(),
                region.to_string(),
                country.to_string(),
            ),
            "case {}: expected the place at the queried position, got {:?}",
            row.case,
            address(&item)
        );
        // The rule itself: the place REPORTED covers the position asked about.
        let view = map_view(&item);
        assert!(
            view.len() == 4
                && row.at[0] >= view[0]
                && row.at[0] <= view[2]
                && row.at[1] >= view[1]
                && row.at[1] <= view[3],
            "case {}: the resolved place's own extent must contain the queried \
             position, got MapView {view:?} for {:?}",
            row.case,
            row.at
        );
    }
}

// ---------------------------------------------------------------------------
// 5. PoliticalView, and how a refusal is worded
// ---------------------------------------------------------------------------

/// A `PoliticalView` is accepted for ANY country the modeled `CountryCode` names —
/// the member is a point of view on the answer, not a place in the catalog — and is
/// echoed on every result item in the alpha-3 form the items model
/// (`CountryCode3`, `pattern ^[A-Z]{3}$`), whichever accepted form it was asked
/// with.
///
/// The rows are both accepted forms of several countries, including ones the
/// catalog holds no place in at all.
#[tokio::test]
async fn any_country_is_an_accepted_political_view_and_is_echoed_in_alpha_three() {
    let emulator = Emulator::new();
    for (case, requested, expected) in [
        ("alpha-2-us", "US", "USA"),
        ("alpha-3-us", "USA", "USA"),
        ("alpha-2-fr", "FR", "FRA"),
        ("alpha-2-ru", "RU", "RUS"),
        ("alpha-3-ru", "RUS", "RUS"),
        ("alpha-2-ua", "UA", "UKR"),
        ("alpha-3-ua", "UKR", "UKR"),
        // Countries the catalog holds no place in: a political view is still a
        // point of view the request may take.
        ("alpha-3-jp", "JPN", "JPN"),
        ("alpha-2-jp", "JP", "JPN"),
        ("alpha-3-in", "IND", "IND"),
        ("alpha-2-ma", "MA", "MAR"),
    ] {
        for (operation, path, body) in [
            (
                "Geocode",
                "/v2/geocode",
                json!({"QueryText": "Space Needle", "PoliticalView": requested}),
            ),
            (
                "SearchText",
                "/v2/search-text",
                json!({"QueryText": "Space Needle", "PoliticalView": requested}),
            ),
            (
                "ReverseGeocode",
                "/v2/reverse-geocode",
                json!({
                    "QueryPosition": [-122.349_358, 47.620_506],
                    "MaxResults": 1,
                    "PoliticalView": requested,
                }),
            ),
            (
                "SearchNearby",
                "/v2/search-nearby",
                json!({
                    "QueryPosition": [-122.349_358, 47.620_506],
                    "QueryRadius": 100,
                    "PoliticalView": requested,
                }),
            ),
        ] {
            let case = format!("{case}-{operation}");
            let response = emulator.post_ok(&case, path, &body).await;
            let returned = items(&response);
            assert!(
                !returned.is_empty(),
                "case {case}: a political view must not empty the result set"
            );
            for item in returned {
                assert_eq!(
                    text(&item, "PoliticalView"),
                    expected,
                    "case {case}: expected the view echoed as {expected}, got {:?}",
                    text(&item, "PoliticalView")
                );
            }
        }
    }
}

/// A row of the political-attribution table: the view asked for, and the country
/// the disputed position is attributed to under it.
struct AttributionRow {
    case: &'static str,
    /// `PoliticalView`, or `None` for a request that asks for no view.
    view: Option<&'static str>,
    /// `Address.Country.Code2` and `Country.Name` the result must carry.
    country: (&'static str, &'static str),
}

/// A `PoliticalView` REATTRIBUTES the places a territorial claim is unresolved
/// over — "to represent unresolved territorial claims through the point of view of
/// the specified country" — and leaves every other place as it is.
///
/// The rows are the views that claim the territory and one that does not, plus the
/// same views applied to an undisputed place, which must answer alike under all of
/// them. Each attribution's country is named in that attribution's OWN language,
/// because no `Language` is asked for and the entry's own is the fallback.
#[tokio::test]
async fn a_political_view_reattributes_only_the_disputed_places() {
    /// A position inland in Crimea, which the catalog holds a region record for.
    const DISPUTED: [f64; 2] = [33.958_76, 45.468_24];
    /// An undisputed position: no view changes what covers it.
    const UNDISPUTED: [f64; 2] = [-122.349_358, 47.620_506];

    let emulator = Emulator::new();
    let rows = [
        AttributionRow {
            case: "no-view",
            view: None,
            country: ("UA", "Україна"),
        },
        AttributionRow {
            case: "ukrainian-view-alpha3",
            view: Some("UKR"),
            country: ("UA", "Україна"),
        },
        AttributionRow {
            case: "ukrainian-view-alpha2",
            view: Some("UA"),
            country: ("UA", "Україна"),
        },
        AttributionRow {
            case: "russian-view-alpha3",
            view: Some("RUS"),
            country: ("RU", "Россия"),
        },
        AttributionRow {
            case: "russian-view-alpha2",
            view: Some("RU"),
            country: ("RU", "Россия"),
        },
        AttributionRow {
            // A third country's view takes no side the record knows of, so the
            // default attribution stands.
            case: "japanese-view",
            view: Some("JPN"),
            country: ("UA", "Україна"),
        },
    ];

    for row in rows {
        for (label, at, expected) in [
            ("disputed", DISPUTED, row.country),
            ("undisputed", UNDISPUTED, ("US", "United States")),
        ] {
            let case = format!("{}-{label}", row.case);
            let mut body = json!({"QueryPosition": [at[0], at[1]], "MaxResults": 1});
            if let Some(view) = row.view {
                body["PoliticalView"] = json!(view);
            }
            let response = emulator.post_ok(&case, "/v2/reverse-geocode", &body).await;
            let item = items(&response).first().cloned().unwrap_or(Value::Null);
            assert_eq!(
                (
                    address_part(&item, "Country", "Code2"),
                    address_part(&item, "Country", "Name"),
                ),
                (expected.0.to_string(), expected.1.to_string()),
                "case {case}: expected the position attributed to {:?}, got {:?}",
                expected,
                address(&item)
            );
            assert!(
                text(&address(&item), "Label").ends_with(expected.1),
                "case {case}: the assembled label must name the attributed country, \
                 got {:?}",
                text(&address(&item), "Label")
            );
        }
    }
}

/// A row of the refusal table: a request outside the contract, and the sentence the
/// service words that refusal with.
struct MessageRow {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    /// The expected `message`: the single-quoted member at fault and the rule it
    /// broke, or the alternatives a request had to satisfy one of.
    expected: &'static str,
}

/// Every refusal is the modeled `ValidationException` (HTTP 400) worded the way
/// geo-places words one: the single-quoted name of the member at fault followed by
/// the rule it broke, derived from the request and the member's modeled constraint
/// — never the generic constraint-validator sentence, and never per-call-site
/// prose.
///
/// The rows span the constraint KINDS and both the paths a refusal is found on:
/// inside a handler (an unresolvable id, an unusable cursor, a rule over several
/// members) and at the boundary before any handler runs (a `@pattern`, both ends of
/// a `@range`, a `@length`, an absent `@required` member).
#[tokio::test]
async fn a_refusal_names_the_member_at_fault_and_the_rule_it_broke() {
    let emulator = Emulator::new();
    let rows = vec![
        MessageRow {
            case: "geocode-no-query",
            method: "POST",
            path: "/v2/geocode",
            body: Some(json!({"MaxResults": 5})),
            expected: "At least one of the following fields must be set: 'QueryText', \
                       'QueryComponents'.",
        },
        MessageRow {
            case: "search-text-no-query",
            method: "POST",
            path: "/v2/search-text",
            body: Some(json!({"MaxResults": 5})),
            expected: "At least one of the following fields must be set: 'QueryText', \
                       'QueryId'.",
        },
        MessageRow {
            case: "get-place-unknown-id",
            method: "GET",
            path: "/v2/place/NOTAPLACEIDTHISSERVICEEVERISSUED00000000",
            body: None,
            expected: "'PlaceId' must be a valid ID.",
        },
        MessageRow {
            case: "search-nearby-garbage-next-token",
            method: "POST",
            path: "/v2/search-nearby",
            body: Some(json!({
                "QueryPosition": [-122.330_062, 47.603_832],
                "NextToken": "not-a-real-next-token",
            })),
            expected: "'NextToken' must be a valid token.",
        },
        MessageRow {
            case: "search-text-garbage-next-token",
            method: "POST",
            path: "/v2/search-text",
            body: Some(json!({"QueryText": "Seattle", "NextToken": "not-a-real-next-token"})),
            expected: "'NextToken' must be a valid token.",
        },
        MessageRow {
            // `PoliticalView` is `CountryCode`, `@pattern ^([A-Z]{2}|[A-Z]{3})$`.
            case: "geocode-political-view-lowercase",
            method: "POST",
            path: "/v2/geocode",
            body: Some(json!({"QueryText": "Seattle, WA", "PoliticalView": "us"})),
            expected: "'PoliticalView' must match regex ^([A-Z]{2}|[A-Z]{3})$",
        },
        MessageRow {
            // `QueryRadius` is `DistanceMeters`, `@range 1..21000000`.
            case: "reverse-geocode-radius-below-range",
            method: "POST",
            path: "/v2/reverse-geocode",
            body: Some(json!({
                "QueryPosition": [-122.3493, 47.6205],
                "QueryRadius": 0,
            })),
            expected: "'QueryRadius' must not be less than 1",
        },
        MessageRow {
            case: "reverse-geocode-radius-above-range",
            method: "POST",
            path: "/v2/reverse-geocode",
            body: Some(json!({
                "QueryPosition": [-122.3493, 47.6205],
                "QueryRadius": 21_000_001_i64,
            })),
            expected: "'QueryRadius' must not be greater than 21000000",
        },
        MessageRow {
            // `MaxResults` is `@range 1..100`.
            case: "geocode-max-results-below-range",
            method: "POST",
            path: "/v2/geocode",
            body: Some(json!({"QueryText": "Seattle", "MaxResults": 0})),
            expected: "'MaxResults' must not be less than 1",
        },
        MessageRow {
            case: "geocode-max-results-above-range",
            method: "POST",
            path: "/v2/geocode",
            body: Some(json!({"QueryText": "Seattle", "MaxResults": 101})),
            expected: "'MaxResults' must not be greater than 100",
        },
        MessageRow {
            // `Language` is `LanguageTag`, `@length 2..35`.
            case: "geocode-language-too-short",
            method: "POST",
            path: "/v2/geocode",
            body: Some(json!({"QueryText": "Seattle", "Language": "e"})),
            expected: "'Language' must have a length of at least 2",
        },
        MessageRow {
            // `QueryPosition` is `Position`, `@length 2..2`.
            case: "reverse-geocode-one-element-position",
            method: "POST",
            path: "/v2/reverse-geocode",
            body: Some(json!({"QueryPosition": [-122.3493]})),
            expected: "'QueryPosition' must have a length of at least 2",
        },
        MessageRow {
            case: "reverse-geocode-three-element-position",
            method: "POST",
            path: "/v2/reverse-geocode",
            body: Some(json!({"QueryPosition": [-122.3493, 47.6205, 10.0]})),
            expected: "'QueryPosition' must have a length of at most 2",
        },
        MessageRow {
            // `QueryPosition` is `@required`.
            case: "search-nearby-without-position",
            method: "POST",
            path: "/v2/search-nearby",
            body: Some(json!({"QueryRadius": 100})),
            expected: "'QueryPosition' must not be null.",
        },
        MessageRow {
            // Well formed against the pattern, but not a country ISO 3166-1
            // assigns, so no alpha-3 form of it exists to echo.
            case: "geocode-political-view-not-a-country",
            method: "POST",
            path: "/v2/geocode",
            body: Some(json!({"QueryText": "Seattle", "PoliticalView": "ZZ"})),
            expected: "'PoliticalView' must be an ISO 3166-1 alpha-2 or alpha-3 country code.",
        },
    ];

    for row in rows {
        let call = match row.method {
            "GET" => emulator.get(row.path).await,
            _ => {
                emulator
                    .post(row.path, row.body.as_ref().unwrap_or(&Value::Null))
                    .await
            }
        };
        assert_eq!(
            call.status, 400,
            "case {}: expected HTTP 400, got {} with {}",
            row.case,
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {}: expected the modeled ValidationException, got {:?}",
            row.case,
            call.error_code()
        );
        assert_eq!(
            message(&call),
            row.expected,
            "case {}: expected the message {:?}, got {:?}",
            row.case,
            row.expected,
            message(&call)
        );
    }

    // A request inside the contract is still answered, so the table above is not
    // refusing everything.
    let call = emulator
        .post(
            "/v2/reverse-geocode",
            &json!({"QueryPosition": [-122.3493, 47.6205], "QueryRadius": 1}),
        )
        .await;
    assert_success("control-inside-the-contract", &call);
}

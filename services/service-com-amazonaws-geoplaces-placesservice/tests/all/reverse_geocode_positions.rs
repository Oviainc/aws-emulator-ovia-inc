// SPDX-License-Identifier: CC0-1.0

//! ReverseGeocode answers from the QUERIED POSITION, and honours the political view
//! the request asks for.
//!
//! Every test here is a TABLE whose rows differ along what the rules generalise
//! over: the position queried (five continents, both hemispheres, inside and
//! outside the catalog's coverage, on land and in open water), the `QueryRadius`,
//! and the form and country of the `PoliticalView` asked for. The position of the
//! recorded fixture this issue was filed from is one row among them.
//!
//! The expected country of each position is its ISO 3166-1 alpha-2/alpha-3 pair —
//! the same standard `Address$Country$Code2`/`Code3` model (`pattern ^[A-Z]{2}$` /
//! `^[A-Z]{3}$`).

use serde_json::{json, Value};

use crate::places_catalog::{assert_success, distance, items, position, text, Emulator};

/// The path ReverseGeocode is bound to (`@http(POST /v2/reverse-geocode)`).
const REVERSE_GEOCODE: &str = "/v2/reverse-geocode";

/// A nested string member, e.g. `Address.Country.Code2`.
fn nested(item: &Value, path: &str) -> String {
    let mut value = item.clone();
    for member in path.split('.') {
        value = value.get(member).cloned().unwrap_or(Value::Null);
    }
    value.as_str().unwrap_or_default().to_string()
}

/// `MapView`, as the modeled `BoundingBox` orders it:
/// `[west, south, east, north]`.
fn bounding_box(item: &Value) -> [f64; 4] {
    let values: Vec<f64> = item
        .get("MapView")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    match values.as_slice() {
        [west, south, east, north] => [*west, *south, *east, *north],
        other => panic!("MapView must be four coordinates, got {other:?}"),
    }
}

/// Is `[longitude, latitude]` inside `[west, south, east, north]`?
///
/// An independent oracle for the service's own coverage rule, deliberately kept
/// simple: it does NOT judge a box crossing the antimeridian, and asserts it is
/// never given one, so no row of the tables below can be judged wrongly.
fn box_contains(box_: [f64; 4], position: [f64; 2]) -> bool {
    let [west, south, east, north] = box_;
    let [longitude, latitude] = position;
    assert!(
        west <= east,
        "case oracle: this oracle does not judge a box crossing the antimeridian, \
         got {box_:?}"
    );
    longitude >= west && longitude <= east && latitude >= south && latitude <= north
}

/// A row of the position table: a `QueryPosition` and the country it lies in.
struct PositionRow {
    case: &'static str,
    /// `[longitude, latitude]`, as the modeled `Position` orders them.
    query: [f64; 2],
    /// ISO 3166-1 alpha-2 and alpha-3 of the country the position lies in.
    country: (&'static str, &'static str),
}

/// Positions the seeded catalog covers, across its countries and granularities.
fn position_rows() -> Vec<PositionRow> {
    vec![
        // The position of the recorded fixture this issue was filed from: a point
        // in Crimea, which the reference data attributes to Ukraine.
        PositionRow {
            case: "crimea",
            query: [33.859_39, 45.714_2],
            country: ("UA", "UKR"),
        },
        PositionRow {
            case: "simferopol",
            query: [34.102_417, 44.952_117],
            country: ("UA", "UKR"),
        },
        PositionRow {
            case: "seattle",
            query: [-122.330_062, 47.603_832],
            country: ("US", "USA"),
        },
        PositionRow {
            case: "washington-dc",
            query: [-77.036_560, 38.897_957],
            country: ("US", "USA"),
        },
        PositionRow {
            case: "paris",
            query: [2.352_222, 48.856_613],
            country: ("FR", "FRA"),
        },
    ]
}

/// A reverse geocode reports the place at the QUERIED POSITION: a result whose own
/// coverage contains the point, the country of that point, and `Distance` zero
/// because the point is inside the area the result covers.
#[tokio::test]
async fn reverse_geocode_resolves_the_queried_position() {
    let emulator = Emulator::new();
    for row in position_rows() {
        let body = emulator
            .post_ok(
                row.case,
                REVERSE_GEOCODE,
                &json!({"QueryPosition": [row.query[0], row.query[1]], "MaxResults": 100}),
            )
            .await;
        let returned = items(&body);
        assert!(
            !returned.is_empty(),
            "case {}: the catalog covers this position, so it must resolve",
            row.case
        );
        for item in &returned {
            assert_eq!(
                nested(item, "Address.Country.Code2"),
                row.country.0,
                "case {}: expected the country of the queried position, got {item:?}",
                row.case
            );
            assert_eq!(
                nested(item, "Address.Country.Code3"),
                row.country.1,
                "case {}: expected the alpha-3 country of the queried position, got \
                 {item:?}",
                row.case
            );
            assert!(
                !text(item, "PlaceId").is_empty()
                    && !text(item, "PlaceType").is_empty()
                    && !text(item, "Title").is_empty()
                    && position(item).len() == 2,
                "case {}: an item must carry PlaceId, PlaceType, Title and Position, \
                 got {item:?}",
                row.case
            );

            // `MapView` is modeled as the box enclosing the area a result covers,
            // and a reverse geocode answers with what covers the queried point.
            let map_view = bounding_box(item);
            assert!(
                box_contains(map_view, row.query),
                "case {}: the result's MapView must enclose the queried position; \
                 {map_view:?} does not contain {:?} for {:?}",
                row.case,
                row.query,
                text(item, "Title")
            );
            assert_eq!(
                distance(item),
                0,
                "case {}: a result whose own area contains the queried position is \
                 zero metres from it, got {} at {:?}",
                row.case,
                distance(item),
                text(item, "Title")
            );
        }
    }
}

/// A position the service holds no place data for is answered with an EMPTY
/// `ResultItems` (modeled `len 0..100`) — never with a place somewhere else.
///
/// The rows are open water and ice in three oceans and both hemispheres.
#[tokio::test]
async fn a_position_with_no_place_data_returns_no_items() {
    let emulator = Emulator::new();
    for (case, query) in [
        ("south-pacific", [-140.0, -30.0]),
        ("south-atlantic", [-30.0, -35.0]),
        ("southern-indian-ocean", [80.0, -45.0]),
        ("antarctic-interior", [0.0, -85.0]),
        ("north-pacific", [-160.0, 35.0]),
    ] {
        let case = format!("{case}-no-radius");
        let call = emulator
            .post(
                REVERSE_GEOCODE,
                &json!({"QueryPosition": [query[0], query[1]], "MaxResults": 100}),
            )
            .await;
        assert_success(&case, &call);
        let returned: Vec<(String, i64)> = items(&call.json())
            .iter()
            .map(|item| (text(item, "Title"), distance(item)))
            .collect();
        assert!(
            returned.is_empty(),
            "case {case}: the service holds no place data at this position, so it \
             must report none at all; got {returned:?}"
        );
    }
}

/// `Distance` zero and a result covering the queried position mean each other: a
/// result is zero metres away exactly when its own `MapView` encloses the point,
/// and a result that does not cover the point is reported only inside the
/// `QueryRadius` the caller allowed, at its real distance.
///
/// The rows are positions at every remove from a seeded place — a city centre, a
/// rural position tens of kilometres away, remote desert, taiga and ice — each
/// asked with no radius and with radii from the bottom of the modeled range up,
/// which is where a result "at" a position it does not cover would show up.
#[tokio::test]
async fn a_zero_distance_means_the_result_covers_the_queried_position() {
    let emulator = Emulator::new();
    let positions = [
        ("fixture-crimea", [33.859_39, 45.714_2]),
        ("city-centre", [-122.330_062, 47.603_832]),
        ("rural-ukraine", [33.300_0, 46.000_0]),
        ("rural-kansas", [-100.000_0, 38.500_0]),
        ("outback-australia", [132.000_0, -24.500_0]),
        ("sahara", [10.000_0, 25.000_0]),
        ("siberian-taiga", [95.000_0, 62.000_0]),
        ("greenland-ice", [-40.000_0, 72.000_0]),
    ];
    // Results that came back for a position their own coverage does NOT contain:
    // the band this rule is about, asserted non-empty below so the table cannot
    // pass by never reaching it.
    let mut non_covering = 0_usize;
    for (case, query) in positions {
        for radius in [None, Some(1_i64), Some(1_000), Some(60_000), Some(21_000_000)] {
            let case = format!("{case}-radius-{radius:?}");
            let mut request = json!({"QueryPosition": [query[0], query[1]], "MaxResults": 100});
            if let Some(radius) = radius {
                request["QueryRadius"] = json!(radius);
            }
            let body = emulator.post_ok(&case, REVERSE_GEOCODE, &request).await;
            for item in items(&body) {
                let covers = box_contains(bounding_box(&item), query);
                if !covers {
                    non_covering += 1;
                }
                assert_eq!(
                    distance(&item) == 0,
                    covers,
                    "case {case}: Distance {} and a MapView {:?} that {} the queried \
                     position {query:?} disagree for {:?}",
                    distance(&item),
                    bounding_box(&item),
                    if covers { "encloses" } else { "does not enclose" },
                    text(&item, "Title")
                );
                if let Some(radius) = radius {
                    assert!(
                        distance(&item) <= radius,
                        "case {case}: a result outside the radius must not be reported; \
                         got {} m for {:?}",
                        distance(&item),
                        text(&item, "Title")
                    );
                } else {
                    assert!(
                        covers,
                        "case {case}: with no radius only a result covering the queried \
                         position may be reported; got {:?}",
                        text(&item, "Title")
                    );
                }
            }
        }
    }
    assert!(
        non_covering > 0,
        "case fixture: the table must reach results that do not cover the position \
         they were asked about, or it never tests the rule"
    );
}

/// A `PoliticalView` whose value satisfies the member's modeled constraints
/// (`CountryCode`, `len 2..3`, `pattern ^([A-Z]{2}|[A-Z]{3})$`, a country ISO
/// 3166-1 assigns) is ACCEPTED — never refused with HTTP 400 — and comes back on
/// every result item in the alpha-3 form the items model (`CountryCode3`,
/// `pattern ^[A-Z]{3}$`), whichever accepted form it was asked with.
///
/// The rows are both accepted forms of countries across the ISO 3166-1 register,
/// against positions in two countries.
#[tokio::test]
async fn an_accepted_political_view_is_echoed_in_alpha_three_form() {
    let emulator = Emulator::new();
    let views = [
        ("RU", "RUS"),
        ("RUS", "RUS"),
        ("UA", "UKR"),
        ("MAR", "MAR"),
        ("AR", "ARG"),
        ("JPN", "JPN"),
        ("IN", "IND"),
        ("FR", "FRA"),
    ];
    for query in [[33.859_39, 45.714_2], [-122.330_062, 47.603_832]] {
        for (asked, expected) in views {
            let case = format!("{query:?}-view-{asked}");
            let body = emulator
                .post_ok(
                    &case,
                    REVERSE_GEOCODE,
                    &json!({
                        "QueryPosition": [query[0], query[1]],
                        "PoliticalView": asked,
                        "MaxResults": 100,
                    }),
                )
                .await;
            let returned = items(&body);
            assert!(
                !returned.is_empty(),
                "case {case}: a political view must not empty the result set"
            );
            for item in returned {
                assert_eq!(
                    text(&item, "PoliticalView"),
                    expected,
                    "case {case}: expected the political view in alpha-3 form"
                );
            }
        }

        // With no political view asked for, no result reports one.
        let case = format!("{query:?}-view-absent");
        let body = emulator
            .post_ok(
                &case,
                REVERSE_GEOCODE,
                &json!({"QueryPosition": [query[0], query[1]], "MaxResults": 100}),
            )
            .await;
        for item in items(&body) {
            assert!(
                item.get("PoliticalView").is_none(),
                "case {case}: no political view was asked for, so none must be \
                 reported: {item:?}"
            );
        }
    }
}

/// The alpha-2 country the first result for `query` is attributed to, under `view`
/// when one is asked for.
async fn attributed_country(
    emulator: &Emulator,
    case: &str,
    query: [f64; 2],
    view: Option<&str>,
) -> String {
    let mut request = json!({"QueryPosition": [query[0], query[1]]});
    if let Some(view) = view {
        request["PoliticalView"] = json!(view);
    }
    let body = emulator.post_ok(case, REVERSE_GEOCODE, &request).await;
    let returned = items(&body);
    assert!(
        !returned.is_empty(),
        "case {case}: the service must hold place data for this position"
    );
    nested(&returned[0], "Address.Country.Code2")
}

/// The political view applies to the RESULTS: a territory the viewing country
/// claims comes back as part of that country, while every other view leaves the
/// attribution the reference data records — which is what the parameter is for
/// ("to represent unresolved territorial claims through the point of view of the
/// specified country", `ReverseGeocodeRequest$PoliticalView`).
///
/// The rows are the claimed territory (at the recorded fixture's own position and
/// at a second position inside it) and a position in another country entirely —
/// each under no view, the claimant's view and a view that claims nothing.
#[tokio::test]
async fn a_political_view_attributes_a_claimed_territory_to_the_claimant() {
    let emulator = Emulator::new();
    for (case, query, default_code2, claimed_code2) in [
        ("fixture-position", [33.859_39, 45.714_2], "UA", "RU"),
        ("simferopol", [34.102_417, 44.952_117], "UA", "RU"),
        // Another country entirely: no view re-attributes it.
        ("seattle", [-122.330_062, 47.603_832], "US", "US"),
    ] {
        assert_eq!(
            attributed_country(&emulator, case, query, None).await,
            default_code2,
            "case {case}: with no political view the recorded attribution stands"
        );
        assert_eq!(
            attributed_country(&emulator, case, query, Some("RUS")).await,
            claimed_code2,
            "case {case}: the claimant's own political view reports a territory it \
             claims as part of that country, and changes nothing elsewhere"
        );
        assert_eq!(
            attributed_country(&emulator, case, query, Some("FRA")).await,
            default_code2,
            "case {case}: a political view that claims nothing here changes nothing"
        );
    }
}

/// A `Position` outside the World Geodetic System range the model documents ("The
/// position in World Geodetic System (WGS 84) format") is REFUSED as client input
/// (HTTP 400, `ValidationException` — the operations' only modelled client-input
/// error) rather than resolved or ranked against a meaningless coordinate.
///
/// The rows differ along the way a position leaves the range (past each bound of
/// longitude and latitude, and absurdly far out) and along the MEMBER it arrives
/// on, because every `Position` the model carries is documented as WGS 84: a
/// `QueryPosition`, a `BiasPosition` and a filter circle's centre alike. Each
/// published bound itself is in range and is not refused.
#[tokio::test]
async fn a_position_outside_the_wgs84_range_is_refused() {
    let emulator = Emulator::new();
    for (case, query) in [
        ("longitude-past-east", [180.000_001_f64, 10.0]),
        ("longitude-past-west", [-180.000_001, 10.0]),
        ("latitude-past-north", [10.0, 90.000_001]),
        ("latitude-past-south", [10.0, -90.000_001]),
        ("longitude-far-out", [1.0e9, 10.0]),
    ] {
        let requests: Vec<(&str, &str, Value)> = vec![
            (
                "reverse-geocode-query-position",
                REVERSE_GEOCODE,
                json!({"QueryPosition": [query[0], query[1]]}),
            ),
            (
                "search-nearby-query-position",
                "/v2/search-nearby",
                json!({"QueryPosition": [query[0], query[1]], "QueryRadius": 1000}),
            ),
            (
                "geocode-bias-position",
                "/v2/geocode",
                json!({"QueryText": "Seattle", "BiasPosition": [query[0], query[1]]}),
            ),
            (
                "search-text-bias-position",
                "/v2/search-text",
                json!({"QueryText": "Seattle", "BiasPosition": [query[0], query[1]]}),
            ),
            (
                "search-text-filter-circle-center",
                "/v2/search-text",
                json!({
                    "QueryText": "Seattle",
                    "Filter": {"Circle": {"Center": [query[0], query[1]], "Radius": 1000}},
                }),
            ),
        ];
        for (member, path, request) in requests {
            let case = format!("{case}-{member}");
            let call = emulator.post(path, &request).await;
            assert_eq!(
                call.status, 400,
                "case {case}: a position outside the WGS 84 range is client input the \
                 service refuses, got {} with {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ValidationException",
                "case {case}: expected the modelled ValidationException, got {:?}",
                call.error_code()
            );
        }
    }

    for (case, query) in [
        ("longitude-at-east", [180.0_f64, 0.0]),
        ("longitude-at-west", [-180.0, 0.0]),
        ("latitude-at-north", [0.0, 90.0]),
        ("latitude-at-south", [0.0, -90.0]),
    ] {
        let call = emulator
            .post(
                REVERSE_GEOCODE,
                &json!({"QueryPosition": [query[0], query[1]]}),
            )
            .await;
        assert_eq!(
            call.status, 200,
            "case {case}: a position ON the published bound is in range, got {} with {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
    }
}

/// A `PlaceId` a reverse geocode hands out resolves through GetPlace to the same
/// place — and GetPlace renders it in the political view ITS request asks for.
#[tokio::test]
async fn a_reverse_geocoded_place_id_resolves_through_get_place() {
    let emulator = Emulator::new();
    for row in position_rows() {
        let body = emulator
            .post_ok(
                row.case,
                REVERSE_GEOCODE,
                &json!({"QueryPosition": [row.query[0], row.query[1]]}),
            )
            .await;
        let item = items(&body)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {}: the position must resolve", row.case));
        let place_id = text(&item, "PlaceId");

        let detail = emulator
            .get_ok(row.case, &format!("/v2/place/{place_id}"))
            .await;
        for member in ["PlaceId", "PlaceType", "Title"] {
            assert_eq!(
                text(&detail, member),
                text(&item, member),
                "case {}: GetPlace must report the same {member} as the reverse geocode",
                row.case
            );
        }
        for member in ["Address.Country.Code2", "Address.Locality", "Address.Label"] {
            assert_eq!(
                nested(&detail, member),
                nested(&item, member),
                "case {}: GetPlace must report the same {member} as the reverse geocode",
                row.case
            );
        }
    }

    // The political view is the reading request's own: the same id, read under a
    // claimant's view, is attributed to that claimant.
    let body = emulator
        .post_ok(
            "fixture-position",
            REVERSE_GEOCODE,
            &json!({"QueryPosition": [33.859_39, 45.714_2]}),
        )
        .await;
    let place_id = text(&items(&body)[0], "PlaceId");
    for (case, view, expected) in [
        ("default", None, "UA"),
        ("claimant", Some("RUS"), "RU"),
        ("bystander", Some("FRA"), "UA"),
    ] {
        let path = match view {
            Some(view) => format!("/v2/place/{place_id}?political-view={view}"),
            None => format!("/v2/place/{place_id}"),
        };
        let detail = emulator.get_ok(case, &path).await;
        assert_eq!(
            nested(&detail, "Address.Country.Code2"),
            expected,
            "case get-place-{case}: expected the attribution of the political view \
             the read asked for, got {detail:?}"
        );
    }
}

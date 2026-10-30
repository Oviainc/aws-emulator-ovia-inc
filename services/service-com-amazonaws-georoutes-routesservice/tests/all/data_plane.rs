// SPDX-License-Identifier: CC0-1.0

//! What the data plane reports about the journey it computed: which trace points the road
//! network matched, which pairs it has no route for, the language a route's narration is
//! rendered in, the time a journey spends stopped, and the clusters a waypoint
//! optimization was asked to form.
//!
//! The rule under test is never "this recorded request gives this recorded number", so
//! every table spans the inputs its rule generalises over: where a position lies (mapped
//! land, a different landmass, international water), travel mode, the boundary geometry,
//! the stop and service durations, and the clustering algorithm. Each catalog written out
//! by hand names its source.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::routing::{int, post, routes_service, MATRIX, OPTIMIZE, ROUTES, SNAP};

/// Positions in mapped territory, where the road network has coverage.
const SEATTLE: [f64; 2] = [-122.3321, 47.6062];
const PORTLAND: [f64; 2] = [-122.6784, 45.5152];
const SAN_FRANCISCO: [f64; 2] = [-122.4194, 37.7749];
const DENVER: [f64; 2] = [-104.9903, 39.7392];
const VANCOUVER: [f64; 2] = [-123.1, 49.28];
const KANSAS: [f64; 2] = [-100.0, 40.0];
/// Two positions on Oahu: a pair on one island, which a road network connects.
const HONOLULU: [f64; 2] = [-157.8583, 21.3069];
const HONOLULU_EAST: [f64; 2] = [-157.8, 21.28];

/// Positions in international waters, where no territory is mapped and so no road network
/// exists: the mid North Pacific, the mid North Atlantic, the middle of the
/// Mediterranean, the mid Caribbean and the deep Pacific west of Oregon.
const PACIFIC: [f64; 2] = [-140.0, 40.0];
const ATLANTIC: [f64; 2] = [-40.0, 40.0];
const MEDITERRANEAN: [f64; 2] = [17.0, 35.0];
const CARIBBEAN: [f64; 2] = [-75.0, 18.0];
const DEEP_PACIFIC: [f64; 2] = [-130.5, 44.6];

// ---------------------------------------------------------------------------
// SnapToRoads: the points no road matched
// ---------------------------------------------------------------------------

/// A trace point the road network could not match is REPORTED: its index is listed under
/// a `RoadSnapNotice`, and only the indexes of the points that were not matched are.
///
/// Rows differ along what the rule generalises over: where the unmatched points sit in the
/// trace, how many there are, which travel mode was asked for and how wide a snap radius
/// the caller allowed — a radius does not conjure a road where the network has none.
#[tokio::test]
async fn snap_to_roads_reports_the_trace_points_no_road_matched() {
    let router = routes_service();
    let on_road = [
        [-122.3331, 47.6052],
        [-122.3321, 47.6062],
        [-122.3311, 47.6072],
        [-122.3301, 47.6082],
    ];

    // name, the trace, the indexes no road can match, travel mode, SnapRadius.
    type SnapCase = (&'static str, Vec<[f64; 2]>, Vec<i64>, &'static str, Option<i64>);
    let cases: [SnapCase; 7] = [
        (
            "every point on a road",
            vec![on_road[0], on_road[1], on_road[2]],
            vec![],
            "Car",
            None,
        ),
        (
            "open water last",
            vec![on_road[0], on_road[1], on_road[2], PACIFIC],
            vec![3],
            "Truck",
            None,
        ),
        (
            "open water in the middle",
            vec![on_road[0], on_road[1], PACIFIC, on_road[2]],
            vec![2],
            "Car",
            None,
        ),
        (
            "open water first",
            vec![DEEP_PACIFIC, on_road[0], on_road[1]],
            vec![0],
            "Pedestrian",
            None,
        ),
        (
            "two unmatched points, in different oceans",
            vec![on_road[0], ATLANTIC, on_road[1], MEDITERRANEAN],
            vec![1, 3],
            "Scooter",
            None,
        ),
        (
            "the widest radius the model allows still matches no road",
            vec![on_road[0], on_road[1], PACIFIC],
            vec![2],
            "Car",
            Some(10_000),
        ),
        (
            "a trace entirely in open water",
            vec![PACIFIC, DEEP_PACIFIC],
            vec![0, 1],
            "Car",
            None,
        ),
    ];

    for (case, trace, unmatched, travel_mode, snap_radius) in cases {
        let mut body = json!({
            "TracePoints": trace.iter().map(|position| json!({ "Position": position })).collect::<Vec<Value>>(),
            "TravelMode": travel_mode,
            "SnappedGeometryFormat": "Simple",
        });
        if let Some(radius) = snap_radius {
            body["SnapRadius"] = json!(radius);
        }
        let (status, answer, _) = post(&router, SNAP, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let notices = answer["Notices"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Notices is required, got {answer:?}"));
        let mut flagged: BTreeSet<i64> = BTreeSet::new();
        for notice in notices {
            assert_eq!(
                notice["Code"], json!("TracePointsNotMatched"),
                "case {case:?}: a point no road matched is reported under the code for it, got {notice:?}"
            );
            assert!(
                notice["Title"].as_str().is_some_and(|title| !title.is_empty()),
                "case {case:?}: Title is required on a notice, got {notice:?}"
            );
            let indexes = notice["TracePointIndexes"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: TracePointIndexes is required, got {notice:?}"));
            assert!(
                (1..=1_000).contains(&indexes.len()),
                "case {case:?}: TracePointIndexes carries 1..1000 entries, got {}",
                indexes.len()
            );
            flagged.extend(indexes.iter().filter_map(Value::as_i64));
        }
        assert_eq!(
            flagged,
            unmatched.iter().copied().collect::<BTreeSet<i64>>(),
            "case {case:?}: exactly the unmatched indexes are flagged, got notices {notices:?}"
        );

        // Every submitted point is answered for, at its own index: an unmatched one keeps
        // the position it was submitted at and scores no confidence, so the notice is the
        // only thing that tells it apart from a road fix.
        let snapped = answer["SnappedTracePoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: SnappedTracePoints is required, got {answer:?}"));
        assert_eq!(
            snapped.len(), trace.len(),
            "case {case:?}: one snapped point per trace point, got {snapped:?}"
        );
        for (index, point) in snapped.iter().enumerate() {
            let confidence = point["Confidence"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: Confidence is required, got {point:?}"));
            let is_unmatched = unmatched.contains(&(index as i64));
            assert_eq!(
                confidence == 0.0, is_unmatched,
                "case {case:?}: point {index} scores no confidence exactly when no road matched it, got {point:?}"
            );
            if is_unmatched {
                assert_eq!(
                    point["SnappedPosition"], point["OriginalPosition"],
                    "case {case:?}: point {index} was not moved onto a road, got {point:?}"
                );
            }
        }

        // The snapped geometry is the line through the points that DID match a road, so a
        // trace with fewer than two of them has no line to draw.
        let matched = trace.len() - unmatched.len();
        assert_eq!(
            answer["SnappedGeometry"]["LineString"].is_array(), matched >= 2,
            "case {case:?}: {matched} matched points, got geometry {:?}",
            answer["SnappedGeometry"]
        );
    }
}

// ---------------------------------------------------------------------------
// CalculateRouteMatrix: the cells the network cannot answer
// ---------------------------------------------------------------------------

/// A cell the road network cannot answer carries the `RouteMatrixErrorCode` for WHY, and
/// no fabricated metrics: a pair no road connects is `NoRoute`, and an endpoint the
/// network cannot match is the qualified `NoMatch` code for that endpoint. `ErrorCount`
/// is the number of cells carrying an error.
///
/// Rows differ along what the rule generalises over: which endpoint the network cannot
/// serve, whether the pair is on one landmass, how far apart the pair is, and the matrix
/// shape.
#[tokio::test]
async fn matrix_cells_the_network_cannot_route_carry_the_error_code_for_why() {
    let router = routes_service();

    // name, origins, destinations, the expected error code of each cell in row-major
    // order (`None` for a cell that must be routed).
    type MatrixCase = (
        &'static str,
        Vec<[f64; 2]>,
        Vec<[f64; 2]>,
        Vec<Option<&'static str>>,
    );
    let cases: [MatrixCase; 8] = [
        (
            "pairs on one landmass",
            vec![SEATTLE, PORTLAND],
            vec![SAN_FRANCISCO, VANCOUVER],
            vec![None, None, None, None],
        ),
        (
            "a long pair on one landmass",
            vec![SEATTLE],
            vec![DENVER],
            vec![None],
        ),
        (
            "an island origin, with a mainland origin beside it",
            vec![SEATTLE, HONOLULU],
            vec![PORTLAND, SAN_FRANCISCO],
            vec![None, None, Some("NoRoute"), Some("NoRoute")],
        ),
        (
            "an island destination",
            vec![PORTLAND],
            vec![HONOLULU],
            vec![Some("NoRoute")],
        ),
        (
            "a pair on the same island",
            vec![HONOLULU],
            vec![HONOLULU_EAST],
            vec![None],
        ),
        (
            "an origin in open water",
            vec![PACIFIC, SEATTLE],
            vec![PORTLAND],
            vec![Some("NoMatchOrigin"), None],
        ),
        (
            "a destination in open water",
            vec![SEATTLE],
            vec![ATLANTIC, PORTLAND],
            vec![Some("NoMatchDestination"), None],
        ),
        (
            "both endpoints in open water",
            vec![CARIBBEAN],
            vec![DEEP_PACIFIC],
            vec![Some("NoMatch")],
        ),
    ];

    for (case, origins, destinations, expected) in cases {
        let (status, answer, _) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": origins.iter().map(|position| json!({ "Position": position })).collect::<Vec<Value>>(),
                "Destinations": destinations.iter().map(|position| json!({ "Position": position })).collect::<Vec<Value>>(),
                "TravelMode": "Car",
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let matrix = answer["RouteMatrix"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: RouteMatrix is required, got {answer:?}"));
        let cells: Vec<&Value> = matrix
            .iter()
            .flat_map(|row| row.as_array().map(Vec::as_slice).unwrap_or_default())
            .collect();
        assert_eq!(
            cells.len(), expected.len(),
            "case {case:?}: one cell per pair, got {matrix:?}"
        );
        for ((cell, expected), index) in cells.iter().zip(&expected).zip(0..) {
            match expected {
                Some(code) => {
                    assert_eq!(
                        cell["Error"], json!(code),
                        "case {case:?}: cell {index} must report {code}, got {cell:?}"
                    );
                    assert_eq!(
                        (int(cell, "Distance", case), int(cell, "Duration", case)),
                        (0, 0),
                        "case {case:?}: cell {index} was not routed, so it carries no metrics, got {cell:?}"
                    );
                }
                None => {
                    assert!(
                        cell["Error"].is_null(),
                        "case {case:?}: cell {index} must be routed, got {cell:?}"
                    );
                    assert!(
                        int(cell, "Distance", case) > 0 && int(cell, "Duration", case) > 0,
                        "case {case:?}: cell {index} carries the computed metrics, got {cell:?}"
                    );
                }
            }
        }
        assert_eq!(
            answer["ErrorCount"],
            json!(expected.iter().filter(|cell| cell.is_some()).count()),
            "case {case:?}: ErrorCount is the number of cells carrying an error, got {:?} in {matrix:?}",
            answer["ErrorCount"]
        );
    }
}

/// A cell the routing boundary rejects carries the UNQUALIFIED `OutOfBounds`: the boundary
/// bounds the area the matrix is computed in, equally for an origin and for a destination,
/// so the failure belongs to the pair rather than to one endpoint's role in it.
///
/// Rows span the boundary geometries the model offers and which endpoint falls outside.
#[tokio::test]
async fn a_cell_outside_the_routing_boundary_carries_the_unqualified_code() {
    let router = routes_service();
    let circle = json!({ "Geometry": { "Circle": { "Center": VANCOUVER, "Radius": 50_000.0 } } });
    let bounding_box = json!({ "Geometry": { "BoundingBox": [-123.3, 49.1, -123.0, 49.4] } });
    let polygon = json!({
        "Geometry": {
            "Polygon": [[
                [-123.3, 49.1], [-123.0, 49.1], [-123.0, 49.4], [-123.3, 49.4], [-123.3, 49.1]
            ]]
        }
    });
    let auto_circle = json!({ "Geometry": { "AutoCircle": { "Margin": 0, "MaxRadius": 1_000 } } });

    // name, the routing boundary, origins, destinations, how many cells must be out of
    // bounds.
    type BoundaryCase = (&'static str, Value, Vec<[f64; 2]>, Vec<[f64; 2]>, usize);
    let cases: [BoundaryCase; 6] = [
        ("circle, destination outside", circle.clone(), vec![VANCOUVER], vec![KANSAS], 1),
        ("circle, origin outside", circle.clone(), vec![KANSAS], vec![VANCOUVER], 1),
        ("circle, both outside", circle, vec![KANSAS], vec![DENVER], 1),
        (
            "bounding box, destination outside",
            bounding_box,
            vec![VANCOUVER],
            vec![KANSAS, VANCOUVER],
            1,
        ),
        ("polygon, origin outside", polygon, vec![KANSAS, VANCOUVER], vec![VANCOUVER], 1),
        (
            "auto circle, a waypoint beyond the derived radius",
            auto_circle,
            vec![VANCOUVER],
            vec![KANSAS],
            1,
        ),
    ];

    for (case, boundary, origins, destinations, out_of_bounds) in cases {
        let (status, answer, _) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": origins.iter().map(|position| json!({ "Position": position })).collect::<Vec<Value>>(),
                "Destinations": destinations.iter().map(|position| json!({ "Position": position })).collect::<Vec<Value>>(),
                "RoutingBoundary": boundary,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let errors: Vec<&Value> = answer["RouteMatrix"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: RouteMatrix is required, got {answer:?}"))
            .iter()
            .flat_map(|row| row.as_array().map(Vec::as_slice).unwrap_or_default())
            .filter(|cell| !cell["Error"].is_null())
            .collect();
        assert_eq!(
            errors.len(), out_of_bounds,
            "case {case:?}: expected {out_of_bounds} rejected cells, got {errors:?}"
        );
        for cell in errors {
            assert_eq!(
                cell["Error"], json!("OutOfBounds"),
                "case {case:?}: a boundary rejection is not attributed to one endpoint, got {cell:?}"
            );
        }
        assert_eq!(
            answer["ErrorCount"], json!(out_of_bounds),
            "case {case:?}: ErrorCount counts the rejected cells, got {:?}",
            answer["ErrorCount"]
        );
    }
}

// ---------------------------------------------------------------------------
// CalculateRoutes: the language the narration is rendered in
// ---------------------------------------------------------------------------

/// `MainLanguageNotFound` is reported for exactly one case: a request whose `Languages`
/// name no language this emulator can render the narration in. A language it can render is
/// rendered in, the leg reports the language actually used, and the narration differs from
/// the English one.
///
/// The languages below are the ones the service's own narration catalog holds
/// (`NARRATIONS` in `src/ops/calculate_routes.rs`), each asked for through a tag the issue
/// did not cite, beside tags of languages the catalog does not hold.
#[tokio::test]
async fn the_fallback_notice_is_reported_only_when_no_requested_language_resolves() {
    let router = routes_service();
    let english = instruction_text(&router, &["en"], "english").await;

    // name, requested `Languages`, the language tag every leg must report, whether the
    // fallback notice must be reported.
    type LanguageCase = (&'static str, Vec<&'static str>, &'static str, bool);
    let cases: [LanguageCase; 11] = [
        ("none asked for", vec![], "en-us", false),
        ("the default language", vec!["en"], "en", false),
        ("a regional variant of it", vec!["en-AU"], "en-AU", false),
        ("Spanish", vec!["es-ES"], "es-ES", false),
        ("Latin American Spanish", vec!["es-419"], "es-419", false),
        ("French", vec!["fr-FR"], "fr-FR", false),
        ("German", vec!["de-AT"], "de-AT", false),
        ("Italian", vec!["it"], "it", false),
        ("Portuguese", vec!["pt-BR"], "pt-BR", false),
        ("a language the catalog does not hold", vec!["ja-JP"], "en-us", true),
        ("the first held language of several", vec!["ja", "fr", "es"], "fr", false),
    ];

    for (case, languages, expected_tag, expects_notice) in cases {
        let mut body = json!({
            "Origin": VANCOUVER,
            "Destination": [-123.12, 49.26],
            "LegAdditionalFeatures": ["TravelStepInstructions"],
        });
        if !languages.is_empty() {
            body["Languages"] = json!(languages);
        }
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let notices = answer["Notices"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Notices is required, got {answer:?}"));
        let reported = notices
            .iter()
            .any(|notice| notice["Code"] == json!("MainLanguageNotFound"));
        assert_eq!(
            reported, expects_notice,
            "case {case:?}: the fallback notice is for a language that cannot be provided, got {notices:?}"
        );

        let legs = answer["Routes"][0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {answer:?}"));
        assert!(!legs.is_empty(), "case {case:?}: a route carries legs, got {answer:?}");
        for leg in legs {
            assert_eq!(
                leg["Language"], json!(expected_tag),
                "case {case:?}: the leg names the language its instructions are in, got {:?}",
                leg["Language"]
            );
        }

        // The narration itself follows: a language the catalog holds is rendered in its
        // own phrases, and a request answered in the default reads like the English one,
        // whether it asked for English or fell back to it.
        let rendered = instruction_text(&router, &languages, case).await;
        assert_eq!(
            rendered == english,
            expected_tag.starts_with("en"),
            "case {case:?}: expected the narration of {expected_tag}, got {rendered:?} against the English {english:?}"
        );
    }
}

/// The first travel step's instruction of a short route, in the requested languages.
async fn instruction_text(
    router: &smithy_server_runtime::MultiServiceRouter,
    languages: &[&str],
    case: &str,
) -> String {
    let mut body = json!({
        "Origin": VANCOUVER,
        "Destination": [-123.12, 49.26],
        "LegAdditionalFeatures": ["TravelStepInstructions"],
    });
    if !languages.is_empty() {
        body["Languages"] = json!(languages);
    }
    let (status, answer, _) = post(router, ROUTES, &body).await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
    answer["Routes"][0]["Legs"][0]["VehicleLegDetails"]["TravelSteps"][0]["Instruction"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the narration was asked for, so a step carries an instruction: {answer:?}")
        })
        .to_string()
}

// ---------------------------------------------------------------------------
// The time a journey spends stopped
// ---------------------------------------------------------------------------

/// A waypoint's `StopDuration` is time the journey spends, so it is part of the route's
/// `Summary.Duration` and of the leg overview that covers the leg's after-travel step —
/// while the leg's travel-only summary keeps reporting the travel alone.
///
/// Rows differ along what the rule generalises over: how long the stop is (from none to
/// the maximum the model allows), how many waypoints stop, and whether the waypoint is
/// stopped at at all.
#[tokio::test]
async fn a_waypoint_stop_is_part_of_the_route_duration() {
    let router = routes_service();
    let waypoints = [[-123.11, 49.275], [-123.105, 49.27]];

    // name, the stop duration of each waypoint, whether each waypoint is passed through,
    // the seconds the stops must add to the route's duration.
    type StopCase = (&'static str, Vec<i64>, bool, i64);
    let cases: [StopCase; 7] = [
        ("no stop at all", vec![0], false, 0),
        ("a one second stop", vec![1], false, 1),
        ("a one minute stop", vec![60], false, 60),
        ("a quarter hour stop", vec![900], false, 900),
        ("the longest stop the model allows", vec![49_999], false, 49_999),
        ("two waypoints that both stop", vec![300, 600], false, 900),
        // "If the waypoint should not be treated as a stop ... the waypoint is passed
        // through": nothing stops there, so nothing is spent there.
        ("a waypoint passed through", vec![900], true, 0),
    ];

    for (case, stops, pass_through, expected) in cases {
        let submitted: Vec<Value> = stops
            .iter()
            .zip(waypoints)
            .map(|(stop, position)| {
                json!({ "Position": position, "StopDuration": stop, "PassThrough": pass_through })
            })
            .collect();
        let baseline: Vec<Value> = submitted
            .iter()
            .map(|waypoint| {
                let mut waypoint = waypoint.clone();
                waypoint["StopDuration"] = json!(0);
                waypoint
            })
            .collect();

        let request = |waypoints: Vec<Value>| {
            json!({
                "Origin": VANCOUVER,
                "Destination": [-123.12, 49.26],
                "Waypoints": waypoints,
            })
        };
        let (status, without, _) = post(&router, ROUTES, &request(baseline)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {without:?}");
        let (status, with, _) = post(&router, ROUTES, &request(submitted)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {with:?}");

        let travel = int(&without["Routes"][0]["Summary"], "Duration", case);
        let total = int(&with["Routes"][0]["Summary"], "Duration", case);
        assert_eq!(
            total - travel,
            expected,
            "case {case:?}: the stops must add {expected} s to the route duration, got {total} against {travel}"
        );
        assert_eq!(
            int(&with["Routes"][0]["Summary"], "Distance", case),
            int(&without["Routes"][0]["Summary"], "Distance", case),
            "case {case:?}: a stop travels nowhere, so the distance is unchanged"
        );

        // Per leg: the overview covers the leg's travel and the stop at its end, the
        // travel-only summary just the travel, and the two differ by exactly that stop.
        let legs = with["Routes"][0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {with:?}"));
        let mut overview_total = 0;
        for (index, leg) in legs.iter().enumerate() {
            let summary = &leg["VehicleLegDetails"]["Summary"];
            let overview = int(&summary["Overview"], "Duration", case);
            let travel_only = int(&summary["TravelOnly"], "Duration", case);
            let after_travel = leg["VehicleLegDetails"]["AfterTravelSteps"]
                .as_array()
                .map(|steps| steps.iter().filter_map(|step| step["Duration"].as_i64()).sum::<i64>())
                .unwrap_or_default();
            assert_eq!(
                overview - travel_only, after_travel,
                "case {case:?}: leg {index} overview covers its after-travel steps, got {summary:?}"
            );
            overview_total += overview;
        }
        assert_eq!(
            overview_total, total,
            "case {case:?}: the route duration is the sum of its legs' overviews, got {total}"
        );
    }
}

/// A waypoint's `ServiceDuration` is time the journey spends, so the optimized sequence's
/// overall `Duration` is every phase `TimeBreakdown` splits the cycle into — travel, rest,
/// wait and service — and never the travel alone.
///
/// Rows differ along what the rule generalises over: which stops are serviced, how long
/// for, and whether the destination is the serviced one.
#[tokio::test]
async fn waypoint_service_time_is_part_of_the_optimized_duration() {
    let router = routes_service();
    let first = [-123.11, 49.27];
    let second = [-123.05, 49.25];

    // name, the service duration of each waypoint, the destination's service duration.
    type ServiceCase = (&'static str, Vec<i64>, i64);
    let cases: [ServiceCase; 5] = [
        ("no service time", vec![0, 0], 0),
        ("one serviced waypoint", vec![600, 0], 0),
        ("both waypoints serviced", vec![600, 300], 0),
        ("a serviced destination", vec![0, 0], 1_200),
        ("every stop serviced", vec![45, 90], 30),
    ];

    for (case, services, destination_service) in cases {
        let request = |services: &[i64], destination: i64| {
            json!({
                "Origin": VANCOUVER,
                "Destination": [-123.12, 49.26],
                "DestinationOptions": { "Id": "end", "ServiceDuration": destination },
                "Waypoints": [
                    { "Id": "a", "Position": first, "ServiceDuration": services[0] },
                    { "Id": "b", "Position": second, "ServiceDuration": services[1] },
                ],
                "DepartureTime": "2026-04-22T17:57:24Z",
            })
        };
        let (status, baseline, _) = post(&router, OPTIMIZE, &request(&[0, 0], 0)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {baseline:?}");
        let (status, answer, _) = post(&router, OPTIMIZE, &request(&services, destination_service)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let expected: i64 = services.iter().sum::<i64>() + destination_service;
        let travel = int(&baseline, "Duration", case);
        let total = int(&answer, "Duration", case);
        assert_eq!(
            total - travel,
            expected,
            "case {case:?}: the service time must add {expected} s to the overall duration, got {total} against {travel}"
        );

        let breakdown = &answer["TimeBreakdown"];
        assert_eq!(
            int(breakdown, "ServiceDuration", case),
            expected,
            "case {case:?}: the breakdown reports the service time, got {breakdown:?}"
        );
        assert_eq!(
            int(breakdown, "TravelDuration", case)
                + int(breakdown, "RestDuration", case)
                + int(breakdown, "WaitDuration", case)
                + int(breakdown, "ServiceDuration", case),
            total,
            "case {case:?}: the overall duration is the phases it is split into, got {breakdown:?} against {total}"
        );
    }
}

// ---------------------------------------------------------------------------
// OptimizeWaypoints: clustering
// ---------------------------------------------------------------------------

/// Clustering is performed, and every optimized waypoint reports the cluster it was
/// assigned to: `DrivingDistance` puts the stops within the configured driving distance of
/// each other in one cluster, `TopologySegment` puts the stops on one stretch of road
/// between two junctions in one, and a request that asks for no clustering reports no
/// `ClusterIndex` at all.
///
/// The two algorithms are every value of `WaypointOptimizationClusteringAlgorithm`, from
/// `smithy_query(query="shape",shape="WaypointOptimizationClusteringAlgorithm")`.
#[tokio::test]
async fn clustering_groups_the_waypoints_the_algorithm_puts_together() {
    let router = routes_service();
    // Two stops ~150 m apart, and one ~7 km away from them.
    let close_a = [-123.1001, 49.28];
    let close_b = [-123.1002, 49.28];
    let far = [-123.0, 49.28];

    // name, the `Clustering` member, whether the two close waypoints must share a cluster,
    // whether the far one must be in a cluster of its own.
    type ClusterCase = (&'static str, Option<Value>, bool, bool);
    let cases: [ClusterCase; 5] = [
        ("no clustering asked for", None, false, false),
        (
            "driving distance wide enough to join the close pair",
            Some(json!({
                "Algorithm": "DrivingDistance",
                "DrivingDistanceOptions": { "DrivingDistance": 5_000 }
            })),
            true,
            true,
        ),
        (
            "driving distance at the smallest the model allows",
            Some(json!({
                "Algorithm": "DrivingDistance",
                "DrivingDistanceOptions": { "DrivingDistance": 5 }
            })),
            false,
            true,
        ),
        (
            "driving distance left at the default",
            Some(json!({ "Algorithm": "DrivingDistance" })),
            false,
            true,
        ),
        (
            "the topology segment the stops lie on",
            Some(json!({ "Algorithm": "TopologySegment" })),
            true,
            true,
        ),
    ];

    for (case, clustering, close_together, far_apart) in cases {
        let mut body = json!({
            "Origin": far,
            "Waypoints": [
                { "Id": "close-a", "Position": close_a },
                { "Id": "close-b", "Position": close_b },
                { "Id": "far", "Position": far },
            ],
            "DepartureTime": "2026-04-22T17:57:24Z",
        });
        if let Some(clustering) = &clustering {
            body["Clustering"] = clustering.clone();
        }
        let (status, answer, _) = post(&router, OPTIMIZE, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let optimized = answer["OptimizedWaypoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: OptimizedWaypoints is required, got {answer:?}"));
        let index_of = |id: &str| -> Option<i64> {
            optimized
                .iter()
                .find(|waypoint| waypoint["Id"] == json!(id))
                .and_then(|waypoint| waypoint["ClusterIndex"].as_i64())
        };
        if clustering.is_none() {
            // "The index is included in the response only if clustering was performed
            // while processing the request."
            for waypoint in optimized {
                assert!(
                    waypoint["ClusterIndex"].is_null(),
                    "case {case:?}: no clustering was performed, so no index is reported, got {waypoint:?}"
                );
            }
            continue;
        }
        for waypoint in optimized {
            let index = waypoint["ClusterIndex"].as_i64();
            assert!(
                index.is_some_and(|index| index >= 0),
                "case {case:?}: a clustered waypoint reports its cluster, got {waypoint:?}"
            );
        }
        let (a, b, away) = (
            index_of("close-a").expect("close-a is clustered"),
            index_of("close-b").expect("close-b is clustered"),
            index_of("far").expect("far is clustered"),
        );
        assert_eq!(
            a == b, close_together,
            "case {case:?}: expected the close pair {}to share a cluster, got {a} and {b}",
            if close_together { "" } else { "not " }
        );
        assert_eq!(
            away != a && away != b, far_apart,
            "case {case:?}: expected the far waypoint {}in a cluster of its own, got {away} against {a} and {b}",
            if far_apart { "" } else { "not " }
        );
    }
}

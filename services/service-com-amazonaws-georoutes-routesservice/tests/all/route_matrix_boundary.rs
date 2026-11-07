// SPDX-License-Identifier: CC0-1.0

//! The routing boundary `CalculateRouteMatrix` reports back.
//!
//! `CalculateRouteMatrixResponse.RoutingBoundary` is a required member that states the
//! boundary the matrix was evaluated under: WHICH area bounded it and WHETHER that area was
//! unrestricted. The rule under test is that both parts are always reported — the
//! `Unbounded` boolean carries the value that applied instead of being left absent — so the
//! tables below span every boundary form the model offers (`Circle`, `BoundingBox`,
//! `Polygon`, the request-only `AutoCircle`, `Unbounded`, and no boundary member at all),
//! not one recorded request.

use serde_json::{json, Value};

use crate::routing::{int, post, routes_service, MATRIX};

/// Positions in mapped territory, near enough one another to be inside the bounded
/// geometries below.
const VANCOUVER: [f64; 2] = [-123.1, 49.28];
const VANCOUVER_EAST: [f64; 2] = [-123.05, 49.25];

/// Every boundary form, as `RoutingBoundary` carries it in a request, paired with the
/// `Unbounded` value the answer must report for it. The forms are the members of
/// `RouteMatrixBoundary` and `RouteMatrixBoundaryGeometry`
/// (`smithy_query(query="shape",shape="RouteMatrixBoundary,RouteMatrixBoundaryGeometry")`),
/// plus the request that sets no boundary at all.
fn boundary_cases() -> Vec<(&'static str, Option<Value>, bool)> {
    vec![
        ("no RoutingBoundary member", None, true),
        ("Unbounded set to true", Some(json!({ "Unbounded": true })), true),
        (
            "Circle",
            Some(json!({ "Geometry": { "Circle": { "Center": VANCOUVER, "Radius": 50_000.0 } } })),
            false,
        ),
        (
            "BoundingBox",
            Some(json!({ "Geometry": { "BoundingBox": [-123.3, 49.1, -123.0, 49.4] } })),
            false,
        ),
        (
            "Polygon",
            Some(json!({
                "Geometry": {
                    "Polygon": [[
                        [-123.3, 49.1], [-123.0, 49.1], [-123.0, 49.4], [-123.3, 49.4], [-123.3, 49.1]
                    ]]
                }
            })),
            false,
        ),
        (
            "AutoCircle",
            Some(json!({ "Geometry": { "AutoCircle": { "Margin": 1_000, "MaxRadius": 100_000 } } })),
            false,
        ),
    ]
}

/// A 1x2 matrix over positions every bounded geometry above contains.
fn request(boundary: Option<&Value>) -> Value {
    let mut body = json!({
        "Origins": [{ "Position": VANCOUVER }],
        "Destinations": [{ "Position": VANCOUVER }, { "Position": VANCOUVER_EAST }],
        "TravelMode": "Car",
    });
    if let Some(boundary) = boundary {
        body["RoutingBoundary"] = boundary.clone();
    }
    body
}

/// The answer reports the boundary the matrix was evaluated under, with `Unbounded`
/// carrying the value that applied: `true` where nothing bounded the area, `false` beside
/// the geometry that did.
#[tokio::test]
async fn the_matrix_reports_the_boundary_it_was_evaluated_under() {
    let router = routes_service();

    for (case, boundary, unbounded) in boundary_cases() {
        let (status, answer, _) = post(&router, MATRIX, &request(boundary.as_ref())).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        let reported = &answer["RoutingBoundary"];
        assert!(
            reported.is_object(),
            "case {case:?}: RoutingBoundary is required, got {answer:?}"
        );
        assert_eq!(
            reported["Unbounded"], json!(unbounded),
            "case {case:?}: expected Unbounded {unbounded}, got {reported:?}"
        );
        if unbounded {
            assert!(
                reported["Geometry"].is_null(),
                "case {case:?}: nothing bounded the area, so no geometry is reported, got {reported:?}"
            );
            continue;
        }
        // A geometry boundary reports the geometry it bounded the matrix by. `AutoCircle`
        // is request-only: "the response routing boundary will return Circle derived from
        // the AutoCircle settings".
        let submitted = boundary.as_ref().expect("a bounded case carries a boundary");
        if submitted["Geometry"]["AutoCircle"].is_null() {
            assert_eq!(
                reported["Geometry"], submitted["Geometry"],
                "case {case:?}: expected the submitted geometry echoed, got {reported:?}"
            );
        } else {
            assert!(
                reported["Geometry"]["Circle"].is_object()
                    && reported["Geometry"]["AutoCircle"].is_null(),
                "case {case:?}: AutoCircle is reported as the Circle it derived, got {reported:?}"
            );
        }
    }
}

/// The reported boundary is the boundary itself, not a description of one: submitting it
/// back verbatim computes the same matrix under the same boundary, whichever form bounded
/// the first call.
#[tokio::test]
async fn the_reported_boundary_can_be_submitted_back_unchanged() {
    let router = routes_service();

    for (case, boundary, _) in boundary_cases() {
        let (status, first, _) = post(&router, MATRIX, &request(boundary.as_ref())).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {first:?}");
        let reported = first["RoutingBoundary"].clone();
        let (status, again, _) = post(&router, MATRIX, &request(Some(&reported))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the reported boundary must be a boundary a request may carry, got {status} with {again:?}"
        );
        assert_eq!(
            again["RoutingBoundary"], reported,
            "case {case:?}: expected the same boundary reported again, got {:?}",
            again["RoutingBoundary"]
        );
        assert_eq!(
            again["RouteMatrix"], first["RouteMatrix"],
            "case {case:?}: the same boundary must give the same matrix, got {:?} against {:?}",
            again["RouteMatrix"], first["RouteMatrix"]
        );
        assert_eq!(
            int(&again, "ErrorCount", case), int(&first, "ErrorCount", case),
            "case {case:?}: the same boundary must give the same error count, got {:?} against {:?}",
            again["ErrorCount"], first["ErrorCount"]
        );
    }
}

/// The boundary reported is the one the cells were judged against: a pair outside it is
/// refused while the pair inside it is routed, under every bounded form.
#[tokio::test]
async fn the_reported_boundary_is_the_one_the_cells_were_judged_against() {
    let router = routes_service();
    // Far outside every bounded geometry of `boundary_cases`, and inside no circle it
    // derives for waypoints around Vancouver.
    let far = [-100.0, 40.0];

    for (case, boundary, unbounded) in boundary_cases() {
        // `AutoCircle` derives its circle from the submitted waypoints themselves, so a
        // far destination moves the circle rather than falling outside it; the fixed
        // geometries are what a pair can be outside of.
        if boundary
            .as_ref()
            .is_some_and(|boundary| !boundary["Geometry"]["AutoCircle"].is_null())
        {
            continue;
        }
        let mut body = request(boundary.as_ref());
        body["Destinations"] = json!([{ "Position": VANCOUVER_EAST }, { "Position": far }]);
        let (status, answer, _) = post(&router, MATRIX, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert_eq!(
            answer["RoutingBoundary"]["Unbounded"], json!(unbounded),
            "case {case:?}: expected Unbounded {unbounded}, got {:?}",
            answer["RoutingBoundary"]
        );
        let row = answer["RouteMatrix"][0]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: RouteMatrix is required, got {answer:?}"));
        assert!(
            row[0]["Error"].is_null() && int(&row[0], "Distance", case) > 0,
            "case {case:?}: the pair inside the boundary is routed, got {:?}",
            row[0]
        );
        if unbounded {
            assert!(
                row[1]["Error"].is_null(),
                "case {case:?}: nothing bounded the area, so no pair is out of bounds, got {:?}",
                row[1]
            );
            assert_eq!(
                int(&answer, "ErrorCount", case), 0,
                "case {case:?}: expected no failed cell, got {answer:?}"
            );
        } else {
            assert_eq!(
                row[1]["Error"], json!("OutOfBounds"),
                "case {case:?}: the pair outside the reported boundary is refused, got {:?}",
                row[1]
            );
            assert_eq!(
                int(&answer, "ErrorCount", case), 1,
                "case {case:?}: expected exactly one failed cell, got {answer:?}"
            );
        }
    }
}

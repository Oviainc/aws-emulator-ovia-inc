// SPDX-License-Identifier: CC0-1.0

//! Probes of the ROAD NETWORK'S COVERAGE: one position, asked about through each
//! operation that reports what the network could do with it, must be answered the same way
//! by all of them.
//!
//! The network has coverage where a territory of the world is mapped and nowhere else, so
//! the table below spans both sides of that: positions in mapped territory and positions
//! in international waters, each carried through SnapToRoads, CalculateRouteMatrix and
//! CalculateRoutes. The rule is the consistency itself — a position one operation reports
//! as unmatched is a position the others cannot place on a road either — not any single
//! operation's wording.

use serde_json::json;

use crate::routing::{post, routes_service, MATRIX, ROUTES, SNAP};

/// A position in mapped territory, where the network has roads, and the reference stop
/// every probe below is paired with.
const ON_LAND: [f64; 2] = [-122.3321, 47.6062];

#[tokio::test]
async fn one_position_is_covered_or_not_by_every_operation_alike() {
    let router = routes_service();

    // name, the position probed, whether the road network covers it.
    let cases: [(&str, [f64; 2], bool); 6] = [
        ("a city street", [-122.3301, 47.6082], true),
        ("an island city", [-157.8583, 21.3069], true),
        ("a coastal position just offshore", [-122.4523, 47.7531], true),
        ("the mid North Pacific", [-140.0, 40.0], false),
        ("the mid North Atlantic", [-40.0, 40.0], false),
        ("the middle of the Mediterranean", [17.0, 35.0], false),
    ];

    for (case, position, covered) in cases {
        // SnapToRoads: an uncovered position is listed under a notice and moved nowhere.
        let (status, snapped, _) = post(
            &router,
            SNAP,
            &json!({
                "TracePoints": [{ "Position": ON_LAND }, { "Position": position }],
                "SnappedGeometryFormat": "Simple",
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {snapped:?}");
        let flagged = snapped["Notices"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Notices is required, got {snapped:?}"))
            .iter()
            .flat_map(|notice| {
                notice["TracePointIndexes"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
            })
            .any(|index| index == &json!(1));
        assert_eq!(
            flagged, !covered,
            "case {case:?}: the probed point is unmatched exactly when the network does not cover it, got {:?}",
            snapped["Notices"]
        );

        // CalculateRouteMatrix: the same position as a destination is the cell's own
        // failure, named for the endpoint it is.
        let (status, matrix, _) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": [{ "Position": ON_LAND }],
                "Destinations": [{ "Position": position }],
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {matrix:?}");
        let cell = &matrix["RouteMatrix"][0][0];
        assert_eq!(
            cell["Error"] == json!("NoMatchDestination"),
            !covered,
            "case {case:?}: an uncovered destination is the cell's NoMatch failure, got {cell:?}"
        );

        // CalculateRoutes reports two positions per place, and which one it reports as the
        // matched `Position` follows coverage: the point on the road where the network
        // matched one, and the submitted position itself where it held no road to match
        // onto — never a road position it never found.
        let (status, route, _) = post(
            &router,
            ROUTES,
            &json!({ "Origin": ON_LAND, "Destination": position }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {route:?}");
        let arrival = &route["Routes"][0]["Legs"][0]["VehicleLegDetails"]["Arrival"]["Place"];
        assert_eq!(
            arrival["Position"] == arrival["OriginalPosition"],
            !covered,
            "case {case:?}: an uncovered arrival is reported where it was submitted, got {arrival:?}"
        );
    }
}

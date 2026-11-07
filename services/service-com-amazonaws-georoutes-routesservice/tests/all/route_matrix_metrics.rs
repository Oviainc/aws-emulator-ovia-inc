// SPDX-License-Identifier: CC0-1.0

//! The metrics a `CalculateRouteMatrix` cell carries, and how they hang together.
//!
//! `RouteMatrixEntry.Distance` (metres) and `RouteMatrixEntry.Duration` (seconds) are the
//! total travel distance and the expected travel duration of one origin/destination pair
//! (`smithy_query(query="shape",shape="RouteMatrixEntry")`). The model states no routing
//! algorithm, so what can be asserted is how the two hang together, and these are the
//! invariants: a routed cell is never shorter than the straight line between its pair, its
//! duration is that distance travelled at a road speed, and one cost function serves the
//! whole matrix — so the speed a cell implies does not drift from cell to cell, and a
//! slower traveller never implies a faster one.
//!
//! The tables span what the rule generalises over: separation (a few hundred metres to a
//! few hundred kilometres), bearing (east-west, north-south, diagonal), travel mode and
//! routing objective.

use serde_json::{json, Value};

use crate::routing::{great_circle_m, int, post, routes_service, MATRIX};

/// Posted speeds of the classes of road this service's network carries, in km/h, as
/// `src/state.rs` holds them (`FOOTPATH_SPEED_KPH`, `STREET_SPEED_KPH`,
/// `MOTORWAY_SPEED_KPH`): nothing travels faster than the fastest road it is allowed on,
/// and a pedestrian walks at the footpath pace.
const FOOTPATH_SPEED_KPH: f64 = 5.0;
const MOTORWAY_SPEED_KPH: f64 = 90.0;

/// The fastest road `mode` may travel on, in metres per second: a pedestrian is on foot,
/// every other mode is a vehicle and the network's motorways are the fastest roads it
/// carries.
fn top_road_speed_mps(mode: &str) -> f64 {
    let kph = if mode == "Pedestrian" {
        FOOTPATH_SPEED_KPH
    } else {
        MOTORWAY_SPEED_KPH
    };
    kph * 1_000.0 / 3_600.0
}

/// One pair per row, differing in separation and bearing. Every position is in mapped
/// territory and every pair is connected by the road network, so each cell is routed.
const PAIRS: [(&str, [f64; 2], [f64; 2]); 5] = [
    ("a few hundred metres east", [-123.1, 49.28], [-123.095, 49.28]),
    ("a kilometre north", [-123.1, 49.28], [-123.1, 49.289]),
    ("a few kilometres diagonally", [-123.1, 49.28], [-123.2, 49.2]),
    ("tens of kilometres diagonally", [-122.3321, 47.6062], [-122.1, 47.3]),
    ("hundreds of kilometres", [-122.3321, 47.6062], [-122.6784, 45.5152]),
];

/// Every routed cell covers at least the straight line between its pair and takes that
/// distance at a speed of the road network: the two members are one cost, not two
/// independent numbers.
#[tokio::test]
async fn every_routed_cell_is_a_distance_and_the_time_it_takes_on_the_road_network() {
    let router = routes_service();
    let modes = ["Car", "Truck", "Scooter", "Pedestrian"];

    for mode in modes {
        for (pair, origin, destination) in PAIRS {
            let case = format!("{mode}, {pair}");
            let cell = cell(&router, origin, destination, mode, "FastestRoute", &case).await;
            let distance = int(&cell, "Distance", &case);
            let duration = int(&cell, "Duration", &case);
            let straight = great_circle_m(origin, destination);
            assert!(
                distance as f64 + 1.0 >= straight,
                "case {case:?}: a route cannot be shorter than the {straight:.0} m straight line, got {distance}"
            );
            assert!(
                duration > 0,
                "case {case:?}: travelling {distance} m takes time, got {duration}"
            );
            let implied = distance as f64 / duration as f64;
            assert!(
                implied > 0.0 && implied <= top_road_speed_mps(mode) + 0.1,
                "case {case:?}: {distance} m in {duration} s implies {implied:.1} m/s, past the {:.1} m/s the fastest road allows",
                top_road_speed_mps(mode)
            );
        }
    }
}

/// One cost function serves the whole matrix, so the speed a cell implies is the same
/// whatever the separation, the bearing or the shape of the matrix it sits in — and a
/// mode held slower never implies a faster one.
#[tokio::test]
async fn the_speed_a_cell_implies_does_not_drift_between_cells() {
    let router = routes_service();
    // Ordered fastest to slowest by the speeds this service holds per mode.
    let modes = ["Car", "Truck", "Scooter", "Pedestrian"];

    let mut slower_than: Option<(&str, f64)> = None;
    for mode in modes {
        let mut speeds: Vec<(String, f64)> = Vec::new();
        for (pair, origin, destination) in PAIRS {
            let case = format!("{mode}, {pair}");
            let cell = cell(&router, origin, destination, mode, "FastestRoute", &case).await;
            let (distance, duration) = (int(&cell, "Distance", &case), int(&cell, "Duration", &case));
            speeds.push((case, distance as f64 / duration as f64));
        }
        // A whole row at once: the cells of one request are costed by the same function as
        // the single-cell matrices above.
        let (status, answer, _) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": [{ "Position": PAIRS[0].1 }],
                "Destinations": PAIRS.iter().map(|(_, _, to)| json!({ "Position": to })).collect::<Vec<Value>>(),
                "TravelMode": mode,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {mode:?}: expected 200, got {status} with {answer:?}");
        for (index, cell) in answer["RouteMatrix"][0]
            .as_array()
            .unwrap_or_else(|| panic!("case {mode:?}: RouteMatrix is required, got {answer:?}"))
            .iter()
            .enumerate()
        {
            let case = format!("{mode}, row cell {index}");
            if !cell["Error"].is_null() {
                continue;
            }
            let (distance, duration) = (int(cell, "Distance", &case), int(cell, "Duration", &case));
            assert!(duration > 0, "case {case:?}: travelling {distance} m takes time, got {duration}");
            speeds.push((case, distance as f64 / duration as f64));
        }

        let (slowest_case, slowest) = speeds
            .iter()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .cloned()
            .expect("the table has rows");
        let (fastest_case, fastest) = speeds
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .cloned()
            .expect("the table has rows");
        // Rounding each cell to whole metres and whole seconds is the only spread the one
        // cost function leaves, which is widest on the shortest cell.
        assert!(
            fastest - slowest <= slowest * 0.1,
            "case {mode:?}: the implied speed drifts between cells: {slowest:.2} m/s in {slowest_case:?} against {fastest:.2} m/s in {fastest_case:?}"
        );
        if let Some((faster_mode, faster)) = slower_than {
            assert!(
                fastest <= faster + 0.1,
                "case {mode:?}: must not imply a higher speed than {faster_mode:?}, got {fastest:.2} m/s against {faster:.2} m/s"
            );
        }
        slower_than = Some((mode, slowest));
    }
}

/// `ShortestRoute` asks for the shorter way, so no cell of it is longer than the same
/// cell of `FastestRoute`: the objective reaches the per-cell cost.
#[tokio::test]
async fn the_routing_objective_reaches_every_matrix_cell() {
    let router = routes_service();

    for (pair, origin, destination) in PAIRS {
        let case = format!("Car, {pair}");
        let fastest = cell(&router, origin, destination, "Car", "FastestRoute", &case).await;
        let shortest = cell(&router, origin, destination, "Car", "ShortestRoute", &case).await;
        assert!(
            int(&shortest, "Distance", &case) <= int(&fastest, "Distance", &case),
            "case {case:?}: ShortestRoute must not be the longer one, got {shortest:?} against {fastest:?}"
        );
    }
}

/// The single cell of a 1x1 matrix for `origin` -> `destination`, which must be routed.
async fn cell(
    router: &smithy_server_runtime::MultiServiceRouter,
    origin: [f64; 2],
    destination: [f64; 2],
    travel_mode: &str,
    objective: &str,
    case: &str,
) -> Value {
    let (status, answer, _) = post(
        router,
        MATRIX,
        &json!({
            "Origins": [{ "Position": origin }],
            "Destinations": [{ "Position": destination }],
            "TravelMode": travel_mode,
            "OptimizeRoutingFor": objective,
        }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
    let cell = answer["RouteMatrix"][0][0].clone();
    assert!(
        cell["Error"].is_null(),
        "case {case:?}: the pair is on the road network, so it is routed, got {cell:?}"
    );
    cell
}

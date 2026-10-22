// SPDX-License-Identifier: CC0-1.0

//! The travel steps of a leg, end to end through the router.
//!
//! The rule under test is not "this request gives these steps" but an invariant over a
//! class of requests: a leg's `TravelSteps` are its manoeuvres, one per RUN of road the leg
//! travels plus its arrival, so the steps TILE the leg — their geometry offsets partition
//! its geometry, each step's distance is the length of the stretch it covers, and the steps
//! of a leg sum to exactly the leg's own distance and duration. Each table therefore spans
//! the inputs the rule generalises over: travel mode (so leg type and which roads are
//! travelled), waypoint count, routing objective, route length, step detail, measurement
//! system and requested languages.
//!
//! Every catalog written out by hand below names the model member it was read from, and is
//! exercised on values the issue did not cite.

use serde_json::{json, Value};

use crate::routing::{great_circle_m, int, post, routes_service, ROUTES};

/// Every value of `RouteVehicleTravelStepType`, from
/// `smithy_query(query="shape",shape="RouteVehicleTravelStepType")`.
const VEHICLE_STEP_TYPES: [&str; 13] = [
    "Arrive",
    "Continue",
    "ContinueHighway",
    "Depart",
    "EnterHighway",
    "Exit",
    "Keep",
    "Ramp",
    "RoundaboutEnter",
    "RoundaboutExit",
    "RoundaboutPass",
    "Turn",
    "UTurn",
];

/// Every value of `RoutePedestrianTravelStepType`, same source. It declares neither the
/// highway manoeuvres nor a U-turn, so a pedestrian leg must never report one.
const PEDESTRIAN_STEP_TYPES: [&str; 8] = [
    "Arrive",
    "Continue",
    "Depart",
    "Keep",
    "RoundaboutEnter",
    "RoundaboutExit",
    "RoundaboutPass",
    "Turn",
];

/// Every value of `RouteSteeringDirection`, same source.
const STEERING_DIRECTIONS: [&str; 3] = ["Left", "Right", "Straight"];

/// Every value of `RouteTurnIntensity`, same source.
const TURN_INTENSITIES: [&str; 3] = ["Sharp", "Slight", "Typical"];

/// Every value of `RouteRoadType`, same source.
const ROAD_TYPES: [&str; 3] = ["Highway", "Rural", "Urban"];

/// Every manoeuvre-detail member `RouteVehicleTravelStep` and `RoutePedestrianTravelStep`
/// declare, from `smithy_query(query="shape",shape="RouteVehicleTravelStep")`.
const DETAIL_MEMBERS: [&str; 11] = [
    "ContinueStepDetails",
    "ContinueHighwayStepDetails",
    "EnterHighwayStepDetails",
    "ExitStepDetails",
    "KeepStepDetails",
    "RampStepDetails",
    "RoundaboutEnterStepDetails",
    "RoundaboutExitStepDetails",
    "RoundaboutPassStepDetails",
    "TurnStepDetails",
    "UTurnStepDetails",
];

/// The detail member a step of each type carries, as those shapes name them. `Depart` and
/// `Arrive` declare none: nothing is turned through at the ends of a leg.
fn detail_member(step_type: &str) -> Option<&'static str> {
    match step_type {
        "Continue" => Some("ContinueStepDetails"),
        "ContinueHighway" => Some("ContinueHighwayStepDetails"),
        "EnterHighway" => Some("EnterHighwayStepDetails"),
        "Exit" => Some("ExitStepDetails"),
        "Keep" => Some("KeepStepDetails"),
        "Ramp" => Some("RampStepDetails"),
        "RoundaboutEnter" => Some("RoundaboutEnterStepDetails"),
        "RoundaboutExit" => Some("RoundaboutExitStepDetails"),
        "RoundaboutPass" => Some("RoundaboutPassStepDetails"),
        "Turn" => Some("TurnStepDetails"),
        "UTurn" => Some("UTurnStepDetails"),
        _ => None,
    }
}

/// The leg details of whichever leg type `leg` is.
fn leg_details(leg: &Value, case: &str) -> Value {
    for member in ["VehicleLegDetails", "PedestrianLegDetails"] {
        if leg[member].is_object() {
            return leg[member].clone();
        }
    }
    panic!("case {case:?}: the leg carries no details of its type: {leg:?}")
}

/// The `TravelSteps` of a leg.
fn travel_steps(details: &Value, case: &str) -> Vec<Value> {
    details["TravelSteps"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: TravelSteps is required, got {details:?}"))
        .clone()
}

/// The `Simple` line string of a leg, as `[longitude, latitude]` pairs.
fn line_string(leg: &Value, case: &str) -> Vec<[f64; 2]> {
    leg["Geometry"]["LineString"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a LineString geometry, got {:?}", leg["Geometry"]))
        .iter()
        .map(|position| {
            let values = position
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a position, got {position:?}"));
            [
                values[0].as_f64().unwrap_or_default(),
                values[1].as_f64().unwrap_or_default(),
            ]
        })
        .collect()
}

/// The length of a stretch of a line, in metres, by a formula independent of the one the
/// service computes with.
fn stretch_m(line: &[[f64; 2]], from: usize, to: usize) -> f64 {
    line[from..=to.min(line.len() - 1)]
        .windows(2)
        .map(|pair| great_circle_m(pair[0], pair[1]))
        .sum()
}

/// A route request over the grid, with the leg summary and the `Simple` geometry the
/// assertions below read.
fn request(origin: [f64; 2], destination: [f64; 2], travel_mode: &str, waypoints: Vec<Value>) -> Value {
    json!({
        "Origin": origin,
        "Destination": destination,
        "TravelMode": travel_mode,
        "Waypoints": waypoints,
        "LegGeometryFormat": "Simple",
        "LegAdditionalFeatures": ["Summary"],
    })
}

/// One manoeuvre per run of road, tiling the leg: whatever the travel mode, waypoint
/// count, objective, route length or step detail, every leg reports steps that start with
/// a `Depart`, end with an `Arrive` that travels nothing, carry offsets that partition the
/// leg geometry in order, state the distance of the stretch each covers, and sum to exactly
/// the leg's own distance and duration.
#[tokio::test]
async fn the_steps_of_a_leg_tile_it_for_every_shape_of_request() {
    let router = routes_service();
    let waypoint_pool = [[-123.11, 49.275], [-123.105, 49.27], [-123.115, 49.265]];

    /// name, origin, destination, travel mode, waypoint count, objective, step detail.
    type StepCase = (
        &'static str,
        [f64; 2],
        [f64; 2],
        &'static str,
        usize,
        Option<&'static str>,
        Option<&'static str>,
    );
    let cases: [StepCase; 9] = [
        (
            "the reported route, car, no step type",
            [-123.1, 49.28],
            [-123.12, 49.26],
            "Car",
            0,
            None,
            None,
        ),
        (
            "the same route with basic steps",
            [-123.1, 49.28],
            [-123.12, 49.26],
            "Car",
            0,
            None,
            Some("Default"),
        ),
        (
            "the same route turn by turn",
            [-123.1, 49.28],
            [-123.12, 49.26],
            "Car",
            0,
            None,
            Some("TurnByTurn"),
        ),
        (
            "a short hop",
            [-123.1, 49.28],
            [-123.0995, 49.2805],
            "Car",
            0,
            None,
            None,
        ),
        (
            "a long run, truck, shortest",
            [-123.2, 49.35],
            [-123.14, 49.3],
            "Truck",
            0,
            Some("ShortestRoute"),
            None,
        ),
        (
            "two stops, scooter",
            [-123.1, 49.28],
            [-123.12, 49.26],
            "Scooter",
            2,
            None,
            Some("Default"),
        ),
        (
            "three stops, car, shortest",
            [-123.1, 49.28],
            [-123.12, 49.26],
            "Car",
            3,
            Some("ShortestRoute"),
            None,
        ),
        (
            "a pedestrian leg",
            [-123.1, 49.28],
            [-123.103, 49.277],
            "Pedestrian",
            0,
            None,
            Some("Default"),
        ),
        (
            "a pedestrian leg with a waypoint, southern hemisphere",
            [151.21, -33.87],
            [151.2, -33.86],
            "Pedestrian",
            1,
            None,
            None,
        ),
    ];

    for (case, origin, destination, travel_mode, waypoint_count, objective, detail) in cases {
        let waypoints: Vec<Value> = waypoint_pool[..waypoint_count.min(waypoint_pool.len())]
            .iter()
            .map(|position| json!({ "Position": position }))
            .collect();
        let waypoints = if origin[0] > 0.0 {
            // The southern-hemisphere row needs waypoints near its own positions.
            (0..waypoint_count)
                .map(|index| json!({ "Position": [origin[0] - 0.004 * (index + 1) as f64, origin[1] + 0.004] }))
                .collect()
        } else {
            waypoints
        };
        let mut body = request(origin, destination, travel_mode, waypoints);
        if let Some(objective) = objective {
            body["OptimizeRoutingFor"] = json!(objective);
        }
        if let Some(detail) = detail {
            body["TravelStepType"] = json!(detail);
        }

        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let routes = answer["Routes"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Routes is required, got {answer:?}"));
        let legs = routes[0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {:?}", routes[0]));
        let pedestrian = travel_mode == "Pedestrian";

        for (index, leg) in legs.iter().enumerate() {
            let case = &format!("{case} / leg {index}");
            let details = leg_details(leg, case);
            let steps = travel_steps(&details, case);
            let line = line_string(leg, case);

            assert!(
                steps.len() >= 2,
                "case {case:?}: a leg departs and arrives, so it carries 2 steps at least, got {steps:?}"
            );
            assert_eq!(
                steps[0]["Type"], json!("Depart"),
                "case {case:?}: the first step departs, got {:?}",
                steps[0]["Type"]
            );
            let last = &steps[steps.len() - 1];
            assert_eq!(
                last["Type"], json!("Arrive"),
                "case {case:?}: the last step arrives, got {:?}",
                last["Type"]
            );
            assert_eq!(
                (int(last, "Distance", case), int(last, "Duration", case)),
                (0, 0),
                "case {case:?}: nothing is travelled after the arrival, got {last:?}"
            );

            // The offsets partition the geometry in order, from its first vertex to its
            // last, so the steps tile the leg rather than sampling it.
            let offsets: Vec<i64> = steps
                .iter()
                .map(|step| int(step, "GeometryOffset", case))
                .collect();
            assert_eq!(
                offsets.first().copied(),
                Some(0),
                "case {case:?}: the departure is at the start of the geometry, got {offsets:?}"
            );
            assert_eq!(
                offsets.last().copied(),
                Some(line.len() as i64 - 1),
                "case {case:?}: the arrival is at the end of the {}-vertex geometry, got {offsets:?}",
                line.len()
            );
            assert!(
                offsets.windows(2).all(|pair| pair[1] > pair[0]),
                "case {case:?}: each step starts after the one before it, got {offsets:?}"
            );

            let mut distance_total = 0;
            let mut duration_total = 0;
            for (ordinal, step) in steps.iter().enumerate() {
                let case = &format!("{case} / step {ordinal}");
                let step_type = step["Type"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: Type is required, got {step:?}"));
                let allowed: &[&str] = if pedestrian {
                    &PEDESTRIAN_STEP_TYPES
                } else {
                    &VEHICLE_STEP_TYPES
                };
                assert!(
                    allowed.contains(&step_type),
                    "case {case:?}: {step_type} is not a value of this leg type's step-type enum {allowed:?}"
                );

                let distance = int(step, "Distance", case);
                let duration = int(step, "Duration", case);
                assert!(
                    distance >= 0 && duration >= 0,
                    "case {case:?}: a step travels a non-negative distance and duration, got {distance} m in {duration} s"
                );
                distance_total += distance;
                duration_total += duration;

                // The step's distance is the length of the stretch of geometry it covers.
                let from = offsets[ordinal] as usize;
                let to = offsets.get(ordinal + 1).copied().unwrap_or(offsets[ordinal]) as usize;
                let measured = stretch_m(&line, from, to);
                assert!(
                    (measured - distance as f64).abs() <= 3.0,
                    "case {case:?}: the step covers vertices {from}..{to}, {measured:.0} m long, but reports {distance} m"
                );

                // Exactly the detail structure its type declares, and no other.
                for member in DETAIL_MEMBERS {
                    let expected = detail_member(step_type) == Some(member);
                    assert_eq!(
                        step[member].is_object(), expected,
                        "case {case:?}: a {step_type} step {} carry {member}, got {:?}",
                        if expected { "must" } else { "must not" },
                        step[member]
                    );
                }
                if let Some(member) = detail_member(step_type) {
                    let carried = &step[member];
                    assert!(
                        carried["Intersection"].is_array(),
                        "case {case:?}: {member}.Intersection is required, got {carried:?}"
                    );
                    if step_type != "Continue" {
                        let angle = carried["TurnAngle"]
                            .as_f64()
                            .unwrap_or_else(|| panic!("case {case:?}: {member}.TurnAngle is missing in {carried:?}"));
                        assert!(
                            (-180.0..=180.0).contains(&angle),
                            "case {case:?}: TurnAngle is modeled in [-180, 180], got {angle}"
                        );
                        let steering = carried["SteeringDirection"].as_str().unwrap_or_default();
                        assert!(
                            STEERING_DIRECTIONS.contains(&steering),
                            "case {case:?}: {steering:?} is not a RouteSteeringDirection"
                        );
                        let intensity = carried["TurnIntensity"].as_str().unwrap_or_default();
                        assert!(
                            TURN_INTENSITIES.contains(&intensity),
                            "case {case:?}: {intensity:?} is not a RouteTurnIntensity"
                        );
                    }
                }

                // The roads the step joins, as the network names them.
                if step["CurrentRoad"].is_object() {
                    let road = &step["CurrentRoad"];
                    let names = road["RoadName"].as_array().unwrap_or_else(|| {
                        panic!("case {case:?}: CurrentRoad.RoadName is required, got {road:?}")
                    });
                    assert!(
                        names.iter().all(|name| name["Value"].as_str().is_some_and(|value| !value.is_empty())),
                        "case {case:?}: every road name carries a value, got {names:?}"
                    );
                    let road_type = road["Type"].as_str().unwrap_or_default();
                    assert!(
                        ROAD_TYPES.contains(&road_type),
                        "case {case:?}: {road_type:?} is not a RouteRoadType"
                    );
                    assert!(
                        road["RouteNumber"].is_array() && road["Towards"].is_array(),
                        "case {case:?}: RouteNumber and Towards are required lists, got {road:?}"
                    );
                }

                // An instruction is carried for `Default` steps only, and a signpost for
                // `TurnByTurn` steps only.
                let turn_by_turn = detail == Some("TurnByTurn");
                assert_eq!(
                    step["Instruction"].as_str().is_some_and(|text| !text.is_empty()),
                    !turn_by_turn,
                    "case {case:?}: an Instruction is only available when the TravelStepType is Default, got {:?}",
                    step["Instruction"]
                );
                assert!(
                    turn_by_turn || step["Signpost"].is_null(),
                    "case {case:?}: a Signpost is applicable only for TurnByTurn steps, got {:?}",
                    step["Signpost"]
                );
            }

            // The steps of a leg sum to exactly the leg.
            let summary = &details["Summary"]["Overview"];
            assert_eq!(
                distance_total,
                int(summary, "Distance", case),
                "case {case:?}: the step distances must sum to the leg's distance, got {distance_total}"
            );
            assert_eq!(
                duration_total,
                int(summary, "Duration", case),
                "case {case:?}: the step durations must sum to the leg's duration, got {duration_total}"
            );
        }
    }
}

/// A turn-by-turn step carries the manoeuvre members the model marks as its own: a
/// signpost wherever the network has a junction to read one at, and no instruction.
#[tokio::test]
async fn turn_by_turn_steps_carry_the_signpost_of_their_junction() {
    let router = routes_service();
    let cases = [
        ("a car route", [-123.1, 49.28], [-123.12, 49.26], "Car"),
        ("a truck route", [-123.2, 49.35], [-123.17, 49.33], "Truck"),
        ("a pedestrian route", [-123.1, 49.28], [-123.112, 49.268], "Pedestrian"),
    ];

    for (case, origin, destination, travel_mode) in cases {
        let mut body = request(origin, destination, travel_mode, Vec::new());
        body["TravelStepType"] = json!("TurnByTurn");
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let legs = answer["Routes"][0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {answer:?}"))
            .clone();
        let mut signposted = 0;
        for leg in &legs {
            for step in travel_steps(&leg_details(leg, case), case) {
                assert!(
                    step["Instruction"].is_null(),
                    "case {case:?}: a TurnByTurn step carries no Instruction, got {:?}",
                    step["Instruction"]
                );
                let intersects = DETAIL_MEMBERS
                    .iter()
                    .filter_map(|member| step[*member]["Intersection"].as_array())
                    .any(|names| !names.is_empty());
                if intersects {
                    // The step is performed in a junction of two roads, so it is signed by
                    // the road it leads onto.
                    let labels = step["Signpost"]["Labels"].as_array().unwrap_or_else(|| {
                        panic!("case {case:?}: a step performed in a junction carries a Signpost, got {step:?}")
                    });
                    assert!(
                        labels
                            .iter()
                            .all(|label| label["Text"]["Value"].as_str().is_some_and(|text| !text.is_empty())),
                        "case {case:?}: every signpost label carries its text, got {labels:?}"
                    );
                    signposted += 1;
                }
            }
        }
        assert!(
            signposted > 0,
            "case {case:?}: a route over the grid crosses roads, so some step is performed in a junction"
        );
    }
}

/// An instruction states its step's distance in the measurement system the request asked
/// for: metres and kilometres for `Metric`, feet and miles for `Imperial`.
#[tokio::test]
async fn an_instruction_states_its_distance_in_the_requested_measurement_system() {
    let router = routes_service();
    // Rows differ in how far a step runs, which is what decides the unit within a system.
    let cases = [
        ("a short hop", [-123.1, 49.28], [-123.0995, 49.2805]),
        ("the reported route", [-123.1, 49.28], [-123.12, 49.26]),
        ("a long run", [-123.3, 49.4], [-123.2, 49.33]),
    ];

    for (case, origin, destination) in cases {
        let mut rendered: Vec<(&str, Vec<String>)> = Vec::new();
        for system in ["Metric", "Imperial"] {
            let mut body = request(origin, destination, "Car", Vec::new());
            body["InstructionsMeasurementSystem"] = json!(system);
            let (status, answer, _) = post(&router, ROUTES, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?} / {system}: expected 200, got {status} with {answer:?}"
            );
            let instructions: Vec<String> = answer["Routes"][0]["Legs"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {answer:?}"))
                .iter()
                .flat_map(|leg| travel_steps(&leg_details(leg, case), case))
                .filter_map(|step| step["Instruction"].as_str().map(ToString::to_string))
                .collect();
            rendered.push((system, instructions));
        }

        for (system, instructions) in &rendered {
            assert!(
                !instructions.is_empty(),
                "case {case:?} / {system}: a Default step carries an instruction"
            );
            // The arrival states no distance; every travelling step states one, in the
            // units of the requested system and of no other.
            let units: [&str; 2] = match *system {
                "Metric" => [" m.", " km."],
                _ => [" ft.", " mi."],
            };
            let foreign: [&str; 2] = match *system {
                "Metric" => [" ft.", " mi."],
                _ => [" m.", " km."],
            };
            for instruction in instructions {
                if instruction.starts_with("Arrive") {
                    continue;
                }
                assert!(
                    units.iter().any(|unit| instruction.ends_with(unit)),
                    "case {case:?} / {system}: {instruction:?} must state its distance in {units:?}"
                );
                assert!(
                    !foreign.iter().any(|unit| instruction.ends_with(unit)),
                    "case {case:?} / {system}: {instruction:?} must not state a distance in {foreign:?}"
                );
            }
        }

        let metric = &rendered[0].1;
        let imperial = &rendered[1].1;
        assert_ne!(
            metric, imperial,
            "case {case:?}: the two measurement systems state the same distances differently"
        );
        assert_eq!(
            metric.len(),
            imperial.len(),
            "case {case:?}: the measurement system changes the text of a step, not how many there are"
        );
    }
}

/// The instructions are rendered in a requested language this emulator holds text in, and
/// otherwise in the regional default — which the answer reports with the model's own
/// `MainLanguageNotFound` notice rather than passing off as a translation.
#[tokio::test]
async fn a_language_the_instructions_are_not_held_in_is_reported_as_a_notice() {
    let router = routes_service();
    // Rows differ in the languages asked for: the one the text is held in, a regional
    // variant of it, one it is not held in, and lists mixing them.
    // name, requested `Languages`, the language tag the strings must carry, whether the
    // answer must carry the notice.
    let cases: [(&str, Option<Vec<&str>>, &str, bool); 6] = [
        ("no language asked for", None, "en", false),
        ("the language the text is held in", Some(vec!["en"]), "en", false),
        ("a regional variant of it", Some(vec!["en-GB"]), "en-GB", false),
        ("a language it is not held in", Some(vec!["de"]), "en", true),
        ("several, none of them held", Some(vec!["fr-CA", "ja"]), "en", true),
        ("several, one of them held", Some(vec!["ja", "en-US"]), "en-US", false),
    ];

    for (case, languages, expected_tag, expects_notice) in cases {
        let mut body = request([-123.1, 49.28], [-123.12, 49.26], "Car", Vec::new());
        if let Some(languages) = &languages {
            body["Languages"] = json!(languages);
        }
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let notices = answer["Notices"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Notices is required, got {answer:?}"));
        let found = notices
            .iter()
            .any(|notice| notice["Code"] == json!("MainLanguageNotFound"));
        assert_eq!(
            found, expects_notice,
            "case {case:?}: MainLanguageNotFound must be reported for {languages:?}, got {notices:?}"
        );

        let mut tagged = 0;
        for leg in answer["Routes"][0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {answer:?}"))
        {
            for step in travel_steps(&leg_details(leg, case), case) {
                for name in step["CurrentRoad"]["RoadName"].as_array().unwrap_or(&Vec::new()) {
                    assert_eq!(
                        name["Language"], json!(expected_tag),
                        "case {case:?}: a localized name carries the language it was rendered in, got {name:?}"
                    );
                    tagged += 1;
                }
            }
        }
        assert!(
            tagged > 0,
            "case {case:?}: the steps of a route over the grid name the roads they travel"
        );
    }
}

/// The stop at the end of a leg is the step performed after its travel: a vehicle parks and
/// a pedestrian waits for the waypoint's `StopDuration`, and a leg with no stop at its end
/// carries no after-travel step.
#[tokio::test]
async fn a_waypoint_stop_is_the_after_travel_step_of_its_leg() {
    let router = routes_service();
    // name, travel mode, the step type that mode performs, the stop durations of the
    // waypoints between the origin and the destination.
    let cases: [(&str, &str, &str, Vec<i64>); 5] = [
        ("no waypoint at all", "Car", "Park", Vec::new()),
        ("one waypoint, no stop", "Car", "Park", vec![0]),
        ("one waypoint, a ten-minute stop", "Car", "Park", vec![600]),
        ("two waypoints, one stopped at", "Truck", "Park", vec![0, 45]),
        ("a pedestrian waiting at two waypoints", "Pedestrian", "Wait", vec![30, 90]),
    ];

    for (case, travel_mode, step_type, stops) in cases {
        let waypoints: Vec<Value> = stops
            .iter()
            .enumerate()
            .map(|(index, stop)| {
                json!({
                    "Position": [-123.1 - 0.004 * (index + 1) as f64, 49.28 - 0.004 * (index + 1) as f64],
                    "StopDuration": stop,
                })
            })
            .collect();
        let body = request([-123.1, 49.28], [-123.12, 49.26], travel_mode, waypoints);
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let legs = answer["Routes"][0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {answer:?}"))
            .clone();
        assert_eq!(
            legs.len(),
            stops.len() + 1,
            "case {case:?}: a stop at each waypoint gives one leg more than there are waypoints, got {}",
            legs.len()
        );

        for (index, leg) in legs.iter().enumerate() {
            let case = &format!("{case} / leg {index}");
            let details = leg_details(leg, case);
            let after = details["AfterTravelSteps"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: AfterTravelSteps is required, got {details:?}"))
                .clone();
            // The last leg ends at the destination, which no waypoint stop applies to.
            let expected = stops.get(index).copied().unwrap_or(0);
            if expected == 0 {
                assert!(
                    after.is_empty(),
                    "case {case:?}: nothing is performed after a leg with no stop at its end, got {after:?}"
                );
                continue;
            }
            assert_eq!(
                after.len(),
                1,
                "case {case:?}: one stop at the end of the leg gives one after-travel step, got {after:?}"
            );
            assert_eq!(
                after[0]["Type"], json!(step_type),
                "case {case:?}: a {travel_mode} leg performs a {step_type}, got {:?}",
                after[0]["Type"]
            );
            assert_eq!(
                int(&after[0], "Duration", case),
                expected,
                "case {case:?}: the step lasts the waypoint's StopDuration of {expected} s"
            );
            assert!(
                after[0]["Instruction"].as_str().is_some_and(|text| !text.is_empty()),
                "case {case:?}: a Default after-travel step carries its instruction, got {:?}",
                after[0]["Instruction"]
            );
        }
    }
}

/// `TravelStepInstructions` is a leg feature this slice computes, so a request naming it is
/// answered with the steps it asks for rather than declined; a feature the road model holds
/// no data for is still declined.
#[tokio::test]
async fn the_travel_step_instructions_leg_feature_is_answered() {
    let router = routes_service();
    // Rows differ along the feature list: the step instructions alone, beside a feature
    // that is computed, and beside one that is not.
    let cases: [(&str, Value, u16); 5] = [
        ("no feature asked for", json!([]), 200),
        ("the step instructions alone", json!(["TravelStepInstructions"]), 200),
        (
            "the step instructions beside the summary",
            json!(["Summary", "TravelStepInstructions"]),
            200,
        ),
        ("a feature the road model has no data for", json!(["Elevation"]), 501),
        (
            "the step instructions beside one it has no data for",
            json!(["TravelStepInstructions", "Zones"]),
            501,
        ),
    ];

    for (case, features, expected) in cases {
        let mut body = request([-123.1, 49.28], [-123.12, 49.26], "Car", Vec::new());
        body["LegAdditionalFeatures"] = features.clone();
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected} for {features}, got {status} with {answer:?}"
        );
        if expected != 200 {
            continue;
        }
        for leg in answer["Routes"][0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {answer:?}"))
        {
            let steps = travel_steps(&leg_details(leg, case), case);
            assert!(
                steps
                    .iter()
                    .all(|step| step["Instruction"].as_str().is_some_and(|text| !text.is_empty())),
                "case {case:?}: every step of a Default answer carries its instruction, got {steps:?}"
            );
        }
    }
}

/// A pass-through waypoint does not end a manoeuvre: it splits no leg, so the leg it falls
/// inside still reports one run of road per step, tiling the whole merged geometry.
#[tokio::test]
async fn a_pass_through_waypoint_does_not_cut_a_step() {
    let router = routes_service();
    let cases = [
        ("one pass-through", vec![[-123.105, 49.274]]),
        ("two pass-throughs", vec![[-123.104, 49.276], [-123.112, 49.267]]),
    ];

    for (case, through) in cases {
        let waypoints: Vec<Value> = through
            .iter()
            .map(|position| json!({ "Position": position, "PassThrough": true }))
            .collect();
        let body = request([-123.1, 49.28], [-123.12, 49.26], "Car", waypoints);
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let legs = answer["Routes"][0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {answer:?}"))
            .clone();
        assert_eq!(
            legs.len(),
            1,
            "case {case:?}: a pass-through waypoint does not split the route, got {} legs",
            legs.len()
        );

        let leg = &legs[0];
        let details = leg_details(leg, case);
        let steps = travel_steps(&details, case);
        let line = line_string(leg, case);
        let summary = &details["Summary"]["Overview"];

        assert_eq!(
            steps
                .last()
                .and_then(|step| step["GeometryOffset"].as_i64()),
            Some(line.len() as i64 - 1),
            "case {case:?}: the arrival is at the end of the merged geometry of {} vertices, got {steps:?}",
            line.len()
        );
        let distance_total: i64 = steps.iter().map(|step| int(step, "Distance", case)).sum();
        assert_eq!(
            distance_total,
            int(summary, "Distance", case),
            "case {case:?}: the steps of the merged leg sum to its distance, got {distance_total}"
        );
    }
}

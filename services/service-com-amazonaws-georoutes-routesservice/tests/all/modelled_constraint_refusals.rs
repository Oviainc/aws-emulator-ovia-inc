// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal of a rule the MODEL carries, and the one cross-member
//! rule the model cannot carry about its per-mode options.
//!
//! Two invariants, each asserted over a table that spans what it generalises over rather
//! than over one recorded request:
//!
//! * a value that breaks a constraint the model declares on a member (`@range`, `@length`,
//!   `@pattern`, an enumerated value set, `@required`) is refused with a sentence that names
//!   that member the way the MODEL spells it — single-quoted, dotted through nested
//!   structures, subscripted where the offender is an element of a list — states the bound,
//!   expression or set it broke, and quotes no part of the rejected value. Such a value is
//!   stopped at the service boundary before any handler runs, so this is the wording of the
//!   boundary refusal;
//! * a `TravelModeOptions` block belongs to the single travel mode its member names, so a
//!   request may set only the block the `TravelMode` it selected names. Where the
//!   operation's own options structure declares a block for that mode the refusal states
//!   which blocks may be set; where it declares none, there is no permitted set to state and
//!   the refusal names the block that must not be set.
//!
//! Each table's rows differ along the member, the bound broken, the nesting (top level,
//! nested structure, list element), the operation called, and — for the per-mode rule — the
//! mode selected, the block set and which of the four operations' differently-shaped options
//! structures is being held to it, including the combinations that must be SERVED.

use serde_json::{json, Value};

use service_com_amazonaws_georoutes_routesservice::ops::types::GEOMETRY_FORMAT_VALUES;

use crate::routing::{post, routes_service, ROUTES, SNAP};

/// The paths of the operations these tables call. `routing.rs` exports the two this crate's
/// other suites share; the rest are named here.
const MATRIX: &str = "/v2/route-matrix";
const OPTIMIZE: &str = "/v2/optimize-waypoints";
const ISOLINES: &str = "/v2/isolines";

/// A position well inside the WGS 84 domain and on this service's network.
const GOOD: [f64; 2] = [-123.1, 49.28];

/// `TimestampWithTimezoneOffset`'s `@pattern`, verbatim from the model
/// (`smithy_query(query="shape", shape="com.amazonaws.georoutes#CalculateRoutesRequest")`,
/// member `DepartureTime`): what a refusal of that member states the value had to match.
const TIMESTAMP_PATTERN: &str = r"^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?(Z|[+-]([01][0-9]|2[0-3]):[0-5][0-9])$";

/// One row of a table that posts a body to an operation's path, expecting either the stated
/// refusal or a served request.
type Row = (&'static str, &'static str, Value, Option<String>);

/// The message of a 400 refusal, asserting the parts every refusal of this service carries
/// whether it was found at the boundary or inside a handler: the status, and the modeled
/// `ValidationException` as the error code clients branch on.
fn refusal_message(case: &str, status: u16, answer: &Value) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 refusal, got {status} with {answer:?}"
    );
    assert_eq!(
        answer["__type"].as_str(),
        Some("ValidationException"),
        "case {case:?}: a refused request is the modeled ValidationException, got {answer:?}"
    );
    answer["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: message is required, got {answer:?}"))
        .to_string()
}

/// A member's modelled constraint, broken, is refused by the member the model names and the
/// bound it broke — and a value ON a bound is inside it.
#[tokio::test]
async fn a_modelled_constraint_is_refused_by_the_member_and_the_bound_it_broke() {
    let router = routes_service();
    let route = |extra: Value| -> Value {
        let mut body = json!({ "Origin": GOOD, "Destination": [-123.12, 49.26] });
        for (member, value) in extra.as_object().expect("an object of members") {
            body[member] = value.clone();
        }
        body
    };
    let languages = |count: usize| -> Value {
        Value::Array((0..count).map(|index| json!(format!("e{index}"))).collect())
    };
    let thresholds = |count: usize| -> Value {
        json!({
            "Origin": GOOD,
            "Thresholds": { "Time": (0..count).map(|index| json!(60 + index as i64)).collect::<Vec<_>>() }
        })
    };
    // The accepted sets are read off the model: `GeometryFormat` from the generated value
    // list, and `RouteTravelMode` in the order the model DECLARES its members
    // (`smithy_query(query="shape", shape="com.amazonaws.georoutes#RouteTravelMode")` ->
    // `["Car", "Pedestrian", "Scooter", "Truck", "Intermodal", "Transit"]`), which is the
    // order a refusal states them in.
    let one_of = |member: &str, accepted: &[&str]| -> Option<String> {
        Some(format!("'{member}' must be one of: {}", accepted.join(", ")))
    };
    const ROUTE_TRAVEL_MODES: [&str; 6] = [
        "Car",
        "Pedestrian",
        "Scooter",
        "Truck",
        "Intermodal",
        "Transit",
    ];
    let cases: [Row; 16] = [
        // `@range` on `MaxAlternatives` (Integer, 0..5): both bounds, and a value on each.
        (
            "an alternative count past its maximum",
            ROUTES,
            route(json!({ "MaxAlternatives": 6 })),
            Some("'MaxAlternatives' must not be greater than 5".to_string()),
        ),
        (
            "an alternative count below its minimum",
            ROUTES,
            route(json!({ "MaxAlternatives": -1 })),
            Some("'MaxAlternatives' must not be less than 0".to_string()),
        ),
        (
            "an alternative count on its maximum",
            ROUTES,
            route(json!({ "MaxAlternatives": 5 })),
            None,
        ),
        // `@length` on a list member (`Languages`, max 10), and on the list's own element
        // (`LanguageTag`, 2..35) — where the offender is subscripted by its position.
        (
            "more languages than the member allows",
            ROUTES,
            route(json!({ "Languages": languages(11) })),
            Some("'Languages' must have length at most 10".to_string()),
        ),
        (
            "as many languages as the member allows",
            ROUTES,
            route(json!({ "Languages": languages(10) })),
            None,
        ),
        (
            "a language tag shorter than an element may be",
            ROUTES,
            route(json!({ "Languages": ["en", "x"] })),
            Some("'Languages[1]' must have length at least 2".to_string()),
        ),
        // `@length` on a nested structure's member, named through the structure.
        (
            "more isoline time thresholds than the nested member allows",
            ISOLINES,
            thresholds(6),
            Some("'Thresholds.Time' must have length at most 5".to_string()),
        ),
        (
            "no isoline time threshold at all",
            ISOLINES,
            thresholds(0),
            Some("'Thresholds.Time' must have length at least 1".to_string()),
        ),
        // `@length 2..2` on a position, broken from both ends.
        (
            "a position of three coordinates",
            ROUTES,
            route(json!({ "Origin": [-123.1, 49.28, 70.0] })),
            Some("'Origin' must have length at most 2".to_string()),
        ),
        (
            "a position of one coordinate",
            ROUTES,
            route(json!({ "Destination": [-123.1] })),
            Some("'Destination' must have length at least 2".to_string()),
        ),
        (
            "fewer trace points than the member allows",
            SNAP,
            json!({ "TracePoints": [{ "Position": GOOD }] }),
            Some("'TracePoints' must have length at least 2".to_string()),
        ),
        // `@pattern`, stated as the expression the value had to match — on two members that
        // target the same patterned shape.
        (
            "a departure time that is not a timestamp",
            ROUTES,
            route(json!({ "DepartureTime": "not-a-timestamp" })),
            Some(format!("'DepartureTime' must match regex {TIMESTAMP_PATTERN}")),
        ),
        (
            "an arrival time with a month that does not exist",
            ROUTES,
            route(json!({ "ArrivalTime": "2024-13-01T00:00:00Z" })),
            Some(format!("'ArrivalTime' must match regex {TIMESTAMP_PATTERN}")),
        ),
        // An enumerated member, refused by the set the model declares for it.
        (
            "a geometry format the enum does not declare",
            ROUTES,
            route(json!({ "LegGeometryFormat": "Polyline" })),
            one_of("LegGeometryFormat", GEOMETRY_FORMAT_VALUES),
        ),
        (
            "a travel mode the enum does not declare",
            ROUTES,
            route(json!({ "TravelMode": "Bicycle" })),
            one_of("TravelMode", &ROUTE_TRAVEL_MODES),
        ),
        // A `@required` member the request left out.
        (
            "a route with no destination at all",
            ROUTES,
            json!({ "Origin": GOOD }),
            Some("'Destination' must not be null.".to_string()),
        ),
    ];

    for (case, path, body, refused) in cases {
        let (status, answer, _) = post(&router, path, &body).await;
        let Some(expected) = refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: a value inside every modelled bound must be served, got {status} with {answer:?}"
            );
            continue;
        };
        let message = refusal_message(case, status, &answer);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        for leaked in ["validation error detected", "failed to satisfy constraint"] {
            assert!(
                !message.contains(leaked),
                "case {case:?}: the refusal must not be worded by the generic constraint \
                 renderer, got {message:?}"
            );
        }
    }
}

/// A `TravelModeOptions` block may be set only for the travel mode its member names: the
/// options of another mode cannot be applied to the selected one, so the request is refused
/// rather than answered with a journey computed for a different vehicle.
///
/// A car route is the one mode that permits a second block beside its own — the service's
/// electric-vehicle options, `Ev` — so the sentence for it states both.
#[tokio::test]
async fn per_mode_options_may_be_set_only_for_the_mode_they_name() {
    let router = routes_service();
    // The sentence used where this operation's options structure DOES declare a block for
    // the selected mode: it states the blocks that may be set.
    let only = |mode: &str, permitted: &[&str]| -> Option<String> {
        let permitted: Vec<String> = permitted.iter().map(|name| format!("'{name}'")).collect();
        Some(format!(
            "Only the following fields may be set within 'TravelModeOptions' when \
             'TravelMode' has value {mode}: {}.",
            permitted.join(", ")
        ))
    };
    // The sentence used where it declares NONE for that mode: there is no permitted set to
    // state, so the refusal names the block that must not be set.
    let not_set = |block: &str, mode: &str| -> Option<String> {
        Some(format!(
            "'TravelModeOptions.{block}' must not be set when 'TravelMode' has value {mode}."
        ))
    };
    let route = |mode: Option<&str>, options: Value| -> Value {
        let mut body = json!({
            "Origin": GOOD,
            "Destination": [-123.12, 49.26],
            "TravelModeOptions": options,
        });
        if let Some(mode) = mode {
            body["TravelMode"] = json!(mode);
        }
        body
    };
    let matrix = |mode: &str, options: Value| -> Value {
        json!({
            "Origins": [{ "Position": GOOD }],
            "Destinations": [{ "Position": [-123.12, 49.26] }],
            "TravelMode": mode,
            "TravelModeOptions": options,
        })
    };
    let snap = |mode: &str, options: Value| -> Value {
        json!({
            "TracePoints": [{ "Position": GOOD }, { "Position": [-123.11, 49.27] }],
            "TravelMode": mode,
            "TravelModeOptions": options,
        })
    };
    let optimize = |mode: &str, options: Value| -> Value {
        json!({
            "Origin": GOOD,
            "Destination": [-123.12, 49.26],
            "TravelMode": mode,
            "TravelModeOptions": options,
        })
    };
    // Rows span the four operations (whose options structures declare different blocks:
    // six for a route, three for a matrix, two for an optimization, one for a snap), the
    // mode selected including the documented default, which block the request set, and the
    // combinations that match and must therefore be served.
    let cases: [Row; 14] = [
        (
            "a route for a car carrying truck options",
            ROUTES,
            route(Some("Car"), json!({ "Truck": {} })),
            only("Car", &["Car", "Ev"]),
        ),
        (
            "a route for the default mode carrying pedestrian options",
            ROUTES,
            route(None, json!({ "Pedestrian": {} })),
            only("Car", &["Car", "Ev"]),
        ),
        (
            "a route for a scooter carrying car options",
            ROUTES,
            route(Some("Scooter"), json!({ "Car": {} })),
            only("Scooter", &["Scooter"]),
        ),
        (
            "a route for a truck carrying truck options",
            ROUTES,
            route(Some("Truck"), json!({ "Truck": { "GrossWeight": 10000 } })),
            None,
        ),
        (
            "a route for a pedestrian carrying pedestrian options",
            ROUTES,
            route(Some("Pedestrian"), json!({ "Pedestrian": { "Speed": 4.5 } })),
            None,
        ),
        (
            "a route with an options structure that sets no block",
            ROUTES,
            route(Some("Car"), json!({})),
            None,
        ),
        (
            "a matrix for a pedestrian carrying truck options",
            MATRIX,
            matrix("Pedestrian", json!({ "Truck": { "GrossWeight": 10000 } })),
            not_set("Truck", "Pedestrian"),
        ),
        (
            "a matrix for a truck carrying car options",
            MATRIX,
            matrix("Truck", json!({ "Car": {} })),
            only("Truck", &["Truck"]),
        ),
        (
            "a matrix for a car carrying car options",
            MATRIX,
            matrix("Car", json!({ "Car": {} })),
            None,
        ),
        (
            "a snap for a car carrying truck options",
            SNAP,
            snap("Car", json!({ "Truck": {} })),
            not_set("Truck", "Car"),
        ),
        (
            "a snap for a truck carrying truck options",
            SNAP,
            snap("Truck", json!({ "Truck": {} })),
            None,
        ),
        (
            "an optimization for a car carrying truck options",
            OPTIMIZE,
            optimize("Car", json!({ "Truck": {} })),
            not_set("Truck", "Car"),
        ),
        (
            "an optimization for a pedestrian carrying truck options",
            OPTIMIZE,
            optimize("Pedestrian", json!({ "Truck": {} })),
            only("Pedestrian", &["Pedestrian"]),
        ),
        (
            "an optimization for a truck carrying truck options",
            OPTIMIZE,
            optimize("Truck", json!({ "Truck": {} })),
            None,
        ),
    ];

    for (case, path, body, refused) in cases {
        let (status, answer, _) = post(&router, path, &body).await;
        let Some(expected) = refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: options that name the selected mode must be served, got {status} with {answer:?}"
            );
            continue;
        };
        let message = refusal_message(case, status, &answer);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // A refusal a handler raises carries the model's required `reason` and `fieldList`,
        // and the entry names the offending block so a client can point at the input it
        // must drop.
        assert_eq!(
            answer["reason"].as_str(),
            Some("FieldValidationFailed"),
            "case {case:?}: a broken field rule is reported as FieldValidationFailed, got {answer:?}"
        );
        let fields = answer["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: fieldList is required, got {answer:?}"));
        assert_eq!(
            fields.len(),
            1,
            "case {case:?}: one broken rule is one fieldList entry, got {fields:?}"
        );
        assert_eq!(
            fields[0]["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: the fieldList entry restates the message, got {fields:?}"
        );
        let named = fields[0]["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: fieldList[0].name is required, got {fields:?}"));
        assert!(
            named.starts_with("TravelModeOptions."),
            "case {case:?}: expected the offending option block to be named, got {named:?}"
        );
    }
}

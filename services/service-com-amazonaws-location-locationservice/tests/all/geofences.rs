// SPDX-License-Identifier: CC0-1.0

//! Geofences: the batch write and the read that proves what it stored.
//!
//! RULE: a geofence write stores every entry whose `Geometry` satisfies the
//! service's own geometry constraints and reports each entry that does not as a
//! per-entry `BatchItemError` with `Code` `"ValidationError"` inside a 200, storing
//! nothing for it; an entry is stored under its `GeofenceId` in the named
//! collection, so a put for an id the collection already holds UPDATES that
//! geofence (its `CreateTime` stays, its `UpdateTime` moves) instead of adding a
//! second one. The geometry constraints come from the model
//! (`smithy_query(query="shape",shape="GeofenceGeometry")`): exactly one of
//! `Polygon`, `Circle`, `MultiPolygon` or `Geobuf` names the geometry, every linear
//! ring is closed and holds at least 4 vertices, no geometry holds more than 1,000
//! vertices, every vertex is a WGS 84 coordinate, and a `Circle`'s radius is
//! greater than zero and no larger than 100,000 metres.
//!
//! The tables span what the rule generalises over: the geometry member a geofence
//! is defined with (circle, single-ring polygon, polygon with an interior ring,
//! multipolygon, Geobuf blob), how many properties it carries (0..3, the modelled
//! bound), which constraint an entry violates, and where the violating entry sits
//! in the batch. The request the issue report replays — one valid entry beside one
//! invalid one — is one row of the partial-failure table.
//!
//! Every call goes through the shared harness (`crate::harness`), so each one is a
//! separate HTTP request against one shared engine: a geometry the read reports
//! survived because it was PERSISTED, not because a handler echoed its input back.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one HTTP harness this crate's suites share, declared from `tests/all.rs`.
use crate::harness::{call, emulator, millis_of, Answer};

/// The `BatchItemError$Code` every per-entry refusal carries: the
/// `BatchItemErrorCode` member for "Input fails to satisfy the constraints specified
/// by the service".
const VALIDATION_ERROR: &str = "ValidationError";

async fn create_collection(router: &MultiServiceRouter, name: &str) {
    let answer = call(
        router,
        "POST",
        "/geofencing/v0/collections",
        Some(json!({ "CollectionName": name })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "CreateGeofenceCollection {name:?} must succeed, got {} {:?}",
        answer.status, answer.body
    );
}

async fn batch_put(router: &MultiServiceRouter, collection: &str, entries: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/geofencing/v0/collections/{collection}/put-geofences"),
        Some(json!({ "Entries": entries })),
    )
    .await
}

async fn get_geofence(router: &MultiServiceRouter, collection: &str, geofence: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/geofencing/v0/collections/{collection}/geofences/{geofence}"),
        None,
    )
    .await
}

async fn geofence_count(router: &MultiServiceRouter, collection: &str) -> i64 {
    let described = call(
        router,
        "GET",
        &format!("/geofencing/v0/collections/{collection}"),
        None,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "DescribeGeofenceCollection {collection:?} must succeed, got {} {:?}",
        described.status, described.body
    );
    described
        .json()
        .get("GeofenceCount")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("DescribeGeofenceCollection must report a GeofenceCount, got {:?}", described.body))
}

/// The same JSON with every number rendered as a float, so a table's `1000.0`
/// compares equal to a wire `1000` whichever way a serializer spelled it.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().unwrap_or_default()),
        Value::Array(items) => Value::Array(items.iter().map(normalized).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(k, v)| (k.clone(), normalized(v)))
                .collect::<Map<String, Value>>(),
        ),
        other => other.clone(),
    }
}

/// The `Successes` / `Errors` list of a batch answer.
fn list(case: &str, body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} list, got {body}"))
        .clone()
}

/// A geometry made of one closed, counter-clockwise ring around a point.
fn square(longitude: f64, latitude: f64) -> Value {
    json!([[
        [longitude, latitude],
        [longitude + 0.01, latitude],
        [longitude + 0.01, latitude + 0.01],
        [longitude, latitude],
    ]])
}

/// One row of the write-then-read table: the geometry member the geofence is
/// defined with, and the properties it is put with.
struct GeofenceCase {
    case: &'static str,
    geofence_id: &'static str,
    geometry: fn() -> Value,
    properties: Option<Value>,
}

/// The write-then-read table. Rows differ along the geometry member (every one the
/// model declares) and the number of properties (0, 1 and the modelled maximum of
/// 3), so what the read reports comes from the store in each shape of geometry
/// rather than from one.
fn geofence_cases() -> Vec<GeofenceCase> {
    vec![
        GeofenceCase {
            case: "circle, no properties",
            geofence_id: "store-front",
            geometry: || json!({ "Circle": { "Center": [-122.33, 47.60], "Radius": 500.0 } }),
            properties: None,
        },
        GeofenceCase {
            case: "circle at the largest radius the member admits",
            geofence_id: "metro.area-1",
            // `Circle$Radius`: "no larger than 100,000 (100 kilometers)" — the
            // published bound itself is accepted.
            geometry: || json!({ "Circle": { "Center": [0.0, 0.0], "Radius": 100_000.0 } }),
            properties: Some(json!({ "zone": "metro" })),
        },
        GeofenceCase {
            case: "single-ring polygon, three properties",
            geofence_id: "warehouse",
            geometry: || json!({ "Polygon": square(-122.4, 47.6) }),
            // `PropertyMap` is `len 0..3`: the modelled maximum.
            properties: Some(json!({ "team": "logistics", "env": "prod", "tier": "1" })),
        },
        GeofenceCase {
            case: "polygon with an interior ring",
            geofence_id: "campus_2",
            geometry: || {
                json!({
                    "Polygon": [
                        // Exterior ring, counter-clockwise, closed.
                        [[-0.02, -0.02], [0.02, -0.02], [0.02, 0.02], [-0.02, 0.02], [-0.02, -0.02]],
                        // Interior ring, clockwise, closed.
                        [[-0.01, -0.01], [-0.01, 0.01], [0.01, 0.01], [0.01, -0.01], [-0.01, -0.01]],
                    ]
                })
            },
            properties: None,
        },
        GeofenceCase {
            case: "multipolygon of two polygons",
            geofence_id: "two-sites",
            geometry: || json!({ "MultiPolygon": [square(10.0, 10.0), square(-10.0, -10.0)] }),
            properties: Some(json!({ "sites": "2" })),
        },
        GeofenceCase {
            case: "geobuf blob",
            geofence_id: "encoded.fence",
            // A blob is base64 on the restJson1 wire; the bytes are stored and
            // reported back as given, so any non-empty encoding round-trips.
            geometry: || json!({ "Geobuf": "Z2VvYnVm" }),
            properties: None,
        },
        GeofenceCase {
            case: "a ring against the northern and eastern domain bounds",
            geofence_id: "north-east",
            // The published bounds of each axis (±180 longitude, ±90 latitude) are
            // INSIDE the domain, so a ring may touch them.
            geometry: || {
                json!({
                    "Polygon": [[
                        [179.98, 89.98],
                        [180.0, 89.98],
                        [180.0, 90.0],
                        [179.98, 89.98],
                    ]]
                })
            },
            properties: None,
        },
        GeofenceCase {
            case: "a ring against the southern and western domain bounds",
            geofence_id: "south-west",
            geometry: || {
                json!({
                    "Polygon": [[
                        [-180.0, -90.0],
                        [-179.98, -90.0],
                        [-179.98, -89.98],
                        [-180.0, -90.0],
                    ]]
                })
            },
            properties: None,
        },
    ]
}

/// The entry a row of the table sends.
fn entry(case: &GeofenceCase) -> Value {
    let mut value = json!({
        "GeofenceId": case.geofence_id,
        "Geometry": (case.geometry)(),
    });
    if let Some(properties) = &case.properties {
        value["GeofenceProperties"] = properties.clone();
    }
    value
}

#[tokio::test]
async fn batch_put_then_get_round_trips_every_geometry_kind() {
    let router = emulator();
    create_collection(&router, "fences").await;

    for case in geofence_cases() {
        let answered = batch_put(&router, "fences", json!([entry(&case)])).await;
        assert_eq!(
            answered.status, 200,
            "case {:?}: BatchPutGeofence must answer 200, got {} {:?}",
            case.case, answered.status, answered.body
        );
        let body = answered.json();
        assert!(
            list(case.case, &body, "Errors").is_empty(),
            "case {:?}: a valid entry must not be reported, got {:?}",
            case.case,
            body
        );
        let successes = list(case.case, &body, "Successes");
        assert_eq!(
            successes.len(),
            1,
            "case {:?}: expected one success, got {successes:?}",
            case.case
        );
        assert_eq!(
            successes[0].get("GeofenceId").and_then(Value::as_str),
            Some(case.geofence_id),
            "case {:?}: the success must name the stored geofence, got {:?}",
            case.case,
            successes[0]
        );
        let put_create = millis_of(&successes[0], "CreateTime");
        let put_update = millis_of(&successes[0], "UpdateTime");
        assert_eq!(
            put_create, put_update,
            "case {:?}: a first put's CreateTime and UpdateTime must be the same instant, got {:?}",
            case.case, successes[0]
        );

        let read = get_geofence(&router, "fences", case.geofence_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the stored geofence must be readable, got {} {:?}",
            case.case, read.status, read.body
        );
        let read = read.json();
        assert_eq!(
            read.get("GeofenceId").and_then(Value::as_str),
            Some(case.geofence_id),
            "case {:?}: GetGeofence must report the stored id, got {read}",
            case.case
        );
        let expected_geometry = normalized(&(case.geometry)());
        let reported_geometry = normalized(
            read.get("Geometry")
                .unwrap_or_else(|| panic!("case {:?}: expected a Geometry, got {read}", case.case)),
        );
        assert_eq!(
            reported_geometry, expected_geometry,
            "case {:?}: expected the geometry it was put with {expected_geometry}, got {reported_geometry}",
            case.case
        );
        assert_eq!(
            read.get("Status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {:?}: a stored geofence is one the system has indexed, got {read}",
            case.case
        );
        assert_eq!(
            read.get("GeofenceProperties").map(normalized),
            case.properties.as_ref().map(normalized),
            "case {:?}: expected GeofenceProperties {:?}, got {read}",
            case.case,
            case.properties
        );
        assert_eq!(
            millis_of(&read, "CreateTime"),
            put_create,
            "case {:?}: GetGeofence must report the CreateTime the put stored, got {read}",
            case.case
        );
        assert_eq!(
            millis_of(&read, "UpdateTime"),
            put_update,
            "case {:?}: GetGeofence must report the UpdateTime the put stored, got {read}",
            case.case
        );
    }

    // Every row is stored under its own id, so the collection holds them all.
    let expected = i64::try_from(geofence_cases().len()).expect("the table fits in an i64");
    assert_eq!(
        geofence_count(&router, "fences").await,
        expected,
        "the collection must report one geofence per stored row"
    );
}

#[tokio::test]
async fn one_batch_stores_every_entry_it_carries() {
    // The batch is the unit of the request, not of the write: every entry of one
    // request is readable afterwards.
    let router = emulator();
    create_collection(&router, "fleet").await;

    let cases = geofence_cases();
    let entries: Vec<Value> = cases.iter().map(entry).collect();
    let answered = batch_put(&router, "fleet", json!(entries)).await;
    assert_eq!(
        answered.status, 200,
        "BatchPutGeofence must answer 200, got {} {:?}",
        answered.status, answered.body
    );
    let body = answered.json();
    let successes = list("whole table", &body, "Successes");
    assert_eq!(
        successes.len(),
        cases.len(),
        "every entry of the batch must be reported a success, got {successes:?}"
    );
    for case in &cases {
        assert!(
            successes
                .iter()
                .any(|success| success.get("GeofenceId").and_then(Value::as_str)
                    == Some(case.geofence_id)),
            "case {:?}: the batch's success list must name it, got {successes:?}",
            case.case
        );
        assert_eq!(
            get_geofence(&router, "fleet", case.geofence_id).await.status,
            200,
            "case {:?}: every entry of the batch must be readable",
            case.case
        );
    }
}

/// One rejected entry: the geometry it carries and the sentence the service refuses
/// it with, written out as a LITERAL so the wording is pinned by text rather than
/// re-derived from the implementation's own template.
struct RejectedCase {
    case: &'static str,
    geometry: fn() -> Value,
    message: &'static str,
}

/// The rejected-geometry table. Rows differ along which constraint the geometry
/// violates — the member combination, the radius bound in both directions, ring
/// closure, the vertex budget, the coordinate domain on either axis, ring winding in
/// both roles, ring self-intersection, rings touching, ring containment, the
/// antimeridian, and an encoding that carries nothing — so each constraint is
/// checked by the service rather than assumed. A ring shorter than 4 vertices is
/// NOT a row: `LinearRing` is modelled `len 4..`, so the request boundary refuses it
/// before any entry is looked at.
fn rejected_cases() -> Vec<RejectedCase> {
    vec![
        RejectedCase {
            case: "no geometry member at all",
            geometry: || json!({}),
            message: "'Geometry' must specify one of [Polygon, Circle, MultiPolygon, Geobuf].",
        },
        RejectedCase {
            case: "two geometry members at once",
            geometry: || {
                json!({
                    "Circle": { "Center": [-122.33, 47.60], "Radius": 500.0 },
                    "Polygon": square(-122.4, 47.6),
                })
            },
            message: "'Geometry' must specify only one of [Polygon, Circle, MultiPolygon, Geobuf].",
        },
        RejectedCase {
            case: "a radius of zero",
            geometry: || json!({ "Circle": { "Center": [-122.33, 47.60], "Radius": 0.0 } }),
            message: RADIUS_REQUIREMENT,
        },
        RejectedCase {
            case: "a negative radius",
            geometry: || json!({ "Circle": { "Center": [-122.33, 47.60], "Radius": -1.0 } }),
            message: RADIUS_REQUIREMENT,
        },
        RejectedCase {
            case: "a radius one metre past the published bound",
            geometry: || json!({ "Circle": { "Center": [-122.33, 47.60], "Radius": 100_001.0 } }),
            message: RADIUS_REQUIREMENT,
        },
        RejectedCase {
            case: "a circle centred outside the coordinate domain",
            geometry: || json!({ "Circle": { "Center": [-200.0, 47.60], "Radius": 500.0 } }),
            message: "longitude must be between -180 and 180 degrees",
        },
        RejectedCase {
            case: "an unclosed polygon ring",
            geometry: || {
                json!({
                    "Polygon": [[
                        [-122.4, 47.6],
                        [-122.3, 47.6],
                        [-122.3, 47.7],
                        [-122.4, 47.7],
                    ]]
                })
            },
            message:
                "'Geometry.Polygon[0]' must be closed: its first and last vertex must be the same.",
        },
        RejectedCase {
            case: "a polygon vertex outside the latitude domain",
            geometry: || {
                json!({
                    "Polygon": [[
                        [-122.4, 47.6],
                        [-122.3, 95.0],
                        [-122.3, 47.7],
                        [-122.4, 47.6],
                    ]]
                })
            },
            message: "latitude must be between -90 and 90 degrees",
        },
        RejectedCase {
            case: "a polygon past the vertex budget",
            geometry: || json!({ "Polygon": [too_many_vertices()] }),
            message: "'Geometry.Polygon' must have no more than 1000 vertices.",
        },
        RejectedCase {
            case: "an unclosed ring inside a multipolygon",
            geometry: || {
                json!({
                    "MultiPolygon": [
                        square(10.0, 10.0),
                        [[
                            [-10.0, -10.0],
                            [-9.99, -10.0],
                            [-9.99, -9.99],
                            [-9.98, -9.99],
                        ]],
                    ]
                })
            },
            message: "'Geometry.MultiPolygon[1][0]' must be closed: its first and last vertex must be the same.",
        },
        RejectedCase {
            case: "a multipolygon past the vertex budget",
            geometry: || json!({ "MultiPolygon": [[too_many_vertices()]] }),
            message: "'Geometry.MultiPolygon' must have no more than 1000 vertices.",
        },
        RejectedCase {
            case: "a clockwise exterior ring",
            // "The vertices for the exterior ring must be listed in
            // counter-clockwise sequence."
            geometry: || {
                json!({
                    "Polygon": [[
                        [-122.4, 47.6],
                        [-122.4, 47.7],
                        [-122.3, 47.7],
                        [-122.4, 47.6],
                    ]]
                })
            },
            message: "'Geometry.Polygon[0]' must be oriented counter-clockwise.",
        },
        RejectedCase {
            case: "a counter-clockwise interior ring",
            // "Vertices for all interior rings must be listed in clockwise
            // sequence."
            geometry: || {
                json!({
                    "Polygon": [
                        [[-0.02, -0.02], [0.02, -0.02], [0.02, 0.02], [-0.02, 0.02], [-0.02, -0.02]],
                        [[-0.01, -0.01], [0.01, -0.01], [0.01, 0.01], [-0.01, 0.01], [-0.01, -0.01]],
                    ]
                })
            },
            message: "'Geometry.Polygon[1]' must be oriented clockwise.",
        },
        RejectedCase {
            case: "a ring that intersects itself",
            // "No ring is permitted to intersect itself."
            geometry: || json!({ "Polygon": [[[0.0, 0.0], [0.01, 0.01], [0.01, 0.0], [0.0, 0.01], [0.0, 0.0]]] }),
            message: RING_REQUIREMENT,
        },
        RejectedCase {
            case: "an interior ring touching the exterior ring at one point",
            // "Rings must not touch or cross each other." — a single shared point
            // is a touch, which plain geometric validity would admit.
            geometry: || {
                json!({
                    "Polygon": [
                        [[0.0, 0.0], [0.04, 0.0], [0.04, 0.04], [0.0, 0.04], [0.0, 0.0]],
                        [[0.02, 0.0], [0.01, 0.01], [0.03, 0.01], [0.02, 0.0]],
                    ]
                })
            },
            message: RING_REQUIREMENT,
        },
        RejectedCase {
            case: "an interior ring outside the exterior ring",
            // "All interior rings must be fully contained within the exterior
            // ring."
            geometry: || {
                json!({
                    "Polygon": [
                        [[-0.02, -0.02], [0.02, -0.02], [0.02, 0.02], [-0.02, 0.02], [-0.02, -0.02]],
                        [[5.0, 5.0], [5.0, 5.01], [5.01, 5.01], [5.01, 5.0], [5.0, 5.0]],
                    ]
                })
            },
            message: RING_REQUIREMENT,
        },
        RejectedCase {
            case: "a polygon that crosses the antimeridian",
            // "Amazon Location doesn't currently support polygons that cross the
            // antimeridian."
            geometry: || {
                json!({
                    "Polygon": [[
                        [170.0, 0.0],
                        [-170.0, 0.0],
                        [-170.0, 10.0],
                        [170.0, 0.0],
                    ]]
                })
            },
            message: "'Geometry.Polygon[0]' must not cross the antimeridian.",
        },
        RejectedCase {
            case: "an empty geobuf blob",
            geometry: || json!({ "Geobuf": "" }),
            message: "'Geometry.Geobuf' must carry a Geobuf-encoded geometry.",
        },
    ]
}

/// The sentence a polygon whose rings intersect, touch, nest wrongly or enclose no
/// area at all is refused with: the `GeofenceGeometry$Polygon` note list ("Rings
/// must not touch or cross each other", "All interior rings must be fully contained
/// within the exterior ring", "Interior rings must not contain other interior
/// rings", "No ring is permitted to intersect itself") in one sentence, written out
/// as a LITERAL so the wording is pinned by text.
const RING_REQUIREMENT: &str = "'Geometry.Polygon' rings must not intersect themselves, must not touch or cross each other, and every interior ring must be fully contained within the exterior ring and within no other interior ring.";

/// The sentence an unusable `Circle` radius is refused with: the member path and
/// the bound the `Circle$Radius` doc publishes ("Must be greater than zero and no
/// larger than 100,000 (100 kilometers)"), written out as a LITERAL.
const RADIUS_REQUIREMENT: &str =
    "'Geometry.Circle.Radius' must be greater than 0 and no larger than 100000.";

/// A closed ring of 1,002 in-domain vertices: one past the 1,000-vertex budget the
/// `Polygon` and `MultiPolygon` members document, with the ring itself closed so the
/// budget is the only thing it violates.
fn too_many_vertices() -> Value {
    let mut ring: Vec<Value> = (0..1_001)
        .map(|step| json!([-122.0 + f64::from(step) * 0.0001, 47.0]))
        .collect();
    ring.push(ring[0].clone());
    Value::Array(ring)
}

#[tokio::test]
async fn a_partial_batch_stores_the_valid_entry_and_reports_only_the_invalid_one() {
    // The issue's recorded request, generalised: one valid entry beside one
    // rejected one, the rejected one reported inside a 200 and storing nothing.
    for (index, rejected) in rejected_cases().into_iter().enumerate() {
        let router = emulator();
        create_collection(&router, "fences").await;

        let good = json!({
            "GeofenceId": "good",
            "Geometry": { "Circle": { "Center": [-122.33, 47.60], "Radius": 500.0 } },
        });
        let bad = json!({ "GeofenceId": "bad", "Geometry": (rejected.geometry)() });
        // The rejected entry sits first in half the rows and last in the other
        // half: where it sits must not change what is stored.
        let entries = if index % 2 == 0 {
            json!([good, bad])
        } else {
            json!([bad, good])
        };

        let answered = batch_put(&router, "fences", entries).await;
        assert_eq!(
            answered.status, 200,
            "case {:?}: a partial failure is still a 200, got {} {:?}",
            rejected.case, answered.status, answered.body
        );
        let body = answered.json();

        let successes = list(rejected.case, &body, "Successes");
        assert_eq!(
            successes
                .iter()
                .map(|success| success.get("GeofenceId").and_then(Value::as_str))
                .collect::<Vec<_>>(),
            vec![Some("good")],
            "case {:?}: only the valid entry may be reported a success, got {successes:?}",
            rejected.case
        );

        let errors = list(rejected.case, &body, "Errors");
        assert_eq!(
            errors.len(),
            1,
            "case {:?}: only the invalid entry may be reported an error, got {errors:?}",
            rejected.case
        );
        assert_eq!(
            errors[0].get("GeofenceId").and_then(Value::as_str),
            Some("bad"),
            "case {:?}: the reported entry must name the refused geofence, got {:?}",
            rejected.case,
            errors[0]
        );
        let error = errors[0]
            .get("Error")
            .unwrap_or_else(|| panic!("case {:?}: expected an Error member, got {:?}", rejected.case, errors[0]));
        assert_eq!(
            error.get("Code").and_then(Value::as_str),
            Some(VALIDATION_ERROR),
            "case {:?}: expected Error.Code {VALIDATION_ERROR}, got {error}",
            rejected.case
        );
        assert_eq!(
            error.get("Message").and_then(Value::as_str),
            Some(rejected.message),
            "case {:?}: expected Error.Message {:?}, got {error}",
            rejected.case,
            rejected.message
        );

        assert_eq!(
            get_geofence(&router, "fences", "good").await.status,
            200,
            "case {:?}: the valid entry must be readable",
            rejected.case
        );
        let refused = get_geofence(&router, "fences", "bad").await;
        assert_eq!(
            refused.status, 404,
            "case {:?}: the refused entry must store nothing, got {} {:?}",
            rejected.case, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException, got {:?}",
            rejected.case,
            refused.body
        );
        assert_eq!(
            refused.message(),
            "Resource of type \"Geofence\" with name \"bad\" cannot be found.",
            "case {:?}: expected the refusal to name the resource type and the requested id, got {:?}",
            rejected.case,
            refused.message()
        );
        assert_eq!(
            geofence_count(&router, "fences").await,
            1,
            "case {:?}: only the valid entry may be counted by the collection",
            rejected.case
        );
    }
}

#[tokio::test]
async fn an_all_invalid_batch_stores_nothing() {
    let router = emulator();
    create_collection(&router, "fences").await;

    // `BatchPutGeofenceRequestEntryList` is `len 1..10`, so the table is sent in
    // chunks the model admits.
    let rejected = rejected_cases();
    for (chunk_index, chunk) in rejected.chunks(5).enumerate() {
        let entries: Vec<Value> = chunk
            .iter()
            .enumerate()
            .map(|(i, bad)| {
                json!({
                    "GeofenceId": format!("bad-{chunk_index}-{i}"),
                    "Geometry": (bad.geometry)(),
                })
            })
            .collect();
        let case = format!("chunk {chunk_index}");
        let answered = batch_put(&router, "fences", json!(entries)).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: BatchPutGeofence must answer 200, got {} {:?}",
            answered.status, answered.body
        );
        let body = answered.json();
        assert!(
            list(&case, &body, "Successes").is_empty(),
            "case {case:?}: no entry of this chunk may be reported a success, got {body}"
        );
        assert_eq!(
            list(&case, &body, "Errors").len(),
            chunk.len(),
            "case {case:?}: every entry of this chunk must be reported an error, got {body}"
        );
        for (i, bad) in chunk.iter().enumerate() {
            let id = format!("bad-{chunk_index}-{i}");
            let reported = list(&case, &body, "Errors")
                .into_iter()
                .find(|error| error.get("GeofenceId").and_then(Value::as_str) == Some(&id))
                .unwrap_or_else(|| panic!("case {case:?}: entry {id} must be reported, got {body}"));
            assert_eq!(
                reported
                    .get("Error")
                    .and_then(|error| error.get("Message"))
                    .and_then(Value::as_str),
                Some(bad.message),
                "case {case:?} entry {i}: expected Error.Message {:?}, got {reported}",
                bad.message
            );
        }
    }

    assert_eq!(
        geofence_count(&router, "fences").await,
        0,
        "a collection every entry was refused for must hold no geofence"
    );
}

#[tokio::test]
async fn re_putting_a_geofence_updates_it_and_keeps_its_create_time() {
    let router = emulator();
    create_collection(&router, "fences").await;

    let first = batch_put(
        &router,
        "fences",
        json!([{
            "GeofenceId": "moving",
            "Geometry": { "Circle": { "Center": [-122.33, 47.60], "Radius": 500.0 } },
            "GeofenceProperties": { "generation": "first" },
        }]),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first put must succeed, got {} {:?}",
        first.status, first.body
    );
    let stored_create = millis_of(&list("first put", &first.json(), "Successes")[0], "CreateTime");

    // The timestamps are millisecond-resolution, so the second put is separated
    // from the first by more than one tick.
    std::thread::sleep(std::time::Duration::from_millis(5));

    // Each row re-puts the SAME id with a different geometry member, so the update
    // replaces the geometry whatever it was defined with.
    for (case, geometry, properties) in [
        (
            "circle to polygon",
            json!({ "Polygon": square(-122.4, 47.6) }),
            json!({ "generation": "second" }),
        ),
        (
            "polygon to multipolygon",
            json!({ "MultiPolygon": [square(1.0, 1.0)] }),
            json!({ "generation": "third", "note": "updated" }),
        ),
    ] {
        let again = batch_put(
            &router,
            "fences",
            json!([{
                "GeofenceId": "moving",
                "Geometry": geometry,
                "GeofenceProperties": properties,
            }]),
        )
        .await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: the update must succeed, got {} {:?}",
            again.status, again.body
        );
        let success = list(case, &again.json(), "Successes")[0].clone();
        assert_eq!(
            millis_of(&success, "CreateTime"),
            stored_create,
            "case {case:?}: an update must keep the CreateTime the geofence was first stored with, got {success}"
        );
        let updated = millis_of(&success, "UpdateTime");
        assert!(
            updated > stored_create,
            "case {case:?}: expected an UpdateTime later than the CreateTime {stored_create}, got {updated}"
        );

        let read = get_geofence(&router, "fences", "moving").await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the updated geofence must be readable, got {} {:?}",
            read.status, read.body
        );
        let read = read.json();
        assert_eq!(
            read.get("Geometry").map(normalized),
            Some(normalized(&geometry)),
            "case {case:?}: GetGeofence must report the geometry the update stored, got {read}"
        );
        assert_eq!(
            read.get("GeofenceProperties").map(normalized),
            Some(normalized(&properties)),
            "case {case:?}: GetGeofence must report the properties the update stored, got {read}"
        );
        assert_eq!(
            millis_of(&read, "UpdateTime"),
            updated,
            "case {case:?}: GetGeofence must report the UpdateTime the update stored, got {read}"
        );
        assert_eq!(
            geofence_count(&router, "fences").await,
            1,
            "case {case:?}: re-putting an id must update the geofence, not add a second one"
        );
    }
}

#[tokio::test]
async fn putting_into_a_collection_that_was_never_created_is_not_found() {
    let router = emulator();
    // One collection exists; the names below satisfy `^[-._\w]+$` but were never
    // created, including case variants of the one that was.
    create_collection(&router, "Exists").await;

    for name in ["missing", "exists", "EXISTS", "Exists.2"] {
        let answered = batch_put(
            &router,
            name,
            json!([{
                "GeofenceId": "orphan",
                "Geometry": { "Circle": { "Center": [-122.33, 47.60], "Radius": 500.0 } },
            }]),
        )
        .await;
        assert_eq!(
            answered.status, 404,
            "case {name:?}: an unknown collection must be 404, got {} {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.error_code(),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {:?}",
            answered.body
        );
        let expected =
            format!("Resource of type \"GeofenceCollection\" with name \"{name}\" cannot be found.");
        assert_eq!(
            answered.message(),
            expected,
            "case {name:?}: expected message {expected:?}, got {:?}",
            answered.message()
        );
    }

    // The refused writes stored nothing anywhere, including under the collection
    // that does exist.
    assert_eq!(
        geofence_count(&router, "Exists").await,
        0,
        "a write addressed to another name must not store a geofence here"
    );
    assert_eq!(
        get_geofence(&router, "Exists", "orphan").await.status,
        404,
        "a refused write must persist nothing"
    );
}

#[tokio::test]
async fn getting_a_geofence_the_collection_does_not_hold_is_not_found() {
    let router = emulator();
    create_collection(&router, "fences").await;
    let put = batch_put(
        &router,
        "fences",
        json!([{
            "GeofenceId": "Stored",
            "Geometry": { "Circle": { "Center": [-122.33, 47.60], "Radius": 500.0 } },
        }]),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "the positive control must be stored, got {} {:?}",
        put.status, put.body
    );

    // Ids that satisfy `^[-._\p{L}\p{N}]+$` but name no stored geofence, including
    // case variants of the one that is stored: the lookup is by exact id.
    for id in ["missing", "stored", "STORED", "Stored.2"] {
        let read = get_geofence(&router, "fences", id).await;
        assert_eq!(
            read.status, 404,
            "case {id:?}: an unknown geofence must be 404, got {} {:?}",
            read.status, read.body
        );
        let expected = format!("Resource of type \"Geofence\" with name \"{id}\" cannot be found.");
        assert_eq!(
            read.message(),
            expected,
            "case {id:?}: expected message {expected:?}, got {:?}",
            read.message()
        );
    }
}

#[tokio::test]
async fn a_read_does_not_change_what_a_later_reader_sees() {
    let router = emulator();
    create_collection(&router, "fences").await;
    batch_put(
        &router,
        "fences",
        json!([{
            "GeofenceId": "stable",
            "Geometry": { "Polygon": square(-122.4, 47.6) },
            "GeofenceProperties": { "k": "v" },
        }]),
    )
    .await;

    let first = get_geofence(&router, "fences", "stable").await.json();
    for round in 1..=3 {
        let again = get_geofence(&router, "fences", "stable").await;
        assert_eq!(
            again.status, 200,
            "round {round}: the read must keep succeeding, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.json(),
            first,
            "round {round}: GetGeofence must report the same geofence every time, got {:?}",
            again.body
        );
    }
    assert_eq!(
        geofence_count(&router, "fences").await,
        1,
        "the reads must not change what the collection holds"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the signal-catalog -> model-manifest -> vehicle core loop over
//! the service's real `awsJson1_0` wire: `POST /` with
//! `X-Amz-Target: IoTAutobahnControlPlane.<OperationName>`.
//!
//! Every request goes through the same `MultiServiceRouter` the production entrypoint
//! serves, so deserialization, modeled-constraint validation, the handler and
//! serialization all run exactly as they do for a real SDK client.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotfleetwise_iotautobahncontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account every generated emulator presents, read from its single source of truth
/// rather than written out, so these assertions follow the core.
fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// A router carrying only this service, over its own fresh engine.
fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests sign nothing; the `Authorization` header below exists to carry the
    // routing key and the region, not a real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Invoke one operation and return its HTTP status and JSON body.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/iotfleetwise/aws4_request, \
         SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iotfleetwise.example.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("IoTAutobahnControlPlane.{target}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|error| {
            panic!(
                "response to {target} is not JSON ({error}): {}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The default region these tests sign for.
const REGION: &str = "us-east-1";

/// The wire error code of an error body (`__type`, less any namespace prefix).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

fn string_at(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string `{member}` in {body}"))
        .to_string()
}

/// A branch plus one sensor and one attribute: enough for a model manifest to select
/// signals out of.
fn demo_nodes() -> Value {
    json!([
        {"branch": {"fullyQualifiedName": "Vehicle"}},
        {"sensor": {"fullyQualifiedName": "Vehicle.Speed", "dataType": "DOUBLE"}},
        {"attribute": {"fullyQualifiedName": "Vehicle.Color", "dataType": "STRING"}},
    ])
}

/// Create `demo-catalog` and `demo-model` and return their ARNs.
async fn seed_catalog_and_model(router: &MultiServiceRouter, region: &str) -> (String, String) {
    let (status, catalog) = call(
        router,
        region,
        "CreateSignalCatalog",
        json!({"name": "demo-catalog", "nodes": demo_nodes()}),
    )
    .await;
    assert_eq!(status, 200, "CreateSignalCatalog: {catalog}");
    let catalog_arn = string_at(&catalog, "arn");

    let (status, manifest) = call(
        router,
        region,
        "CreateModelManifest",
        json!({
            "name": "demo-model",
            "signalCatalogArn": catalog_arn,
            "nodes": ["Vehicle.Speed", "Vehicle.Color"],
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateModelManifest: {manifest}");
    let manifest_arn = string_at(&manifest, "arn");

    (catalog_arn, manifest_arn)
}

/// RULE: a request that would make one stored record grow past the bound this emulator
/// puts on it is refused, not stored. Rows: the largest accepted size for each bound,
/// then one entry past it.
#[tokio::test]
async fn a_request_past_a_stored_records_bound_is_refused() {
    let router = new_router();
    let (catalog_arn, manifest_arn) = seed_catalog_and_model(&router, REGION).await;

    // Bounds restated from `src/state.rs`, where each one is declared and explained.
    const MAX_VEHICLE_ATTRIBUTES: usize = 500;
    const MAX_ATTRIBUTE_VALUE_LENGTH: usize = 4_096;
    const MAX_MODEL_MANIFEST_NODES: usize = 500;

    let attribute_map = |count: usize| -> Value {
        Value::Object(
            (0..count)
                .map(|index| (format!("attr-{index}"), json!("v")))
                .collect(),
        )
    };

    // At the bound: accepted and stored.
    create_vehicle(
        &router,
        REGION,
        "vin-at-bound",
        &manifest_arn,
        Some(attribute_map(MAX_VEHICLE_ATTRIBUTES)),
    )
    .await;
    let (status, vehicle) = call(
        &router,
        REGION,
        "GetVehicle",
        json!({"vehicleName": "vin-at-bound"}),
    )
    .await;
    assert_eq!(status, 200, "a vehicle at the attribute bound is stored: {vehicle}");
    assert_eq!(
        vehicle["attributes"].as_object().map(serde_json::Map::len),
        Some(MAX_VEHICLE_ATTRIBUTES),
        "case \"attributes at the bound\": expected {MAX_VEHICLE_ATTRIBUTES} entries, got \
         {vehicle}"
    );

    let long_value = "v".repeat(MAX_ATTRIBUTE_VALUE_LENGTH);
    create_vehicle(
        &router,
        REGION,
        "vin-long-value",
        &manifest_arn,
        Some(json!({"attr": long_value})),
    )
    .await;

    // One past the bound: refused with the error that bound owes, and nothing stored.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "one attribute too many",
            "CreateVehicle",
            json!({
                "vehicleName": "vin-too-many",
                "modelManifestArn": manifest_arn,
                "decoderManifestArn": format!(
                    "arn:aws:iotfleetwise:{REGION}:{}:decoder-manifest/demo-decoder",
                    account_id()
                ),
                "attributes": attribute_map(MAX_VEHICLE_ATTRIBUTES + 1),
            }),
            // A count quota, which is what LimitExceededException reports.
            "LimitExceededException",
        ),
        (
            "attribute value one character too long",
            "CreateVehicle",
            json!({
                "vehicleName": "vin-too-long",
                "modelManifestArn": manifest_arn,
                "decoderManifestArn": format!(
                    "arn:aws:iotfleetwise:{REGION}:{}:decoder-manifest/demo-decoder",
                    account_id()
                ),
                "attributes": {"attr": "v".repeat(MAX_ATTRIBUTE_VALUE_LENGTH + 1)},
            }),
            // A malformed member value, which is what ValidationException reports.
            "ValidationException",
        ),
        (
            "one node too many",
            "CreateModelManifest",
            json!({
                "name": "too-many-nodes",
                "signalCatalogArn": catalog_arn,
                "nodes": (0..=MAX_MODEL_MANIFEST_NODES)
                    .map(|index| format!("Vehicle.Signal{index}"))
                    .collect::<Vec<_>>(),
            }),
            // Past the member's modelled `@length(max: 500)`.
            "ValidationException",
        ),
    ];

    for (case, target, request, want_code) in cases {
        let (status, body) = call(&router, REGION, target, request).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            want_code,
            "case {case:?}: expected {want_code}, got {body}"
        );
    }

    for (case, target, request) in [
        ("vin-too-many", "GetVehicle", json!({"vehicleName": "vin-too-many"})),
        ("vin-too-long", "GetVehicle", json!({"vehicleName": "vin-too-long"})),
        (
            "too-many-nodes",
            "GetModelManifest",
            json!({"name": "too-many-nodes"}),
        ),
    ] {
        let (status, body) = call(&router, REGION, target, request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store nothing, got {status} {body}"
        );
    }
}

/// RULE: a `maxResults` outside the modelled 1..100 range is refused, never treated as
/// "give me everything". Rows: both published bounds, and one past each.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = new_router();
    let (_, manifest_arn) = seed_catalog_and_model(&router, REGION).await;
    create_vehicle(&router, REGION, "vin-a", &manifest_arn, None).await;

    // Source: `listVehiclesMaxResults`, `@range(min: 1, max: 100)`.
    for accepted in [1, 100] {
        let (status, body) = call(
            &router,
            REGION,
            "ListVehicles",
            json!({"maxResults": accepted}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case maxResults={accepted}: a published bound is accepted, got {status} {body}"
        );
    }

    for refused in [0, -1, 101, 1_000_000] {
        let (status, body) =
            call(&router, REGION, "ListVehicles", json!({"maxResults": refused})).await;
        assert_eq!(
            status, 400,
            "case maxResults={refused}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case maxResults={refused}: expected ValidationException, got {body}"
        );
    }
}

async fn create_vehicle(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    model_manifest_arn: &str,
    attributes: Option<Value>,
) -> Value {
    let mut request = json!({
        "vehicleName": name,
        "modelManifestArn": model_manifest_arn,
        "decoderManifestArn": format!(
            "arn:aws:iotfleetwise:{region}:{}:decoder-manifest/demo-decoder",
            account_id()
        ),
    });
    if let Some(attributes) = attributes {
        request["attributes"] = attributes;
    }
    let (status, body) = call(router, region, "CreateVehicle", request).await;
    assert_eq!(status, 201, "CreateVehicle {name}: {body}");
    body
}

/// The whole client script of the bootstrap slice, end to end, with every read served
/// from what the writes stored.
#[tokio::test]
async fn the_catalog_model_vehicle_loop_round_trips_through_state() {
    let router = new_router();
    let (catalog_arn, manifest_arn) = seed_catalog_and_model(&router, REGION).await;

    assert_eq!(
        catalog_arn,
        format!(
            "arn:aws:iotfleetwise:{REGION}:{}:signal-catalog/demo-catalog",
            account_id()
        ),
        "the signal catalog ARN is built from the request's region and the emulator account"
    );
    assert_eq!(
        manifest_arn,
        format!(
            "arn:aws:iotfleetwise:{REGION}:{}:model-manifest/demo-model",
            account_id()
        ),
        "the model manifest ARN is built from the request's region and the emulator account"
    );

    // The catalog reads back with the ARN it was minted with and a node tally derived
    // from the nodes that were stored.
    let (status, catalog) = call(
        &router,
        REGION,
        "GetSignalCatalog",
        json!({"name": "demo-catalog"}),
    )
    .await;
    assert_eq!(status, 200, "GetSignalCatalog: {catalog}");
    assert_eq!(
        string_at(&catalog, "arn"),
        catalog_arn,
        "GetSignalCatalog must report the ARN CreateSignalCatalog minted, got {catalog}"
    );
    assert_eq!(
        catalog["nodeCounts"]["totalNodes"].as_i64(),
        Some(3),
        "GetSignalCatalog node tally: {catalog}"
    );

    // So does the model manifest, which starts out DRAFT.
    let (status, manifest) = call(
        &router,
        REGION,
        "GetModelManifest",
        json!({"name": "demo-model"}),
    )
    .await;
    assert_eq!(status, 200, "GetModelManifest: {manifest}");
    assert_eq!(
        string_at(&manifest, "signalCatalogArn"),
        catalog_arn,
        "GetModelManifest must report the catalog it was created against, got {manifest}"
    );
    assert_eq!(
        string_at(&manifest, "status"),
        "DRAFT",
        "a freshly created model manifest is DRAFT, got {manifest}"
    );

    let created = create_vehicle(
        &router,
        REGION,
        "vin-123",
        &manifest_arn,
        Some(json!({"color": "red"})),
    )
    .await;
    let vehicle_arn = string_at(&created, "arn");
    assert_eq!(
        vehicle_arn,
        format!(
            "arn:aws:iotfleetwise:{REGION}:{}:vehicle/vin-123",
            account_id()
        ),
        "the vehicle ARN is built from the request's region and the emulator account"
    );
    assert_eq!(
        string_at(&created, "thingArn"),
        format!("arn:aws:iot:{REGION}:{}:thing/vin-123", account_id()),
        "the associated IoT thing is reported as an `iot` ARN, got {created}"
    );

    // GetVehicle reports what CreateVehicle stored — its own input names only the
    // vehicle, so none of this can be an echo.
    let (status, vehicle) = call(&router, REGION, "GetVehicle", json!({"vehicleName": "vin-123"}))
        .await;
    assert_eq!(status, 200, "GetVehicle: {vehicle}");
    assert_eq!(
        string_at(&vehicle, "arn"),
        vehicle_arn,
        "GetVehicle must report the ARN CreateVehicle minted, got {vehicle}"
    );
    assert_eq!(
        string_at(&vehicle, "modelManifestArn"),
        manifest_arn,
        "GetVehicle must report the stored model manifest ARN, got {vehicle}"
    );
    assert_eq!(
        vehicle["attributes"],
        json!({"color": "red"}),
        "GetVehicle must report the stored attributes, got {vehicle}"
    );
    let creation_time = vehicle["creationTime"].clone();
    assert!(
        creation_time.is_number(),
        "GetVehicle must report a creationTime, got {vehicle}"
    );

    // A second read reports the same minted values: they were stored, not recomputed.
    let (_, again) = call(&router, REGION, "GetVehicle", json!({"vehicleName": "vin-123"})).await;
    assert_eq!(
        again["creationTime"], creation_time,
        "creationTime must be stable across reads, got {again}"
    );
    assert_eq!(
        again["lastModificationTime"], vehicle["lastModificationTime"],
        "lastModificationTime must be stable across reads, got {again}"
    );

    // Delete removes it, is idempotent, and reports the ARN of what it removed.
    for attempt in 1..=2 {
        let (status, deleted) =
            call(&router, REGION, "DeleteVehicle", json!({"vehicleName": "vin-123"})).await;
        assert_eq!(status, 204, "DeleteVehicle attempt {attempt}: {deleted}");
        assert_eq!(
            string_at(&deleted, "vehicleName"),
            "vin-123",
            "DeleteVehicle attempt {attempt} must name what it removed, got {deleted}"
        );
        assert_eq!(
            string_at(&deleted, "arn"),
            vehicle_arn,
            "DeleteVehicle attempt {attempt} must report the vehicle's ARN, got {deleted}"
        );
    }

    let (status, missing) =
        call(&router, REGION, "GetVehicle", json!({"vehicleName": "vin-123"})).await;
    assert_eq!(status, 404, "GetVehicle after delete: {missing}");
    assert_eq!(
        error_code(&missing),
        "ResourceNotFoundException",
        "GetVehicle after delete: {missing}"
    );

    let (status, listed) = call(&router, REGION, "ListVehicles", json!({})).await;
    assert_eq!(status, 200, "ListVehicles after delete: {listed}");
    assert_eq!(
        listed["vehicleSummaries"],
        json!([]),
        "ListVehicles must not list a deleted vehicle, got {listed}"
    );
    assert!(
        listed.get("nextToken").is_none(),
        "the last page carries no nextToken, got {listed}"
    );
}

/// RULE: an ARN a create mints carries the region the request was signed for. One row
/// per region, including a non-default one, so nothing can be hard-coded to `us-east-1`.
#[tokio::test]
async fn minted_arns_carry_the_region_the_request_was_signed_for() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "eu-central-1"] {
        let router = new_router();
        let (catalog_arn, manifest_arn) = seed_catalog_and_model(&router, region).await;
        let created = create_vehicle(&router, region, "vin-1", &manifest_arn, None).await;

        for (member, arn, resource) in [
            ("signal catalog", catalog_arn.clone(), "signal-catalog/demo-catalog"),
            ("model manifest", manifest_arn.clone(), "model-manifest/demo-model"),
            ("vehicle", string_at(&created, "arn"), "vehicle/vin-1"),
        ] {
            assert_eq!(
                arn,
                format!("arn:aws:iotfleetwise:{region}:{}:{resource}", account_id()),
                "case {region}/{member}: expected the signed region in the ARN, got {arn:?}"
            );
        }
    }
}

/// RULE: creating a resource whose name is already taken is a `ConflictException` (409)
/// and does not replace what is stored. One row per resource type the slice creates.
#[tokio::test]
async fn re_creating_a_name_conflicts_for_every_resource_type() {
    let router = new_router();
    let (catalog_arn, manifest_arn) = seed_catalog_and_model(&router, REGION).await;
    create_vehicle(&router, REGION, "vin-1", &manifest_arn, Some(json!({"color": "red"}))).await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "signal catalog",
            "CreateSignalCatalog",
            json!({"name": "demo-catalog", "nodes": demo_nodes()}),
        ),
        (
            "model manifest",
            "CreateModelManifest",
            json!({
                "name": "demo-model",
                "signalCatalogArn": catalog_arn,
                "nodes": ["Vehicle.Speed"],
            }),
        ),
        (
            "vehicle",
            "CreateVehicle",
            json!({
                "vehicleName": "vin-1",
                "modelManifestArn": manifest_arn,
                "decoderManifestArn": format!(
                    "arn:aws:iotfleetwise:{REGION}:{}:decoder-manifest/other",
                    account_id()
                ),
                "attributes": {"color": "blue"},
            }),
        ),
    ];

    for (case, target, request) in cases {
        let (status, body) = call(&router, REGION, target, request).await;
        assert_eq!(status, 409, "case {case:?}: expected 409, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
    }

    // The conflicting create changed nothing.
    let (_, vehicle) = call(&router, REGION, "GetVehicle", json!({"vehicleName": "vin-1"})).await;
    assert_eq!(
        vehicle["attributes"],
        json!({"color": "red"}),
        "a refused create must not overwrite the stored vehicle, got {vehicle}"
    );
}

/// RULE: an operation that resolves a resource by name or ARN answers
/// `ResourceNotFoundException` (404) when nothing in state answers to it. One row per
/// lookup the slice performs.
#[tokio::test]
async fn an_unresolvable_reference_is_a_resource_not_found() {
    let router = new_router();
    seed_catalog_and_model(&router, REGION).await;
    let absent = format!(
        "arn:aws:iotfleetwise:{REGION}:{}:signal-catalog/no-such-catalog",
        account_id()
    );

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "CreateModelManifest/signalCatalogArn",
            "CreateModelManifest",
            json!({"name": "other-model", "signalCatalogArn": absent, "nodes": []}),
        ),
        (
            "CreateVehicle/modelManifestArn",
            "CreateVehicle",
            json!({
                "vehicleName": "vin-9",
                "modelManifestArn": format!(
                    "arn:aws:iotfleetwise:{REGION}:{}:model-manifest/no-such-model",
                    account_id()
                ),
                "decoderManifestArn": format!(
                    "arn:aws:iotfleetwise:{REGION}:{}:decoder-manifest/demo-decoder",
                    account_id()
                ),
            }),
        ),
        (
            "GetSignalCatalog/name",
            "GetSignalCatalog",
            json!({"name": "no-such-catalog"}),
        ),
        (
            "GetModelManifest/name",
            "GetModelManifest",
            json!({"name": "no-such-model"}),
        ),
        (
            "GetVehicle/vehicleName",
            "GetVehicle",
            json!({"vehicleName": "no-such-vehicle"}),
        ),
    ];

    for (case, target, request) in cases {
        let (status, body) = call(&router, REGION, target, request).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // The refused creates stored nothing.
    for (case, target, request) in [
        ("model manifest", "GetModelManifest", json!({"name": "other-model"})),
        ("vehicle", "GetVehicle", json!({"vehicleName": "vin-9"})),
    ] {
        let (status, body) = call(&router, REGION, target, request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store nothing, got {status} {body}"
        );
    }
}

/// RULE: a model manifest may only select nodes its signal catalog declares; the others
/// are reported as invalid signals (`InvalidSignalsException`, 400). The rows walk every
/// member of the model's `Node` union plus a name no node carries.
#[tokio::test]
async fn a_model_manifest_only_selects_nodes_its_catalog_declares() {
    let router = new_router();

    // Source: the `com.amazonaws.iotfleetwise#Node` union's members.
    let nodes = json!([
        {"branch": {"fullyQualifiedName": "Vehicle"}},
        {"sensor": {"fullyQualifiedName": "Vehicle.Speed", "dataType": "DOUBLE"}},
        {"actuator": {"fullyQualifiedName": "Vehicle.Door.Lock", "dataType": "BOOLEAN"}},
        {"attribute": {"fullyQualifiedName": "Vehicle.Color", "dataType": "STRING"}},
        {"struct": {"fullyQualifiedName": "Vehicle.Camera"}},
        {"property": {"fullyQualifiedName": "Vehicle.Camera.Fps", "dataType": "UINT32"}},
    ]);
    let (status, catalog) = call(
        &router,
        REGION,
        "CreateSignalCatalog",
        json!({"name": "wide-catalog", "nodes": nodes}),
    )
    .await;
    assert_eq!(status, 200, "CreateSignalCatalog: {catalog}");
    let catalog_arn = string_at(&catalog, "arn");

    let accepted = [
        "Vehicle",
        "Vehicle.Speed",
        "Vehicle.Door.Lock",
        "Vehicle.Color",
        "Vehicle.Camera",
        "Vehicle.Camera.Fps",
    ];
    for (index, node) in accepted.iter().enumerate() {
        let (status, body) = call(
            &router,
            REGION,
            "CreateModelManifest",
            json!({
                "name": format!("model-{index}"),
                "signalCatalogArn": catalog_arn,
                "nodes": [node],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {node:?}: a node the catalog declares is accepted, got {status} {body}"
        );
    }

    for (case, requested) in [
        ("absent node", vec!["Vehicle.Mass"]),
        ("one present one absent", vec!["Vehicle.Speed", "Vehicle.Mass"]),
        ("case differs", vec!["vehicle.speed"]),
    ] {
        let (status, body) = call(
            &router,
            REGION,
            "CreateModelManifest",
            json!({
                "name": "rejected-model",
                "signalCatalogArn": catalog_arn,
                "nodes": requested,
            }),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidSignalsException",
            "case {case:?}: expected InvalidSignalsException, got {body}"
        );
    }
}

/// RULE: `GetSignalCatalog`'s node tally counts the stored nodes by union member. One
/// row per member of `com.amazonaws.iotfleetwise#Node`, plus a mixed and an empty one.
#[tokio::test]
async fn the_node_tally_counts_the_stored_nodes_by_type() {
    let router = new_router();

    // (case, nodes, (totalNodes, branches, sensors, attributes, actuators, structs, properties))
    let cases: Vec<(&str, Value, [i64; 7])> = vec![
        ("empty", json!([]), [0, 0, 0, 0, 0, 0, 0]),
        (
            "branches",
            json!([
                {"branch": {"fullyQualifiedName": "Vehicle"}},
                {"branch": {"fullyQualifiedName": "Vehicle.Body"}},
            ]),
            [2, 2, 0, 0, 0, 0, 0],
        ),
        (
            "sensor",
            json!([{"sensor": {"fullyQualifiedName": "Vehicle.Speed", "dataType": "DOUBLE"}}]),
            [1, 0, 1, 0, 0, 0, 0],
        ),
        (
            "attribute",
            json!([{"attribute": {"fullyQualifiedName": "Vehicle.Color", "dataType": "STRING"}}]),
            [1, 0, 0, 1, 0, 0, 0],
        ),
        (
            "actuator",
            json!([{"actuator": {"fullyQualifiedName": "Vehicle.Lock", "dataType": "BOOLEAN"}}]),
            [1, 0, 0, 0, 1, 0, 0],
        ),
        (
            "struct and property",
            json!([
                {"struct": {"fullyQualifiedName": "Vehicle.Camera"}},
                {"property": {"fullyQualifiedName": "Vehicle.Camera.Fps", "dataType": "UINT32"}},
            ]),
            [2, 0, 0, 0, 0, 1, 1],
        ),
        (
            "one of each",
            json!([
                {"branch": {"fullyQualifiedName": "Vehicle"}},
                {"sensor": {"fullyQualifiedName": "Vehicle.Speed", "dataType": "DOUBLE"}},
                {"attribute": {"fullyQualifiedName": "Vehicle.Color", "dataType": "STRING"}},
                {"actuator": {"fullyQualifiedName": "Vehicle.Lock", "dataType": "BOOLEAN"}},
                {"struct": {"fullyQualifiedName": "Vehicle.Camera"}},
                {"property": {"fullyQualifiedName": "Vehicle.Camera.Fps", "dataType": "UINT32"}},
            ]),
            [6, 1, 1, 1, 1, 1, 1],
        ),
    ];

    let members = [
        "totalNodes",
        "totalBranches",
        "totalSensors",
        "totalAttributes",
        "totalActuators",
        "totalStructs",
        "totalProperties",
    ];

    for (index, (case, nodes, expected)) in cases.into_iter().enumerate() {
        let name = format!("catalog-{index}");
        let (status, body) = call(
            &router,
            REGION,
            "CreateSignalCatalog",
            json!({"name": name, "nodes": nodes}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: CreateSignalCatalog: {body}");

        let (status, catalog) =
            call(&router, REGION, "GetSignalCatalog", json!({"name": name})).await;
        assert_eq!(status, 200, "case {case:?}: GetSignalCatalog: {catalog}");
        for (member, want) in members.iter().zip(expected) {
            let got = catalog["nodeCounts"][member].as_i64().unwrap_or_default();
            assert_eq!(
                got, want,
                "case {case:?}: expected {member}={want}, got {got:?} in {catalog}"
            );
        }
    }
}

/// RULE: a vehicle's `attributes` read back exactly as they were stored. The rows differ
/// in count and in the characters the values carry.
#[tokio::test]
async fn stored_attributes_read_back_verbatim() {
    let router = new_router();
    let (_, manifest_arn) = seed_catalog_and_model(&router, REGION).await;

    let cases: Vec<(&str, Option<Value>)> = vec![
        ("absent", None),
        ("empty map", Some(json!({}))),
        ("one entry", Some(json!({"color": "red"}))),
        (
            "several entries",
            Some(json!({
                "color": "blue",
                "engine.type": "1.3 L R2",
                "Vehicle_Body-Trim": "",
            })),
        ),
        (
            "non-ascii value",
            Some(json!({"color": "rot \u{00e4}\u{00f6}\u{00fc} \u{4e2d}\u{6587}"})),
        ),
    ];

    for (index, (case, attributes)) in cases.into_iter().enumerate() {
        let name = format!("vin-attr-{index}");
        create_vehicle(&router, REGION, &name, &manifest_arn, attributes.clone()).await;

        let (status, vehicle) =
            call(&router, REGION, "GetVehicle", json!({"vehicleName": name})).await;
        assert_eq!(status, 200, "case {case:?}: GetVehicle: {vehicle}");
        match &attributes {
            None => assert!(
                vehicle.get("attributes").is_none(),
                "case {case:?}: expected no attributes member, got {vehicle}"
            ),
            Some(expected) => assert_eq!(
                &vehicle["attributes"], expected,
                "case {case:?}: expected {expected}, got {vehicle}"
            ),
        }
    }
}

/// RULE (invariant): paging with any `maxResults` visits every vehicle exactly once, in
/// one stable order, and returns a `nextToken` exactly when more remain. Checked over a
/// grid of store sizes and page sizes.
#[tokio::test]
async fn paging_visits_every_vehicle_exactly_once_for_any_page_size() {
    for count in [0_usize, 1, 2, 3, 5, 8] {
        let router = new_router();
        let (_, manifest_arn) = seed_catalog_and_model(&router, REGION).await;
        let mut expected = Vec::new();
        for index in 0..count {
            let name = format!("vin-{index:02}");
            create_vehicle(&router, REGION, &name, &manifest_arn, None).await;
            expected.push(name);
        }
        expected.sort();

        for page_size in [1_usize, 2, 3, 100] {
            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({"maxResults": page_size});
                if let Some(token) = &next_token {
                    request["nextToken"] = json!(token);
                }
                let (status, body) = call(&router, REGION, "ListVehicles", request).await;
                let case = format!("count={count} maxResults={page_size} page={pages}");
                assert_eq!(status, 200, "case {case:?}: ListVehicles: {body}");

                let summaries = body["vehicleSummaries"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {body}"))
                    .clone();
                assert!(
                    summaries.len() <= page_size,
                    "case {case:?}: expected at most {page_size} summaries, got {}",
                    summaries.len()
                );
                for summary in &summaries {
                    seen.push(string_at(summary, "vehicleName"));
                }

                next_token = body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let remaining = expected.len() - seen.len();
                assert_eq!(
                    next_token.is_some(),
                    remaining > 0,
                    "case {case:?}: expected a nextToken only while {remaining} remain, got {body}"
                );
                pages += 1;
                if next_token.is_none() {
                    break;
                }
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?}: paging did not terminate"
                );
            }

            assert_eq!(
                seen, expected,
                "case count={count} maxResults={page_size}: expected every vehicle once in \
                 order, got {seen:?}"
            );
        }
    }
}

/// RULE: a `nextToken` resumes exactly where the previous page stopped, and one this
/// service did not issue — or one handed back with a different filter set — is refused
/// rather than silently paging a different enumeration.
#[tokio::test]
async fn an_unrecognised_or_mismatched_page_token_is_refused() {
    let router = new_router();
    let (catalog_arn, manifest_arn) = seed_catalog_and_model(&router, REGION).await;
    for index in 0..3 {
        create_vehicle(
            &router,
            REGION,
            &format!("vin-{index}"),
            &manifest_arn,
            Some(json!({"color": "red"})),
        )
        .await;
    }

    for (case, token) in [
        ("not hex", "vin-1"),
        ("odd length", "abc"),
        ("not utf-8", "ff"),
        ("hex but not a cursor", "7b7d"),
    ] {
        let (status, body) = call(
            &router,
            REGION,
            "ListVehicles",
            json!({"nextToken": token, "maxResults": 1}),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // A second vehicle model, so one filter can name a manifest that exists.
    let (status, other) = call(
        &router,
        REGION,
        "CreateModelManifest",
        json!({
            "name": "other-model",
            "signalCatalogArn": catalog_arn,
            "nodes": ["Vehicle.Speed"],
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateModelManifest: {other}");
    let other_model = string_at(&other, "arn");

    // A token from an unfiltered page names the enumeration it continues, so handing it
    // back with any filter is refused.
    let (status, first) = call(&router, REGION, "ListVehicles", json!({"maxResults": 1})).await;
    assert_eq!(status, 200, "ListVehicles: {first}");
    let unfiltered_token = string_at(&first, "nextToken");

    let cases: Vec<(&str, Value)> = vec![
        (
            "same filters",
            json!({"maxResults": 1, "nextToken": unfiltered_token}),
        ),
        (
            "model manifest added",
            json!({
                "maxResults": 1,
                "nextToken": unfiltered_token,
                "modelManifestArn": other_model,
            }),
        ),
        (
            "attribute filter added",
            json!({
                "maxResults": 1,
                "nextToken": unfiltered_token,
                "attributeNames": ["color"],
                "attributeValues": ["red"],
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = call(&router, REGION, "ListVehicles", request).await;
        if case == "same filters" {
            assert_eq!(
                status, 200,
                "case {case:?}: the token continues its own enumeration, got {status} {body}"
            );
            assert_eq!(
                body["vehicleSummaries"][0]["vehicleName"],
                json!("vin-1"),
                "case {case:?}: expected the page to resume at vin-1, got {body}"
            );
        } else {
            assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }

    // Paging a FILTERED enumeration works, as long as the filters stay the same.
    let mut seen = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({
            "maxResults": 1,
            "attributeNames": ["color"],
            "attributeValues": ["red"],
        });
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call(&router, REGION, "ListVehicles", request).await;
        assert_eq!(status, 200, "paging a filtered enumeration: {body}");
        for summary in body["vehicleSummaries"]
            .as_array()
            .expect("ListVehicles returns a list")
        {
            seen.push(string_at(summary, "vehicleName"));
        }
        next_token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen,
        vec!["vin-0", "vin-1", "vin-2"],
        "paging a filtered enumeration must visit every match once, got {seen:?}"
    );
}

/// RULE: `ListVehicles` filters by `modelManifestArn`, and by every
/// `attributeNames`/`attributeValues` pair at once (AND, exact value).
#[tokio::test]
async fn list_vehicles_filters_on_model_manifest_and_attribute_pairs() {
    let router = new_router();
    let (catalog_arn, red_model) = seed_catalog_and_model(&router, REGION).await;

    let (status, other) = call(
        &router,
        REGION,
        "CreateModelManifest",
        json!({
            "name": "other-model",
            "signalCatalogArn": catalog_arn,
            "nodes": ["Vehicle.Speed"],
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateModelManifest: {other}");
    let other_model = string_at(&other, "arn");

    create_vehicle(
        &router,
        REGION,
        "vin-a",
        &red_model,
        Some(json!({"color": "red", "trim": "base"})),
    )
    .await;
    create_vehicle(
        &router,
        REGION,
        "vin-b",
        &red_model,
        Some(json!({"color": "blue", "trim": "base"})),
    )
    .await;
    create_vehicle(
        &router,
        REGION,
        "vin-c",
        &other_model,
        Some(json!({"color": "red", "trim": "sport"})),
    )
    .await;

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filter", json!({}), vec!["vin-a", "vin-b", "vin-c"]),
        (
            "model manifest",
            json!({"modelManifestArn": red_model}),
            vec!["vin-a", "vin-b"],
        ),
        (
            "other model manifest",
            json!({"modelManifestArn": other_model}),
            vec!["vin-c"],
        ),
        (
            "one attribute",
            json!({"attributeNames": ["color"], "attributeValues": ["red"]}),
            vec!["vin-a", "vin-c"],
        ),
        (
            "two attributes AND",
            json!({
                "attributeNames": ["color", "trim"],
                "attributeValues": ["red", "sport"],
            }),
            vec!["vin-c"],
        ),
        (
            "attribute and model manifest",
            json!({
                "modelManifestArn": red_model,
                "attributeNames": ["color"],
                "attributeValues": ["red"],
            }),
            vec!["vin-a"],
        ),
        (
            "value nothing carries",
            json!({"attributeNames": ["color"], "attributeValues": ["green"]}),
            vec![],
        ),
        (
            "name nothing carries",
            json!({"attributeNames": ["mass"], "attributeValues": ["red"]}),
            vec![],
        ),
    ];

    for (case, request, want) in cases {
        let (status, body) = call(&router, REGION, "ListVehicles", request).await;
        assert_eq!(status, 200, "case {case:?}: ListVehicles: {body}");
        let got: Vec<String> = body["vehicleSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {body}"))
            .iter()
            .map(|summary| string_at(summary, "vehicleName"))
            .collect();
        assert_eq!(
            got, want,
            "case {case:?}: expected {want:?}, got {got:?}"
        );
    }
}

/// RULE: the attributes filter is a set of name/value PAIRS, so a request whose two
/// lists do not line up, or that repeats a name, is refused.
#[tokio::test]
async fn a_malformed_attributes_filter_is_refused() {
    let router = new_router();
    let (_, manifest_arn) = seed_catalog_and_model(&router, REGION).await;
    create_vehicle(&router, REGION, "vin-a", &manifest_arn, Some(json!({"color": "red"}))).await;

    let cases: Vec<(&str, Value)> = vec![
        ("names without values", json!({"attributeNames": ["color"]})),
        ("values without names", json!({"attributeValues": ["red"]})),
        (
            "more names than values",
            json!({"attributeNames": ["color", "trim"], "attributeValues": ["red"]}),
        ),
        (
            "more values than names",
            json!({"attributeNames": ["color"], "attributeValues": ["red", "base"]}),
        ),
        (
            "repeated name",
            json!({
                "attributeNames": ["color", "color"],
                "attributeValues": ["red", "blue"],
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = call(&router, REGION, "ListVehicles", request).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE: `listResponseScope=METADATA_ONLY` asks for the vehicle's identity and
/// timestamps, so the summary drops the attributes it would otherwise carry.
#[tokio::test]
async fn metadata_only_drops_the_attributes_from_a_summary() {
    let router = new_router();
    let (_, manifest_arn) = seed_catalog_and_model(&router, REGION).await;
    create_vehicle(
        &router,
        REGION,
        "vin-a",
        &manifest_arn,
        Some(json!({"color": "red"})),
    )
    .await;

    let cases: Vec<(&str, Value, bool)> = vec![
        ("default scope", json!({}), true),
        (
            "metadata only",
            json!({"listResponseScope": "METADATA_ONLY"}),
            false,
        ),
    ];

    for (case, request, want_attributes) in cases {
        let (status, body) = call(&router, REGION, "ListVehicles", request).await;
        assert_eq!(status, 200, "case {case:?}: ListVehicles: {body}");
        let summary = &body["vehicleSummaries"][0];
        assert_eq!(
            summary.get("attributes").is_some(),
            want_attributes,
            "case {case:?}: expected attributes present={want_attributes}, got {summary}"
        );
        // Identity and timestamps are reported either way.
        for member in ["vehicleName", "arn", "creationTime", "lastModificationTime"] {
            assert!(
                summary.get(member).is_some(),
                "case {case:?}: expected {member} in {summary}"
            );
        }
    }
}

/// RULE: a summary reports the same stored values the resource's own reader does.
#[tokio::test]
async fn a_summary_agrees_with_get_vehicle() {
    let router = new_router();
    let (_, manifest_arn) = seed_catalog_and_model(&router, REGION).await;
    let mut created: BTreeMap<String, Value> = BTreeMap::new();
    for index in 0..3 {
        let name = format!("vin-{index}");
        let body = create_vehicle(
            &router,
            REGION,
            &name,
            &manifest_arn,
            Some(json!({"color": format!("c{index}")})),
        )
        .await;
        created.insert(name, body);
    }

    let (status, listed) = call(&router, REGION, "ListVehicles", json!({})).await;
    assert_eq!(status, 200, "ListVehicles: {listed}");
    let summaries = listed["vehicleSummaries"]
        .as_array()
        .expect("ListVehicles returns a list")
        .clone();
    assert_eq!(summaries.len(), created.len(), "every vehicle is listed");

    for summary in summaries {
        let name = string_at(&summary, "vehicleName");
        let (status, vehicle) =
            call(&router, REGION, "GetVehicle", json!({"vehicleName": name})).await;
        assert_eq!(status, 200, "case {name:?}: GetVehicle: {vehicle}");
        for member in [
            "arn",
            "modelManifestArn",
            "decoderManifestArn",
            "creationTime",
            "lastModificationTime",
        ] {
            assert_eq!(
                summary[member], vehicle[member],
                "case {name:?}: expected the summary's {member} to match GetVehicle's, got \
                 {summary} vs {vehicle}"
            );
        }
        assert_eq!(
            summary["attributes"], vehicle["attributes"],
            "case {name:?}: expected the summary's attributes to match GetVehicle's, got \
             {summary} vs {vehicle}"
        );
        assert_eq!(
            summary["arn"],
            created[&name]["arn"],
            "case {name:?}: expected the ARN CreateVehicle minted, got {summary}"
        );
    }
}

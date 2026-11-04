// SPDX-License-Identifier: CC0-1.0

//! The wireless-gateway-task-definition lifecycle, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `CreateWirelessGatewayTaskDefinition` persists the definition it was given and
//!   mints its identity, so `GetWirelessGatewayTaskDefinition` reports back every
//!   member that was stored — over both values of `AutoCreateTasks`, both presence
//!   states of each optional member, and both ends of the published `SigKeyCrc` range
//!   and `Name` length;
//! * a minted `Id` is a fresh uuid and its ARN carries the region the request names;
//! * an `Id` nothing holds is a 404, and one outside the published pattern is a 400;
//! * an `@idempotencyToken` replay with the same parameters returns the first call's
//!   resource, and with different parameters is a 409 that mints nothing;
//! * a value outside a published constraint is refused and mints nothing;
//! * the store is bounded and refuses once full.
//!
//! The wire harness is the one the sibling suites already have: `tests/all/*.rs` are
//! modules of ONE test target, so `request_signed_for`, `call`, `error_code`,
//! `is_uuid_shaped`, `call_signed_for` and `assert_echoes` are shared from there rather
//! than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The store ceiling the service enforces, so this suite asserts the bound the
/// implementation declares rather than a number copied by hand.
use service_com_amazonaws_iotwireless_iotwireless::state::{
    DEFAULT_REGION, MAX_WIRELESS_GATEWAY_TASK_DEFINITIONS,
};

// The ONE wire harness this crate's suites share, and the region and account its
// signed requests mint ARNs in.
use super::destination_and_wireless_device_lifecycle::{
    call, error_code, is_uuid_shaped, router, ACCOUNT, REGION,
};
use super::multicast_group_lifecycle::{assert_echoes, call_signed_for};

/// `WirelessGatewayTaskName`'s published length range.
/// Source: `smithy_query(query="operation",
/// operation="CreateWirelessGatewayTaskDefinition")`.
const NAME_MIN_LEN: usize = 1;
const NAME_MAX_LEN: usize = 2048;

/// `Crc`'s published range, which `LoRaWANUpdateGatewayTaskCreate.SigKeyCrc` targets.
/// Source: `smithy_query(query="shape", shape="LoRaWANUpdateGatewayTaskCreate")`.
const SIG_KEY_CRC_MIN: i64 = 1;
const SIG_KEY_CRC_MAX: i64 = 4_294_967_295;

/// `UpdateDataSource`'s published length range, which both
/// `UpdateWirelessGatewayTaskCreate.UpdateDataSource` and `.UpdateDataRole` target.
/// Source: `smithy_query(query="shape", shape="UpdateWirelessGatewayTaskCreate")`.
const UPDATE_DATA_SOURCE_MIN_LEN: usize = 1;
const UPDATE_DATA_SOURCE_MAX_LEN: usize = 4096;

/// An `Id` the published `WirelessGatewayTaskDefinitionId` pattern admits but no create
/// in this suite ever minted.
const UNMINTED_ID: &str = "11111111-2222-3333-4444-555555555555";

async fn create_definition(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(
        router,
        Method::POST,
        "/wireless-gateway-task-definitions",
        Some(body),
    )
    .await
}

async fn get_definition(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::GET,
        &format!("/wireless-gateway-task-definitions/{id}"),
        None,
    )
    .await
}

/// One create and the read that must report exactly what it stored.
struct Case {
    case: &'static str,
    /// `AutoCreateTasks` as the request carries it — `None` leaves the member out, which
    /// the model marks required, so only the refusal table does that.
    auto_create_tasks: Option<bool>,
    name: Option<String>,
    /// The `Update` member the create carries, if any.
    update: Option<Value>,
    tags: Option<Value>,
}

/// The cases spanning the class: both values of `AutoCreateTasks`, each optional member
/// present and absent, and both ends of every published bound.
fn cases() -> Vec<Case> {
    vec![
        // The whole member set, every bounded value at the top of its range.
        Case {
            case: "full-high-bounds",
            auto_create_tasks: Some(true),
            name: Some("firmware-rollout".to_string()),
            update: Some(json!({
                "UpdateDataSource": "s3://gateway-firmware/station-2.0.6.bin",
                "UpdateDataRole": "arn:aws:iam::000000000000:role/iotwireless-update",
                "LoRaWAN": {
                    "UpdateSignature": "x".repeat(UPDATE_DATA_SOURCE_MAX_LEN),
                    "SigKeyCrc": SIG_KEY_CRC_MAX,
                    "CurrentVersion": {
                        "PackageVersion": "1.0.0",
                        "Model": "SX1301",
                        "Station": "2.0.5",
                    },
                    "UpdateVersion": {
                        "PackageVersion": "1.0.1",
                        "Model": "SX1301",
                        "Station": "2.0.6",
                    },
                },
            })),
            tags: Some(json!([{ "Key": "stage", "Value": "test" }])),
        },
        // The same members at the other end of every bound, and no tags.
        Case {
            case: "full-low-bounds",
            auto_create_tasks: Some(false),
            name: Some("x".repeat(NAME_MIN_LEN)),
            update: Some(json!({
                "UpdateDataSource": "x".repeat(UPDATE_DATA_SOURCE_MIN_LEN),
                "UpdateDataRole": "x".repeat(UPDATE_DATA_SOURCE_MIN_LEN),
                "LoRaWAN": {
                    "UpdateSignature": "x".repeat(UPDATE_DATA_SOURCE_MIN_LEN),
                    "SigKeyCrc": SIG_KEY_CRC_MIN,
                    "CurrentVersion": { "PackageVersion": "0.0.1" },
                    "UpdateVersion": { "Station": "2.0.6" },
                },
            })),
            tags: None,
        },
        // An `Update` carrying only the LoRaWAN half, under a name no other case uses.
        Case {
            case: "lorawan-only",
            auto_create_tasks: Some(false),
            name: Some("lorawan-only".to_string()),
            update: Some(json!({
                "LoRaWAN": {
                    "CurrentVersion": { "Model": "SX1302", "Station": "2.0.5" },
                },
            })),
            tags: Some(json!([])),
        },
        // No name and no update: the definition is just its own identity.
        Case {
            case: "no-name-no-update",
            auto_create_tasks: Some(true),
            name: None,
            update: None,
            tags: Some(json!([
                { "Key": "owner", "Value": "fleet-a" },
                { "Key": "stage", "Value": "prod" },
            ])),
        },
        // A name at its published ceiling, and the `Update` member present and empty.
        Case {
            case: "name-at-ceiling-empty-update",
            auto_create_tasks: Some(false),
            name: Some("x".repeat(NAME_MAX_LEN)),
            update: Some(json!({})),
            tags: None,
        },
        // Only the S3 half of the update, with no LoRaWAN settings at all.
        Case {
            case: "update-data-source-only",
            auto_create_tasks: Some(true),
            name: Some("s3-only".to_string()),
            update: Some(json!({
                "UpdateDataSource": "s3://gateway-firmware/station-2.0.6.bin",
            })),
            tags: Some(json!([{ "Key": "kind", "Value": "update" }])),
        },
    ]
}

/// The request body for one case.
fn body_for(case: &Case) -> Value {
    let mut body = json!({});
    let object = body.as_object_mut().expect("an object body");
    if let Some(auto_create_tasks) = case.auto_create_tasks {
        object.insert("AutoCreateTasks".to_string(), json!(auto_create_tasks));
    }
    if let Some(name) = case.name.clone() {
        object.insert("Name".to_string(), json!(name));
    }
    if let Some(update) = case.update.clone() {
        object.insert("Update".to_string(), update);
    }
    if let Some(tags) = case.tags.clone() {
        object.insert("Tags".to_string(), tags);
    }
    body
}

/// A create persists the definition and the read reports back exactly what was stored,
/// for every case in the table — and each definition is stored independently of the
/// others created on the same store.
#[tokio::test]
async fn a_task_definition_round_trips_through_get_for_every_case() {
    let router = router();
    let mut created: Vec<(&'static str, String, Value)> = Vec::new();

    for case in cases() {
        let body = body_for(&case);
        let (status, answered) = create_definition(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: expected 201 from the create, got {status} {answered}",
            case.case
        );

        let id = answered["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the create must mint an Id", case.case))
            .to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {:?}: expected a uuid-shaped Id, got {id:?}",
            case.case
        );
        assert_eq!(
            answered["Arn"],
            json!(format!(
                "arn:aws:iotwireless:{REGION}:{ACCOUNT}:WirelessGatewayTaskDefinition/{id}"
            )),
            "case {:?}: expected the definition ARN, got {:?}",
            case.case,
            answered["Arn"]
        );
        assert!(
            created.iter().all(|(_, minted, _)| minted != &id),
            "case {:?}: expected a fresh Id, got the already-minted {id:?}",
            case.case
        );

        created.push((case.case, id, body));
    }

    // Every definition is still readable, and reports its OWN record, after the whole
    // table has been created on one store.
    for (case, id, sent) in &created {
        let (status, fetched) = get_definition(&router, id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from the read, got {status} {fetched}"
        );
        assert_eq!(
            fetched["Arn"],
            json!(format!(
                "arn:aws:iotwireless:{REGION}:{ACCOUNT}:WirelessGatewayTaskDefinition/{id}"
            )),
            "case {case:?}: expected the stored ARN read back, got {:?}",
            fetched["Arn"]
        );
        // `Name` comes back exactly as sent, and stays absent when none was sent.
        assert_eq!(
            fetched.get("Name"),
            sent.get("Name"),
            "case {case:?}: expected Name {:?}, got {:?}",
            sent.get("Name"),
            fetched.get("Name")
        );
        let expected_auto = sent
            .get("AutoCreateTasks")
            .cloned()
            .expect("every case carries the required AutoCreateTasks");
        assert_eq!(
            fetched["AutoCreateTasks"], expected_auto,
            "case {case:?}: expected AutoCreateTasks {expected_auto}, got {:?}",
            fetched["AutoCreateTasks"]
        );
        match sent.get("Update") {
            Some(update) => {
                assert_echoes(case, "Update", update, &fetched);
                if let Some(lorawan) = update.get("LoRaWAN") {
                    assert_eq!(
                        fetched["Update"]["LoRaWAN"], *lorawan,
                        "case {case:?}: expected Update.LoRaWAN {lorawan}, got {:?}",
                        fetched["Update"]["LoRaWAN"]
                    );
                }
            }
            None => assert!(
                fetched.get("Update").is_none_or(Value::is_null),
                "case {case:?}: expected no Update member, got {:?}",
                fetched.get("Update")
            ),
        }
    }
}

/// A minted ARN carries the region the request's SigV4 credential scope names, and
/// `DEFAULT_REGION` when the request carries no parseable scope.
#[tokio::test]
async fn a_minted_task_definition_arn_carries_the_region_the_request_names() {
    // (case, credential-scope region, the region the ARN must carry)
    let cases = [
        ("signed for us-west-2", Some("us-west-2"), "us-west-2"),
        ("signed for eu-central-1", Some("eu-central-1"), "eu-central-1"),
        ("signed for ap-southeast-2", Some("ap-southeast-2"), "ap-southeast-2"),
        ("unsigned, routed by endpoint", None, DEFAULT_REGION),
    ];

    for (case, scope_region, expected_region) in cases {
        let router = router();
        let (status, created) = call_signed_for(
            &router,
            scope_region,
            Method::POST,
            "/wireless-gateway-task-definitions",
            Some(json!({ "AutoCreateTasks": false, "Name": "region-case" })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case}: expected 201, got {status} {created}"
        );
        let id = created["Id"].as_str().expect("an Id");
        let expected_arn = json!(format!(
            "arn:aws:iotwireless:{expected_region}:{ACCOUNT}:WirelessGatewayTaskDefinition/{id}"
        ));
        assert_eq!(
            created["Arn"], expected_arn,
            "case {case}: expected the ARN in {expected_region}, got {:?}",
            created["Arn"]
        );

        // The read answers with the ARN that was stored, whatever region the reader is
        // signed for.
        let (status, fetched) = call_signed_for(
            &router,
            Some("us-east-2"),
            Method::GET,
            &format!("/wireless-gateway-task-definitions/{id}"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case}: expected 200 from the read, got {status} {fetched}"
        );
        assert_eq!(
            fetched["Arn"], expected_arn,
            "case {case}: expected the stored ARN read back, got {:?}",
            fetched["Arn"]
        );
    }
}

/// An `Id` the store does not hold is a 404; a minted one is a 200; an `Id` outside the
/// published pattern never reaches the store and is a 400.
#[tokio::test]
async fn a_task_definition_id_nothing_holds_is_not_found() {
    let router = router();
    let (status, created) =
        create_definition(&router, json!({ "AutoCreateTasks": true, "Name": "held" })).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding a definition: expected 201, got {status} {created}"
    );
    let held = created["Id"].as_str().expect("an Id").to_string();

    // (case, the Id addressed, the status it must answer)
    let cases: Vec<(&str, String, StatusCode)> = vec![
        ("the minted id", held.clone(), StatusCode::OK),
        ("a well-formed id nothing holds", UNMINTED_ID.to_string(), StatusCode::NOT_FOUND),
        (
            "an uppercase-hex id nothing holds",
            UNMINTED_ID.to_uppercase(),
            StatusCode::NOT_FOUND,
        ),
        ("not a uuid", "not-a-task-definition".to_string(), StatusCode::BAD_REQUEST),
        (
            "one group too short",
            "11111111-2222-3333-4444-55555555555".to_string(),
            StatusCode::BAD_REQUEST,
        ),
    ];

    for (case, id, expected) in cases {
        let (status, body) = get_definition(&router, &id).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected} for Id {id:?}, got {status} {body}"
        );
        match expected {
            StatusCode::NOT_FOUND => assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            ),
            StatusCode::BAD_REQUEST => assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            ),
            _ => {}
        }
    }
}

/// Replaying a `ClientRequestToken` with the same parameters returns the definition the
/// first call minted; replaying it with different parameters is a 409 that mints
/// nothing.
#[tokio::test]
async fn replaying_a_client_request_token_is_idempotent() {
    let router = router();

    for case in cases() {
        let token = format!("token-{}", case.case);
        let mut body = body_for(&case);
        body.as_object_mut()
            .expect("an object body")
            .insert("ClientRequestToken".to_string(), json!(token));

        let (status, first) = create_definition(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: expected 201 from the first create, got {status} {first}",
            case.case
        );

        let (status, replay) = create_definition(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: expected 201 from the replay, got {status} {replay}",
            case.case
        );
        assert_eq!(
            replay["Id"], first["Id"],
            "case {:?}: expected the replay to return the first Id {:?}, got {:?}",
            case.case, first["Id"], replay["Id"]
        );
        assert_eq!(
            replay["Arn"], first["Arn"],
            "case {:?}: expected the replay to return the first ARN, got {:?}",
            case.case, replay["Arn"]
        );

        // The same token with different parameters is a 409 and mints nothing.
        let mut altered = body.clone();
        altered
            .as_object_mut()
            .expect("an object body")
            .insert("Name".to_string(), json!(format!("other-{}", case.case)));
        let (status, conflict) = create_definition(&router, altered).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {:?}: expected 409 for a token replayed with different parameters, \
             got {status} {conflict}",
            case.case
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {:?}: expected ConflictException, got {conflict}",
            case.case
        );
        assert!(
            conflict.get("Id").is_none(),
            "case {:?}: a refused create must mint nothing, got {conflict}",
            case.case
        );

        // The first definition is untouched by the refusal.
        let id = first["Id"].as_str().expect("an Id");
        let (status, fetched) = get_definition(&router, id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: expected the first definition to survive, got {status} {fetched}",
            case.case
        );
        assert_eq!(
            fetched.get("Name"),
            body.get("Name"),
            "case {:?}: expected the stored Name unchanged, got {:?}",
            case.case,
            fetched.get("Name")
        );
    }
}

/// Each published constraint is accepted on its bound and refused one past it, and a
/// refused create mints nothing readable.
#[tokio::test]
async fn a_value_outside_a_published_constraint_is_refused() {
    let router = router();

    // (case, the create body, whether the service accepts it)
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "Name at its floor",
            json!({ "AutoCreateTasks": false, "Name": "x".repeat(NAME_MIN_LEN) }),
            true,
        ),
        (
            "Name at its ceiling",
            json!({ "AutoCreateTasks": true, "Name": "x".repeat(NAME_MAX_LEN) }),
            true,
        ),
        (
            "Name below its floor",
            json!({ "AutoCreateTasks": false, "Name": "" }),
            false,
        ),
        (
            "Name past its ceiling",
            json!({ "AutoCreateTasks": false, "Name": "x".repeat(NAME_MAX_LEN + 1) }),
            false,
        ),
        (
            "SigKeyCrc at its floor",
            json!({
                "AutoCreateTasks": false,
                "Update": { "LoRaWAN": { "SigKeyCrc": SIG_KEY_CRC_MIN } },
            }),
            true,
        ),
        (
            "SigKeyCrc at its ceiling",
            json!({
                "AutoCreateTasks": false,
                "Update": { "LoRaWAN": { "SigKeyCrc": SIG_KEY_CRC_MAX } },
            }),
            true,
        ),
        (
            "SigKeyCrc below its floor",
            json!({
                "AutoCreateTasks": false,
                "Update": { "LoRaWAN": { "SigKeyCrc": SIG_KEY_CRC_MIN - 1 } },
            }),
            false,
        ),
        (
            "SigKeyCrc past its ceiling",
            json!({
                "AutoCreateTasks": false,
                "Update": { "LoRaWAN": { "SigKeyCrc": SIG_KEY_CRC_MAX + 1 } },
            }),
            false,
        ),
        (
            "UpdateDataSource at its floor",
            json!({
                "AutoCreateTasks": false,
                "Update": { "UpdateDataSource": "x".repeat(UPDATE_DATA_SOURCE_MIN_LEN) },
            }),
            true,
        ),
        (
            "UpdateDataSource below its floor",
            json!({ "AutoCreateTasks": false, "Update": { "UpdateDataSource": "" } }),
            false,
        ),
        (
            "UpdateDataRole past its ceiling",
            json!({
                "AutoCreateTasks": false,
                "Update": {
                    "UpdateDataRole": "x".repeat(UPDATE_DATA_SOURCE_MAX_LEN + 1),
                },
            }),
            false,
        ),
        (
            "ClientRequestToken outside its pattern",
            json!({ "AutoCreateTasks": false, "ClientRequestToken": "not a token!" }),
            false,
        ),
        (
            "ClientRequestToken inside its pattern",
            json!({
                "AutoCreateTasks": false,
                "ClientRequestToken": "token_for-the-pattern-case",
            }),
            true,
        ),
        // `AutoCreateTasks` is modelled required, so a request that leaves it out is
        // refused before it reaches the store.
        (
            "AutoCreateTasks left out",
            json!({ "Name": "no-auto-create-tasks" }),
            false,
        ),
    ];

    for (case, body, accepted) in cases {
        let (status, answered) = create_definition(&router, body).await;
        if accepted {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {case:?}: expected 201, got {status} {answered}"
            );
            let id = answered["Id"].as_str().expect("an Id");
            let (status, fetched) = get_definition(&router, id).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 from the read, got {status} {fetched}"
            );
        } else {
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "case {case:?}: expected 400, got {status} {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {answered}"
            );
            assert!(
                answered.get("Id").is_none(),
                "case {case:?}: a refused create must mint nothing, got {answered}"
            );
        }
    }
}

/// The task-definition store is BOUNDED: it accepts up to its declared ceiling, every
/// definition it minted along the way is still readable, and the next create is refused
/// rather than growing the store without bound.
#[tokio::test]
async fn the_task_definition_store_is_bounded() {
    let router = router();
    let mut ids = Vec::new();

    for index in 0..MAX_WIRELESS_GATEWAY_TASK_DEFINITIONS {
        let (status, body) = create_definition(
            &router,
            json!({ "AutoCreateTasks": index % 2 == 0, "Name": format!("definition-{index}") }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "definition {index}: expected 201 below the ceiling, got {status} {body}"
        );
        ids.push(body["Id"].as_str().expect("an Id").to_string());
    }

    let (status, body) = create_definition(
        &router,
        json!({ "AutoCreateTasks": false, "Name": "one-too-many" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a full store must refuse the next create, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "a full store must refuse with ValidationException, got {body}"
    );

    for (index, id) in ids.iter().enumerate() {
        let (status, fetched) = get_definition(&router, id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "definition {index}: expected it still readable, got {status} {fetched}"
        );
        assert_eq!(
            fetched["Name"],
            json!(format!("definition-{index}")),
            "definition {index}: expected its own Name, got {:?}",
            fetched["Name"]
        );
        assert_eq!(
            fetched["AutoCreateTasks"],
            json!(index % 2 == 0),
            "definition {index}: expected its own AutoCreateTasks, got {:?}",
            fetched["AutoCreateTasks"]
        );
    }
}

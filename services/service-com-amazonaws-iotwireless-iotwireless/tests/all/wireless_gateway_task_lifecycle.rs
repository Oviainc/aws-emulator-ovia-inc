// SPDX-License-Identifier: CC0-1.0

//! The wireless-gateway-task lifecycle, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `CreateWirelessGatewayTask` resolves BOTH resources it joins against stored state —
//!   the wireless gateway the path addresses and the task definition the body names — so
//!   an identifier the account does not hold is a 404 that records nothing, whichever of
//!   the two it is;
//! * a created task is persisted against its gateway, so `GetWirelessGatewayTask`
//!   reports back the definition it applies, its `PENDING` status and the instant it was
//!   created, for every definition shape in the table;
//! * a gateway holds at most one task (its reader and its delete address a task by the
//!   gateway id alone), so a second create while one is held is a 409 that leaves the
//!   held task untouched;
//! * a gateway that holds no task is REPORTED as holding none — every output member of
//!   the reader is optional — while a gateway the account does not hold is refused;
//! * each gateway's task is its own record, independent of every other gateway's;
//! * reading a task does not change what the next caller sees;
//! * a delete is observable by the next caller: the gateway reports no task afterwards
//!   and can take a new one, and a gateway that holds no task answers the same success;
//! * a value outside a published constraint is refused and records nothing.
//!
//! The wire harness is the one the sibling suites already have: `tests/all/*.rs` are
//! modules of ONE test target, so `call`, `error_code`, `is_uuid_shaped` and `router`
//! are shared from there rather than copied.

use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE wire harness this crate's suites share.
use super::destination_and_wireless_device_lifecycle::{call, error_code, is_uuid_shaped, router};

/// `WirelessGatewayTaskStatus`'s modelled values — the model-generated constant, so the
/// suite and the service cannot drift apart.
const WIRELESS_GATEWAY_TASK_STATUSES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::WIRELESS_GATEWAY_TASK_STATUS_VALUES;

/// The `WirelessGatewayTaskStatus` a freshly created task carries: the gateway has not
/// picked the update up yet, so the task is still waiting.
/// Source: `smithy_query(query="shape", shape="WirelessGatewayTaskStatus")` lists it, and
/// `CreateWirelessGatewayTask`'s `Status` is "the status of the request".
const CREATED_TASK_STATUS: &str = "PENDING";

/// Ids the published `WirelessGatewayTaskDefinitionId` / `WirelessGatewayId` forms admit
/// but no create in this suite ever minted.
const UNMINTED_DEFINITION_ID: &str = "11111111-2222-3333-4444-555555555555";
const UNMINTED_GATEWAY_ID: &str = "99999999-8888-7777-6666-555555555555";

async fn create_gateway(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/wireless-gateways", Some(body)).await
}

async fn create_definition(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(
        router,
        Method::POST,
        "/wireless-gateway-task-definitions",
        Some(body),
    )
    .await
}

async fn create_task(
    router: &MultiServiceRouter,
    gateway_id: &str,
    body: Value,
) -> (StatusCode, Value) {
    call(
        router,
        Method::POST,
        &format!("/wireless-gateways/{gateway_id}/tasks"),
        Some(body),
    )
    .await
}

async fn get_task(router: &MultiServiceRouter, gateway_id: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::GET,
        &format!("/wireless-gateways/{gateway_id}/tasks"),
        None,
    )
    .await
}

async fn delete_task(router: &MultiServiceRouter, gateway_id: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::DELETE,
        &format!("/wireless-gateways/{gateway_id}/tasks"),
        None,
    )
    .await
}

/// Seed one wireless gateway and return the `Id` the create minted.
async fn seed_gateway(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = create_gateway(router, body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding a gateway: expected 201, got {status} {created}"
    );
    created["Id"]
        .as_str()
        .expect("the gateway create mints an Id")
        .to_string()
}

/// Seed one task definition and return the `Id` the create minted.
async fn seed_definition(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = create_definition(router, body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding a task definition: expected 201, got {status} {created}"
    );
    created["Id"]
        .as_str()
        .expect("the definition create mints an Id")
        .to_string()
}

/// Seconds since the epoch on the wall clock the service stamps a task with.
fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs() as i64
}

/// `candidate` parsed as the modelled `ISODateTimeString`, in seconds since the epoch.
///
/// Parsed by `aws_smithy_types`, so the suite hand-writes no calendar arithmetic;
/// `Format::DateTime` is the RFC-3339 form the `ISODateTimeString` pattern admits.
/// Source: `smithy_query(query="shape", shape="ISODateTimeString")`.
fn iso_epoch_seconds(candidate: &str) -> Option<i64> {
    DateTime::from_str(candidate, Format::DateTime)
        .ok()
        .map(|parsed| parsed.secs())
}

/// One task creation: the gateway it is created for and the definition it applies.
struct Case {
    case: &'static str,
    /// The `CreateWirelessGateway` body the gateway is seeded with.
    gateway: Value,
    /// The `CreateWirelessGatewayTaskDefinition` body the definition is seeded with.
    definition: Value,
}

/// The cases spanning the class: every definition shape a create can carry (both values
/// of `AutoCreateTasks`, the `Update` member present, partial and absent) against
/// gateways that differ in the members they carry.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "full-update-definition",
            gateway: json!({
                "Name": "task-gateway-full",
                "LoRaWAN": { "GatewayEui": "0102030405060708" },
            }),
            definition: json!({
                "AutoCreateTasks": false,
                "Name": "station-2.0.6",
                "Update": {
                    "UpdateDataSource": "s3://gateway-firmware/station-2.0.6.bin",
                    "UpdateDataRole": "arn:aws:iam::000000000000:role/iotwireless-update",
                    "LoRaWAN": {
                        "SigKeyCrc": 4_294_967_295_i64,
                        "CurrentVersion": { "Station": "2.0.5", "Model": "SX1301" },
                        "UpdateVersion": { "Station": "2.0.6", "Model": "SX1301" },
                    },
                },
            }),
        },
        Case {
            case: "auto-create-tasks-definition",
            gateway: json!({
                "Name": "task-gateway-auto",
                "LoRaWAN": { "GatewayEui": "aa:bb:cc:dd:ee:ff:00:11" },
            }),
            definition: json!({
                "AutoCreateTasks": true,
                "Name": "auto-rollout",
                "Update": { "LoRaWAN": { "CurrentVersion": { "PackageVersion": "1.0.0" } } },
            }),
        },
        Case {
            case: "bare-definition-named-gateway",
            gateway: json!({ "Name": "task-gateway-bare", "LoRaWAN": {} }),
            definition: json!({ "AutoCreateTasks": false }),
        },
        Case {
            case: "unnamed-gateway-tagged-definition",
            gateway: json!({ "LoRaWAN": { "GatewayEui": "01-02-03-04-05-06-07-08" } }),
            definition: json!({
                "AutoCreateTasks": true,
                "Name": "tagged",
                "Tags": [{ "Key": "stage", "Value": "test" }],
            }),
        },
        Case {
            case: "described-gateway-s3-only-definition",
            gateway: json!({
                "Name": "task-gateway-described",
                "Description": "the gateway under test",
                "LoRaWAN": { "GatewayEui": "0a 0b 0c 0d 0e 0f 10 11" },
            }),
            definition: json!({
                "AutoCreateTasks": false,
                "Update": { "UpdateDataSource": "s3://gateway-firmware/station-2.0.7.bin" },
            }),
        },
    ]
}

/// A create records the task against its gateway and the read reports it back, for every
/// case in the table — and each gateway's task stays its own record once the whole table
/// has been created on one store.
#[tokio::test]
async fn a_task_round_trips_through_get_for_every_case() {
    let router = router();
    let mut created: Vec<(&'static str, String, String)> = Vec::new();

    for case in cases() {
        let gateway_id = seed_gateway(&router, case.gateway.clone()).await;
        let definition_id = seed_definition(&router, case.definition.clone()).await;
        assert!(
            is_uuid_shaped(&definition_id),
            "case {:?}: expected a uuid-shaped definition Id, got {definition_id:?}",
            case.case
        );

        let before = now_epoch_seconds();
        let (status, answered) = create_task(
            &router,
            &gateway_id,
            json!({ "WirelessGatewayTaskDefinitionId": definition_id }),
        )
        .await;
        let after = now_epoch_seconds();
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: expected 201 from the create, got {status} {answered}",
            case.case
        );
        assert_eq!(
            answered["WirelessGatewayTaskDefinitionId"],
            json!(definition_id),
            "case {:?}: expected the create to answer with the definition it applied, got {:?}",
            case.case,
            answered["WirelessGatewayTaskDefinitionId"]
        );
        assert_eq!(
            answered["Status"],
            json!(CREATED_TASK_STATUS),
            "case {:?}: expected Status {CREATED_TASK_STATUS:?}, got {:?}",
            case.case,
            answered["Status"]
        );
        assert!(
            WIRELESS_GATEWAY_TASK_STATUSES.contains(&CREATED_TASK_STATUS),
            "case {:?}: expected {CREATED_TASK_STATUS:?} among the modelled statuses \
             {WIRELESS_GATEWAY_TASK_STATUSES:?}",
            case.case
        );

        let (status, fetched) = get_task(&router, &gateway_id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: expected 200 from the read, got {status} {fetched}",
            case.case
        );
        assert_eq!(
            fetched["WirelessGatewayId"],
            json!(gateway_id),
            "case {:?}: expected the task to name its gateway {gateway_id:?}, got {:?}",
            case.case,
            fetched["WirelessGatewayId"]
        );
        assert_eq!(
            fetched["WirelessGatewayTaskDefinitionId"],
            json!(definition_id),
            "case {:?}: expected the stored definition {definition_id:?} read back, got {:?}",
            case.case,
            fetched["WirelessGatewayTaskDefinitionId"]
        );
        assert_eq!(
            fetched["Status"],
            json!(CREATED_TASK_STATUS),
            "case {:?}: expected the stored Status read back, got {:?}",
            case.case,
            fetched["Status"]
        );

        // `TaskCreatedAt` is the instant the create happened, in the modelled
        // `ISODateTimeString` form — so it parses, and it sits inside the window the
        // create ran in.
        let stamped = fetched["TaskCreatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the task must report TaskCreatedAt", case.case))
            .to_string();
        let stamped_seconds = iso_epoch_seconds(&stamped).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected an ISODateTimeString TaskCreatedAt, got {stamped:?}",
                case.case
            )
        });
        assert!(
            (before - 1..=after + 1).contains(&stamped_seconds),
            "case {:?}: expected TaskCreatedAt inside [{}, {}], got {stamped:?} ({stamped_seconds})",
            case.case,
            before - 1,
            after + 1
        );
        // No uplink is carried by this emulator, so the member stays absent rather than
        // reporting traffic that never happened.
        assert!(
            fetched.get("LastUplinkReceivedAt").is_none_or(Value::is_null),
            "case {:?}: expected no LastUplinkReceivedAt, got {:?}",
            case.case,
            fetched.get("LastUplinkReceivedAt")
        );

        // Reading the task again answers exactly the same record: a read changes nothing
        // a later caller sees.
        let (status, again) = get_task(&router, &gateway_id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: expected 200 from the second read, got {status} {again}",
            case.case
        );
        assert_eq!(
            again, fetched,
            "case {:?}: expected the second read to answer the same record, got {again}",
            case.case
        );

        created.push((case.case, gateway_id, definition_id));
    }

    // Every task is still readable, and reports its OWN gateway and definition, after
    // the whole table has been created on one store.
    for (case, gateway_id, definition_id) in &created {
        let (status, fetched) = get_task(&router, gateway_id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected the task to survive the table, got {status} {fetched}"
        );
        assert_eq!(
            fetched["WirelessGatewayTaskDefinitionId"],
            json!(definition_id),
            "case {case:?}: expected its own definition {definition_id:?}, got {:?}",
            fetched["WirelessGatewayTaskDefinitionId"]
        );
        assert_eq!(
            fetched["WirelessGatewayId"],
            json!(gateway_id),
            "case {case:?}: expected its own gateway {gateway_id:?}, got {:?}",
            fetched["WirelessGatewayId"]
        );
    }
}

/// Both identifiers a create joins are resolved against stored state: an identifier the
/// account does not hold is a 404 that records nothing, whichever of the two it is — and
/// each refusal carries the body the service words for its resource kind.
#[tokio::test]
async fn a_create_naming_a_resource_nothing_holds_is_not_found() {
    let router = router();
    let held_definition = seed_definition(&router, json!({ "AutoCreateTasks": false })).await;

    /// How the gateway the create addresses is obtained.
    enum Gateway {
        /// A gateway seeded for this row.
        Held,
        /// A well-formed `WirelessGatewayId` nothing holds.
        Unminted,
    }
    /// Which resource the row expects the create to refuse.
    enum Missing {
        /// Neither: the create is accepted.
        None,
        Gateway,
        Definition,
    }
    // (case, how the gateway is addressed, the definition id the body names, what is
    // missing)
    let rows: Vec<(&str, Gateway, String, Missing)> = vec![
        (
            "both held",
            Gateway::Held,
            held_definition.clone(),
            Missing::None,
        ),
        (
            "definition nothing holds",
            Gateway::Held,
            UNMINTED_DEFINITION_ID.to_string(),
            Missing::Definition,
        ),
        (
            "definition nothing holds, in upper-case hex",
            Gateway::Held,
            UNMINTED_DEFINITION_ID.to_uppercase(),
            Missing::Definition,
        ),
        (
            "gateway nothing holds",
            Gateway::Unminted,
            held_definition.clone(),
            Missing::Gateway,
        ),
        (
            "neither held",
            Gateway::Unminted,
            UNMINTED_DEFINITION_ID.to_string(),
            Missing::Gateway,
        ),
    ];

    for (index, (case, gateway, definition_id, missing)) in rows.into_iter().enumerate() {
        let gateway_id = match gateway {
            Gateway::Held => {
                seed_gateway(
                    &router,
                    json!({ "Name": format!("not-found-gateway-{index}"), "LoRaWAN": {} }),
                )
                .await
            }
            Gateway::Unminted => UNMINTED_GATEWAY_ID.to_string(),
        };

        let (status, answered) = create_task(
            &router,
            &gateway_id,
            json!({ "WirelessGatewayTaskDefinitionId": definition_id }),
        )
        .await;

        let expected = match missing {
            Missing::None => StatusCode::CREATED,
            _ => StatusCode::NOT_FOUND,
        };
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {answered}"
        );

        match missing {
            Missing::None => continue,
            Missing::Gateway => {
                assert_not_found(case, &answered, &gateway_refusal());
                // The gateway itself is not held, so the reader still refuses it: the
                // refused create minted no gateway on the way to recording a task.
                let (status, fetched) = get_task(&router, &gateway_id).await;
                assert_eq!(
                    status,
                    StatusCode::NOT_FOUND,
                    "case {case:?}: a refused create must mint no gateway, got {status} {fetched}"
                );
            }
            Missing::Definition => {
                assert_not_found(
                    case,
                    &answered,
                    &resource_refusal("WirelessGatewayTaskDefinition", &definition_id),
                );
                // A refused create records nothing: the gateway still holds no task.
                assert_holds_no_task(case, &router, &gateway_id).await;
            }
        }
    }
}

/// Assert the gateway holds no task: the reader answers 200 and reports none of the
/// members a held task would fill.
async fn assert_holds_no_task(case: &str, router: &MultiServiceRouter, gateway_id: &str) {
    let (status, fetched) = get_task(router, gateway_id).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "case {case:?}: expected 200 for a gateway with no task, got {status} {fetched}"
    );
    for member in [
        "WirelessGatewayId",
        "WirelessGatewayTaskDefinitionId",
        "Status",
        "TaskCreatedAt",
        "LastUplinkReceivedAt",
    ] {
        assert!(
            fetched.get(member).is_none_or(Value::is_null),
            "case {case:?}: expected no {member} for a gateway with no task, got {:?}",
            fetched.get(member)
        );
    }
}

/// The 404 body a resource kind addressed by an `Id` is refused with: the sentence this
/// service words for the class, with the resource kind and the requested id interpolated,
/// and both optional members carrying those two values.
fn resource_refusal(resource_type: &str, resource_id: &str) -> Value {
    json!({
        "Message": format!(
            "Cannot complete the step because resource is not found, with resource type \
             {resource_type}, resource Id {resource_id}"
        ),
        "ResourceId": resource_id,
        "ResourceType": resource_type,
    })
}

/// The 404 body a missing wireless gateway is refused with: one fixed sentence that
/// interpolates nothing, and neither optional member.
fn gateway_refusal() -> Value {
    json!({ "Message": "Wireless Gateway doesn't exist" })
}

/// Assert `answered` is the `ResourceNotFoundException` body `expected` describes: the
/// error code, the message, and each optional member present with its value or absent.
fn assert_not_found(case: &str, answered: &Value, expected: &Value) {
    assert_eq!(
        error_code(answered),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answered}"
    );
    for member in ["Message", "ResourceId", "ResourceType"] {
        match expected.get(member) {
            Some(value) => assert_eq!(
                answered.get(member),
                Some(value),
                "case {case:?}: expected {member} {value}, got {:?}",
                answered.get(member)
            ),
            None => assert!(
                answered.get(member).is_none_or(Value::is_null),
                "case {case:?}: expected no {member} member, got {:?}",
                answered.get(member)
            ),
        }
    }
}

/// A gateway holds at most ONE task: while a task is held, another create is a 409 that
/// leaves the held task exactly as it was — whether it names the same definition or a
/// different one.
#[tokio::test]
async fn a_gateway_holds_at_most_one_task() {
    let router = router();

    // (case, the definition the second create names relative to the first)
    let rows = ["a different definition", "the same definition"];

    for (index, case) in rows.into_iter().enumerate() {
        let gateway_id = seed_gateway(
            &router,
            json!({ "Name": format!("conflict-gateway-{index}"), "LoRaWAN": {} }),
        )
        .await;
        let first = seed_definition(
            &router,
            json!({ "AutoCreateTasks": false, "Name": format!("first-{index}") }),
        )
        .await;
        let second = if case == "the same definition" {
            first.clone()
        } else {
            seed_definition(
                &router,
                json!({ "AutoCreateTasks": true, "Name": format!("second-{index}") }),
            )
            .await
        };

        let (status, created) = create_task(
            &router,
            &gateway_id,
            json!({ "WirelessGatewayTaskDefinitionId": first }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: expected 201 from the first create, got {status} {created}"
        );
        let (_, held) = get_task(&router, &gateway_id).await;

        let (status, conflict) = create_task(
            &router,
            &gateway_id,
            json!({ "WirelessGatewayTaskDefinitionId": second }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {case:?}: expected 409 while a task is held, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {conflict}"
        );

        let (status, after) = get_task(&router, &gateway_id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected the held task to survive, got {status} {after}"
        );
        assert_eq!(
            after, held,
            "case {case:?}: expected the held task unchanged by the refusal, got {after}"
        );
        assert_eq!(
            after["WirelessGatewayTaskDefinitionId"],
            json!(first),
            "case {case:?}: expected the first definition {first:?} still applied, got {:?}",
            after["WirelessGatewayTaskDefinitionId"]
        );
    }
}

/// A gateway that holds no task is reported as holding none; a gateway nothing holds at
/// all is a 404 carrying the body a missing gateway is refused with.
#[tokio::test]
async fn reading_a_task_no_gateway_holds_reports_none() {
    let router = router();
    let idle = seed_gateway(&router, json!({ "Name": "idle-gateway", "LoRaWAN": {} })).await;

    assert_holds_no_task("a gateway with no task", &router, &idle).await;

    let (status, body) = get_task(&router, UNMINTED_GATEWAY_ID).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "case {:?}: expected 404, got {status} {body}",
        "a gateway nothing holds"
    );
    assert_not_found("a gateway nothing holds", &body, &gateway_refusal());
}

/// Each published constraint on the create is accepted inside its bound and refused
/// outside it, and a refused create records nothing.
#[tokio::test]
async fn a_create_outside_a_published_constraint_is_refused() {
    let router = router();
    let definition = seed_definition(&router, json!({ "AutoCreateTasks": false })).await;

    // (case, the create body, whether the service accepts it)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a definition id inside its pattern",
            json!({ "WirelessGatewayTaskDefinitionId": definition }),
            true,
        ),
        (
            "a definition id outside its pattern",
            json!({ "WirelessGatewayTaskDefinitionId": "not-a-task-definition" }),
            false,
        ),
        (
            "a definition id one group too short",
            json!({ "WirelessGatewayTaskDefinitionId": "11111111-2222-3333-4444-55555555555" }),
            false,
        ),
        (
            "a definition id past its length ceiling",
            json!({
                "WirelessGatewayTaskDefinitionId":
                    "11111111-2222-3333-4444-555555555555-6666",
            }),
            false,
        ),
        // `WirelessGatewayTaskDefinitionId` is modelled required, so a body that leaves
        // it out never reaches the store.
        ("no definition id at all", json!({}), false),
    ];

    for (index, (case, body, accepted)) in rows.into_iter().enumerate() {
        let gateway_id = seed_gateway(
            &router,
            json!({ "Name": format!("constraint-gateway-{index}"), "LoRaWAN": {} }),
        )
        .await;
        let (status, answered) = create_task(&router, &gateway_id, body).await;
        if accepted {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {case:?}: expected 201, got {status} {answered}"
            );
            let (status, fetched) = get_task(&router, &gateway_id).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected the task readable, got {status} {fetched}"
            );
            assert_eq!(
                fetched["Status"],
                json!(CREATED_TASK_STATUS),
                "case {case:?}: expected the stored Status read back, got {:?}",
                fetched["Status"]
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
            assert_holds_no_task(case, &router, &gateway_id).await;
        }
    }
}

/// A delete is observable by the next caller: the gateway reports no task afterwards and
/// can take a new one; deleting again is the same success; and a gateway nothing holds is
/// refused as the missing gateway it is.
#[tokio::test]
async fn deleting_a_task_is_observable_and_frees_the_gateway() {
    let router = router();

    for case in cases() {
        let gateway_id = seed_gateway(&router, case.gateway.clone()).await;
        let definition_id = seed_definition(&router, case.definition.clone()).await;
        let (status, created) = create_task(
            &router,
            &gateway_id,
            json!({ "WirelessGatewayTaskDefinitionId": definition_id }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: expected 201 from the create, got {status} {created}",
            case.case
        );

        let (status, deleted) = delete_task(&router, &gateway_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {:?}: expected 204 from the delete, got {status} {deleted}",
            case.case
        );
        assert_holds_no_task(case.case, &router, &gateway_id).await;

        // Deleting again asks for a state the gateway is already in, so it answers the
        // same success rather than a refusal.
        let (status, again) = delete_task(&router, &gateway_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {:?}: expected 204 from the second delete, got {status} {again}",
            case.case
        );

        // The gateway is free to take another task, which the create no longer refuses.
        let (status, recreated) = create_task(
            &router,
            &gateway_id,
            json!({ "WirelessGatewayTaskDefinitionId": definition_id }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: expected the freed gateway to take a new task, got {status} {recreated}",
            case.case
        );
        let (status, fetched) = get_task(&router, &gateway_id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: expected the new task readable, got {status} {fetched}",
            case.case
        );
        assert_eq!(
            fetched["WirelessGatewayTaskDefinitionId"],
            json!(definition_id),
            "case {:?}: expected the new task's definition {definition_id:?}, got {:?}",
            case.case,
            fetched["WirelessGatewayTaskDefinitionId"]
        );
    }

    let (status, body) = delete_task(&router, UNMINTED_GATEWAY_ID).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "case {:?}: expected 404, got {status} {body}",
        "deleting the task of a gateway nothing holds"
    );
    assert_not_found(
        "deleting the task of a gateway nothing holds",
        &body,
        &gateway_refusal(),
    );
}

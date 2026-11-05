// SPDX-License-Identifier: CC0-1.0

//! The FUOTA-task lifecycle, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `CreateFuotaTask` persists the task it was given and mints its identity, so
//!   `GetFuotaTask` reports back every member that was stored, over the modelled
//!   `SupportedRfRegion` catalog, both ends of the published `RedundancyPercent` range
//!   and both `Descriptor` padding forms the published pattern admits;
//! * a minted `Id` is a fresh uuid and its ARN carries the region the request names;
//! * a created task reports a modelled `FuotaTaskStatus` and no session start time,
//!   because nothing has started its session;
//! * an `@idempotencyToken` replay with the same parameters returns the first call's
//!   resource, and with different parameters is a 409 that changes nothing;
//! * a value outside a published range or pattern, and a missing required member, are
//!   refused and persist nothing;
//! * an `Id` the store does not hold is a 404;
//! * the store is bounded and refuses once full.
//!
//! The wire harness is the one the sibling destination/device suite already has:
//! `tests/all/*.rs` are modules of ONE test target, so `request_signed_for`, `call`,
//! `error_code` and `is_uuid_shaped` are shared from there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The store ceiling and the created-task status the service declares, so this suite
/// asserts what the implementation publishes rather than numbers copied by hand.
use service_com_amazonaws_iotwireless_iotwireless::state::{
    DEFAULT_REGION, FUOTA_TASK_STATUS_PENDING, MAX_FUOTA_TASKS,
};

// The ONE wire harness this crate's suites share, and the region and account its
// signed requests mint ARNs in.
use super::destination_and_wireless_device_lifecycle::{
    call, error_code, is_uuid_shaped, router, ACCOUNT, REGION,
};
use super::multicast_group_lifecycle::call_signed_for;

/// `SupportedRfRegion`'s modelled values — the model-generated constant, so the suite
/// and the service cannot drift apart.
const RF_REGIONS: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::SUPPORTED_RF_REGION_VALUES;

/// `FuotaTaskStatus`'s modelled values, from the same generated source.
const FUOTA_TASK_STATUSES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::FUOTA_TASK_STATUS_VALUES;

/// The published ranges and length ceilings `CreateFuotaTask` declares.
/// Source: `smithy_query(query="operation", operation="CreateFuotaTask")`.
const REDUNDANCY_PERCENT_MIN: i64 = 0;
const REDUNDANCY_PERCENT_MAX: i64 = 100;
const FRAGMENT_SIZE_BYTES_MIN: i64 = 1;
const FRAGMENT_INTERVAL_MS_MIN: i64 = 1;
const NAME_MAX_LEN: usize = 256;

/// An S3 URI and a role the cases reuse for the two required members.
const IMAGE: &str = "s3://fuota-images/firmware-v1.bin";
const ROLE: &str = "arn:aws:iam::000000000000:role/iotwireless-fuota";

async fn create_task(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/fuota-tasks", Some(body)).await
}

async fn get_task(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(router, Method::GET, &format!("/fuota-tasks/{id}"), None).await
}

/// One create and the read that must report exactly what it stored.
struct Case {
    case: &'static str,
    /// The whole request body, so a case varies in which optional members it carries.
    body: Value,
}

fn cases() -> Vec<Case> {
    vec![
        // Every member present, each integer at the top of its published range, and a
        // `Descriptor` in the two-pad (`==`) form the pattern admits.
        Case {
            case: "full-high-bounds",
            body: json!({
                "Name": "FuotaTask_1",
                "Description": "the whole member set",
                "LoRaWAN": { "RfRegion": "US915" },
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
                "RedundancyPercent": REDUNDANCY_PERCENT_MAX,
                "FragmentSizeBytes": 4096,
                "FragmentIntervalMS": 60_000,
                "Descriptor": "dmVyc2lvbg==",
                "Tags": [{ "Key": "stage", "Value": "test" }],
            }),
        },
        // The same member set at the bottom of every range, an empty description and a
        // one-pad (`=`) `Descriptor`.
        Case {
            case: "full-low-bounds",
            body: json!({
                "Name": "",
                "Description": "",
                "LoRaWAN": { "RfRegion": "EU868" },
                "FirmwareUpdateImage": "s3://b/f",
                "FirmwareUpdateRole": ROLE,
                "RedundancyPercent": REDUNDANCY_PERCENT_MIN,
                "FragmentSizeBytes": FRAGMENT_SIZE_BYTES_MIN,
                "FragmentIntervalMS": FRAGMENT_INTERVAL_MS_MIN,
                "Descriptor": "dmVyc2lvbjE=",
                "Tags": [],
            }),
        },
        // Only what the model marks required: every optional member is absent.
        Case {
            case: "required-only",
            body: json!({
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
            }),
        },
        // A catalog entry the issue's units do not cite, an unpadded `Descriptor`, and
        // the `LoRaWAN` member carrying only the region.
        Case {
            case: "as923-4-unpadded-descriptor",
            body: json!({
                "Name": "AS923 task",
                "LoRaWAN": { "RfRegion": "AS923-4" },
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
                "Descriptor": "dmVyc2lvbjEx",
                "Tags": [
                    { "Key": "owner", "Value": "fleet-a" },
                    { "Key": "stage", "Value": "prod" },
                ],
            }),
        },
        // A name at its published ceiling, the `LoRaWAN` member present and empty, and
        // an empty `Descriptor` (the pattern admits the empty string).
        Case {
            case: "name-at-ceiling-lorawan-empty",
            body: json!({
                "Name": "x".repeat(NAME_MAX_LEN),
                "Description": "x".repeat(16),
                "LoRaWAN": {},
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
                "Descriptor": "",
                "FragmentSizeBytes": 256,
            }),
        },
    ]
}

/// The member names `GetFuotaTask` reports straight back from what the create stored.
const ECHOED_MEMBERS: &[&str] = &[
    "Name",
    "Description",
    "FirmwareUpdateImage",
    "FirmwareUpdateRole",
    "RedundancyPercent",
    "FragmentSizeBytes",
    "FragmentIntervalMS",
    "Descriptor",
];

/// A create persists the task and `GetFuotaTask` reports exactly what was stored,
/// across both presence states of every optional member, the published range bounds and
/// every `Descriptor` padding form.
#[tokio::test]
async fn a_fuota_task_round_trips_through_get() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for case in cases() {
        if let Some(region) = case.body["LoRaWAN"].get("RfRegion").and_then(Value::as_str) {
            assert!(
                RF_REGIONS.contains(&region),
                "case {:?}: {region:?} is not in the modelled catalog {RF_REGIONS:?}",
                case.case
            );
        }

        let (status, created) = create_task(&router, case.body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: expected 201, got {status} {created}",
            case.case
        );

        let id = created["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no Id in {created}", case.case))
            .to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {:?}: expected a uuid Id, got {id:?}",
            case.case
        );
        assert!(
            !minted.contains(&id),
            "case {:?}: Id {id:?} was already minted for another task",
            case.case
        );
        let expected_arn = format!("arn:aws:iotwireless:{REGION}:{ACCOUNT}:FuotaTask/{id}");
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {:?}: expected Arn {expected_arn}, got {:?}",
            case.case,
            created["Arn"]
        );
        minted.push(id.clone());

        let (status, fetched) = get_task(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: expected 200 from GetFuotaTask, got {status} {fetched}",
            case.case
        );
        assert_eq!(
            fetched["Id"],
            json!(id),
            "case {:?}: expected Id {id:?}, got {:?}",
            case.case,
            fetched["Id"]
        );
        assert_eq!(
            fetched["Arn"],
            json!(expected_arn),
            "case {:?}: GetFuotaTask Arn must match the created ARN, got {:?}",
            case.case,
            fetched["Arn"]
        );

        for member in ECHOED_MEMBERS {
            assert_eq!(
                fetched.get(*member),
                case.body.get(*member),
                "case {:?}: expected {member} = {:?}, got {:?}",
                case.case,
                case.body.get(*member),
                fetched.get(*member)
            );
        }
        assert_eq!(
            fetched["LoRaWAN"].get("RfRegion"),
            case.body["LoRaWAN"].get("RfRegion"),
            "case {:?}: expected LoRaWAN.RfRegion = {:?}, got {:?}",
            case.case,
            case.body["LoRaWAN"].get("RfRegion"),
            fetched["LoRaWAN"].get("RfRegion")
        );

        assert!(
            fetched
                .get("CreatedAt")
                .is_some_and(|created_at| !created_at.is_null()),
            "case {:?}: expected a CreatedAt timestamp, got {:?}",
            case.case,
            fetched.get("CreatedAt")
        );
        // Nothing has started this task's session, so its status is the one a created
        // task carries and there is no start time to report.
        let status_value = fetched["Status"].as_str().unwrap_or_default();
        assert!(
            FUOTA_TASK_STATUSES.contains(&status_value),
            "case {:?}: {status_value:?} is not in the modelled catalog \
             {FUOTA_TASK_STATUSES:?}",
            case.case
        );
        assert_eq!(
            status_value, FUOTA_TASK_STATUS_PENDING,
            "case {:?}: expected a created task to report {FUOTA_TASK_STATUS_PENDING}, got \
             {status_value:?}",
            case.case
        );
        assert!(
            fetched["LoRaWAN"].get("StartTime").is_none(),
            "case {:?}: a task whose session never started has no StartTime, got {:?}",
            case.case,
            fetched["LoRaWAN"].get("StartTime")
        );
    }
}

/// A minted ARN carries the region the request's SigV4 credential scope names, and
/// falls back to the default region when the request names none.
#[tokio::test]
async fn a_minted_fuota_task_arn_carries_the_region_the_request_names() {
    let router = router();

    for scope_region in [Some("us-east-2"), Some("ap-south-1"), None] {
        let (status, created) = call_signed_for(
            &router,
            scope_region,
            Method::POST,
            "/fuota-tasks",
            Some(json!({ "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE })),
        )
        .await;
        let case = scope_region.unwrap_or("<unsigned>");
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: expected 201, got {status} {created}"
        );
        let id = created["Id"].as_str().unwrap_or_default();
        let expected_region = scope_region.unwrap_or(DEFAULT_REGION);
        assert_eq!(
            created["Arn"],
            json!(format!(
                "arn:aws:iotwireless:{expected_region}:{ACCOUNT}:FuotaTask/{id}"
            )),
            "case {case:?}: expected the task ARN in {expected_region}, got {:?}",
            created["Arn"]
        );
    }
}

/// Every entry of the modelled `SupportedRfRegion` catalog is accepted and read back,
/// and a value outside it is refused without minting anything.
#[tokio::test]
async fn every_rf_region_round_trips_and_an_unmodelled_one_is_refused() {
    let router = router();

    for rf_region in RF_REGIONS {
        let (status, created) = create_task(
            &router,
            json!({
                "LoRaWAN": { "RfRegion": rf_region },
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {rf_region}: expected 201, got {status} {created}"
        );
        let (status, fetched) = get_task(&router, created["Id"].as_str().expect("an Id")).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {rf_region}: expected 200 from the read, got {status} {fetched}"
        );
        assert_eq!(
            fetched["LoRaWAN"]["RfRegion"],
            json!(rf_region),
            "case {rf_region}: expected RfRegion read back, got {:?}",
            fetched["LoRaWAN"]["RfRegion"]
        );
    }

    for rejected in ["EU868-NOT-A-REGION", "eu868", ""] {
        let (status, body) = create_task(
            &router,
            json!({
                "LoRaWAN": { "RfRegion": rejected },
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {rejected:?}: expected 400, got {status} {body}"
        );
        assert!(
            body.get("Id").is_none(),
            "case {rejected:?}: a refused create must mint nothing, got {body}"
        );
    }
}

/// A member is accepted on each published bound and refused one past it; a required
/// member left out, or a `Descriptor` outside its published pattern, is refused too.
/// An accepted value reads back.
#[tokio::test]
async fn a_value_outside_its_published_constraint_is_refused() {
    let router = router();

    // (case, the member and value to overlay on a minimal body, accepted?)
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "redundancy at min",
            "RedundancyPercent",
            json!(REDUNDANCY_PERCENT_MIN),
            true,
        ),
        (
            "redundancy at max",
            "RedundancyPercent",
            json!(REDUNDANCY_PERCENT_MAX),
            true,
        ),
        (
            "redundancy below min",
            "RedundancyPercent",
            json!(REDUNDANCY_PERCENT_MIN - 1),
            false,
        ),
        (
            "redundancy above max",
            "RedundancyPercent",
            json!(REDUNDANCY_PERCENT_MAX + 1),
            false,
        ),
        (
            "fragment size at min",
            "FragmentSizeBytes",
            json!(FRAGMENT_SIZE_BYTES_MIN),
            true,
        ),
        (
            "fragment size below min",
            "FragmentSizeBytes",
            json!(FRAGMENT_SIZE_BYTES_MIN - 1),
            false,
        ),
        (
            "fragment interval at min",
            "FragmentIntervalMS",
            json!(FRAGMENT_INTERVAL_MS_MIN),
            true,
        ),
        (
            "fragment interval below min",
            "FragmentIntervalMS",
            json!(FRAGMENT_INTERVAL_MS_MIN - 1),
            false,
        ),
        (
            "descriptor inside its pattern",
            "Descriptor",
            json!("dmVyc2lvbg=="),
            true,
        ),
        (
            "descriptor outside its pattern",
            "Descriptor",
            json!("not base64!"),
            false,
        ),
        (
            "descriptor mispadded",
            "Descriptor",
            json!("dmVyc2lvbg="),
            false,
        ),
    ];

    for (case, member, value, accepted) in cases {
        let mut body = json!({ "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE });
        body[member] = value.clone();
        let (status, answered) = create_task(&router, body).await;
        if accepted {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {case:?} ({member}={value}): expected 201, got {status} {answered}"
            );
            let (status, fetched) = get_task(&router, answered["Id"].as_str().expect("an Id")).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 from the read, got {status} {fetched}"
            );
            assert_eq!(
                fetched[member], value,
                "case {case:?}: expected {member} read back as {value}, got {:?}",
                fetched[member]
            );
        } else {
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "case {case:?} ({member}={value}): expected 400, got {status} {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {answered}"
            );
        }
    }

    // Each required member, left out on its own.
    for missing in ["FirmwareUpdateImage", "FirmwareUpdateRole"] {
        let mut body = json!({ "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE });
        body.as_object_mut().expect("an object").remove(missing);
        let (status, answered) = create_task(&router, body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case missing {missing}: expected 400, got {status} {answered}"
        );
        assert!(
            answered.get("Id").is_none(),
            "case missing {missing}: a refused create must mint nothing, got {answered}"
        );
    }
}

/// Replaying a `ClientRequestToken` with the same parameters returns the task the first
/// call minted; replaying it with different parameters is a 409 that changes nothing.
#[tokio::test]
async fn replaying_a_fuota_task_client_request_token_is_idempotent() {
    let router = router();

    for case in cases() {
        let token = format!("token-{}", case.case);
        let mut body = case.body.clone();
        body["ClientRequestToken"] = json!(token);

        let (status, first) = create_task(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: first create, got {status} {first}",
            case.case
        );

        let (status, replay) = create_task(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: a replay with the same parameters must succeed, got {status} {replay}",
            case.case
        );
        assert_eq!(
            replay["Id"], first["Id"],
            "case {:?}: a replay must return the first Id {:?}, got {:?}",
            case.case, first["Id"], replay["Id"]
        );
        assert_eq!(
            replay["Arn"], first["Arn"],
            "case {:?}: a replay must return the first Arn {:?}, got {:?}",
            case.case, first["Arn"], replay["Arn"]
        );

        let mut changed = body.clone();
        changed["FirmwareUpdateImage"] = json!(format!("s3://other/{}.bin", case.case));
        let (status, conflict) = create_task(&router, changed).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {:?}: a replay with different parameters must be 409, got {status} {conflict}",
            case.case
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {:?}: expected ConflictException, got {conflict}",
            case.case
        );

        let id = first["Id"].as_str().expect("an Id");
        let (_, fetched) = get_task(&router, id).await;
        assert_eq!(
            fetched.get("FirmwareUpdateImage"),
            case.body.get("FirmwareUpdateImage"),
            "case {:?}: the refused create must not have overwritten FirmwareUpdateImage, \
             got {:?}",
            case.case,
            fetched.get("FirmwareUpdateImage")
        );
    }
}

/// `GetFuotaTask` answers 404 for an `Id` the store does not hold — whatever the id
/// looks like — while the stored task keeps resolving.
#[tokio::test]
async fn an_unknown_fuota_task_id_is_not_found() {
    let router = router();
    let (_, created) = create_task(
        &router,
        json!({ "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE }),
    )
    .await;
    let present = created["Id"].as_str().expect("an Id").to_string();

    for id in [
        "00000000-0000-4000-8000-000000000000",
        "not-a-uuid",
        "A",
        &present.to_uppercase(),
    ] {
        let (status, body) = get_task(&router, id).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {id:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    let (status, _) = get_task(&router, &present).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the stored task must still resolve after the refused reads"
    );
}

/// The FUOTA-task store is BOUNDED: the create refuses once it is full, and every task
/// it accepted up to that point still reads back.
#[tokio::test]
async fn the_fuota_task_store_is_bounded() {
    let router = router();
    let mut ids = Vec::new();

    for index in 0..MAX_FUOTA_TASKS {
        let (status, body) = create_task(
            &router,
            json!({
                "Name": format!("Bounded{index}"),
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {index}: expected 201 below the ceiling, got {status} {body}"
        );
        ids.push(body["Id"].as_str().expect("an Id").to_string());
    }

    let (status, body) = create_task(
        &router,
        json!({ "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a full store must refuse, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "a full store must refuse with ValidationException, got {body}"
    );

    for (index, id) in ids.iter().enumerate() {
        let (status, fetched) = get_task(&router, id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {index}: task {id} must still read back, got {status} {fetched}"
        );
    }
}

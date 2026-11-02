// SPDX-License-Identifier: CC0-1.0

//! The multicast-group lifecycle, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `CreateMulticastGroup` persists the group it was given and mints its identity, so
//!   `GetMulticastGroup` reports back every member that was stored, over the modelled
//!   `SupportedRfRegion` and `DlClass` catalogs and both ends of the published
//!   `DefaultSessionParameters` ranges;
//! * a minted `Id` is a fresh uuid and its ARN carries the region the request names;
//! * an `@idempotencyToken` replay with the same parameters returns the first call's
//!   resource, and with different parameters is a 409;
//! * the membership counts a group reports are DERIVED from the devices that name it,
//!   so they follow the device store rather than a tally written at create time;
//! * an `Id` the store does not hold is a 404;
//! * the store is bounded and refuses once full.
//!
//! The wire harness is the one the sibling destination/device suite already has:
//! `tests/all/*.rs` are modules of ONE test target, so `request_signed_for`, `call`,
//! `error_code` and `is_uuid_shaped` are shared from there rather than copied.

use std::sync::Arc;

use http::{Method, StatusCode};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The store ceiling the service enforces, so this suite asserts the bound the
/// implementation declares rather than a number copied by hand.
use service_com_amazonaws_iotwireless_iotwireless::state::{
    service_state, DEFAULT_REGION, MAX_MULTICAST_GROUPS,
};

// The ONE wire harness this crate's suites share, and the region and account its
// signed requests mint ARNs in.
use super::destination_and_wireless_device_lifecycle::{
    call, error_code, is_uuid_shaped, request_signed_for, ACCOUNT, REGION,
};

/// `SupportedRfRegion`'s modelled values — the model-generated constant, so the suite
/// and the service cannot drift apart.
const RF_REGIONS: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::SUPPORTED_RF_REGION_VALUES;

/// `DlClass`'s modelled values, from the same generated source.
const DL_CLASSES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::DL_CLASS_VALUES;

/// `DefaultSessionParametersMulticast`'s published ranges.
/// Source: `smithy_query(query="shape", shape="DefaultSessionParametersMulticast")`.
const DL_DR_MIN: i64 = 0;
const DL_DR_MAX: i64 = 15;
const DL_FREQ_MIN: i64 = 100_000_000;
const DL_FREQ_MAX: i64 = 1_000_000_000;

/// `ParticipatingGatewaysMulticast.TransmissionInterval`'s published range.
/// Source: `smithy_query(query="shape", shape="ParticipatingGatewaysMulticast")`.
const TRANSMISSION_INTERVAL_MIN: i64 = 0;
const TRANSMISSION_INTERVAL_MAX: i64 = 60_000;

/// `MulticastGroupName`'s published length ceiling.
/// Source: `smithy_query(query="operation", operation="CreateMulticastGroup")`.
const NAME_MAX_LEN: usize = 256;

/// A router plus the shared engine it was built on, so a case can observe the state the
/// handlers read — the engine the router hands every operation, never a second one.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iotwireless_iotwireless::EmulatedService::new(engine.clone()),
    );
    (router, engine)
}

fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// Dispatch one call signed for `scope_region` — `None` leaves it unsigned, so a minted
/// ARN falls back to [`DEFAULT_REGION`] — and return its status plus its parsed JSON
/// body.
pub(crate) async fn call_signed_for(
    router: &MultiServiceRouter,
    scope_region: Option<&str>,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let response = router
        .dispatch(request_signed_for(scope_region, method, uri, body))
        .await;
    let status = response.status();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn create_group(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/multicast-groups", Some(body)).await
}

async fn get_group(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::GET,
        &format!("/multicast-groups/{id}"),
        None,
    )
    .await
}

/// Every member of `sent` must come back from the reader under `member`, unchanged.
pub(crate) fn assert_echoes(case: &str, member: &str, sent: &Value, fetched: &Value) {
    let sent: &Map<String, Value> = sent.as_object().expect("the sent member is an object");
    for (key, value) in sent {
        assert_eq!(
            fetched.get(member).and_then(|found| found.get(key)),
            Some(value),
            "case {case:?}: expected {member}.{key} = {value}, got {:?}",
            fetched.get(member).and_then(|found| found.get(key))
        );
    }
}

/// One create and the read that must report exactly what it stored.
struct Case {
    case: &'static str,
    name: Option<String>,
    description: Option<String>,
    /// The required `LoRaWAN` member the create carries.
    lorawan: Value,
    tags: Option<Value>,
}

fn cases() -> Vec<Case> {
    vec![
        // The whole LoRaWAN member set, every integer at the top of its published range.
        Case {
            case: "full-high-bounds",
            name: Some("MulticastGroup_1".to_string()),
            description: Some("the whole member set".to_string()),
            lorawan: json!({
                "RfRegion": "US915",
                "DlClass": "ClassB",
                "ParticipatingGateways": {
                    "GatewayList": ["gw-one", "gw-two"],
                    "TransmissionInterval": TRANSMISSION_INTERVAL_MAX,
                },
                "DefaultSessionParameters": { "DlDr": DL_DR_MAX, "DlFreq": DL_FREQ_MAX },
            }),
            tags: Some(json!([{ "Key": "stage", "Value": "test" }])),
        },
        // The same member set at the other end of every range, and no name.
        Case {
            case: "full-low-bounds-no-name",
            name: None,
            description: Some(String::new()),
            lorawan: json!({
                "RfRegion": "EU868",
                "DlClass": "ClassC",
                "ParticipatingGateways": {
                    "GatewayList": [],
                    "TransmissionInterval": TRANSMISSION_INTERVAL_MIN,
                },
                "DefaultSessionParameters": { "DlDr": DL_DR_MIN, "DlFreq": DL_FREQ_MIN },
            }),
            tags: None,
        },
        // A catalog entry the issue's units do not cite, with only the region named.
        Case {
            case: "rf-region-only",
            name: Some("AS923 group".to_string()),
            description: None,
            lorawan: json!({ "RfRegion": "AS923-4" }),
            tags: Some(json!([])),
        },
        // The required member present and empty: the group carries no LoRaWAN settings.
        Case {
            case: "lorawan-empty",
            name: Some(String::new()),
            description: Some("x".repeat(16)),
            lorawan: json!({}),
            tags: Some(json!([
                { "Key": "owner", "Value": "fleet-a" },
                { "Key": "stage", "Value": "prod" },
            ])),
        },
        // A name at its published ceiling, and the gateway list without an interval.
        Case {
            case: "name-at-ceiling",
            name: Some("x".repeat(NAME_MAX_LEN)),
            description: None,
            lorawan: json!({
                "RfRegion": "IN865",
                "DlClass": "ClassC",
                "ParticipatingGateways": { "GatewayList": ["only-gw"] },
            }),
            tags: None,
        },
    ]
}

/// The body a case sends to `CreateMulticastGroup`.
fn body_for(case: &Case) -> Value {
    let mut body = json!({ "LoRaWAN": case.lorawan.clone() });
    if let Some(name) = &case.name {
        body["Name"] = json!(name);
    }
    if let Some(description) = &case.description {
        body["Description"] = json!(description);
    }
    if let Some(tags) = &case.tags {
        body["Tags"] = tags.clone();
    }
    body
}

/// A create persists the group and `GetMulticastGroup` reports exactly what was stored,
/// across both presence states of every optional member and both ends of the published
/// integer ranges.
#[tokio::test]
async fn a_multicast_group_round_trips_through_get() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for case in cases() {
        if let Some(region) = case.lorawan.get("RfRegion").and_then(Value::as_str) {
            assert!(
                RF_REGIONS.contains(&region),
                "case {:?}: {region:?} is not in the modelled catalog {RF_REGIONS:?}",
                case.case
            );
        }

        let (status, created) = create_group(&router, body_for(&case)).await;
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
            "case {:?}: Id {id:?} was already minted for another group",
            case.case
        );
        let expected_arn = format!("arn:aws:iotwireless:{REGION}:{ACCOUNT}:MulticastGroup/{id}");
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {:?}: expected Arn {expected_arn}, got {:?}",
            case.case,
            created["Arn"]
        );
        minted.push(id.clone());

        let (status, fetched) = get_group(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: expected 200 from GetMulticastGroup, got {status} {fetched}",
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
            "case {:?}: GetMulticastGroup Arn must match the created ARN, got {:?}",
            case.case,
            fetched["Arn"]
        );
        assert_eq!(
            fetched.get("Name").cloned(),
            case.name.as_ref().map(|name| json!(name)),
            "case {:?}: expected Name {:?}, got {:?}",
            case.case,
            case.name,
            fetched.get("Name")
        );
        assert_eq!(
            fetched.get("Description").cloned(),
            case.description.as_ref().map(|text| json!(text)),
            "case {:?}: expected Description {:?}, got {:?}",
            case.case,
            case.description,
            fetched.get("Description")
        );
        assert_echoes(case.case, "LoRaWAN", &case.lorawan, &fetched);
        assert!(
            fetched
                .get("CreatedAt")
                .is_some_and(|created_at| !created_at.is_null()),
            "case {:?}: expected a CreatedAt timestamp, got {:?}",
            case.case,
            fetched.get("CreatedAt")
        );
        // A freshly created group has no members, and the counts are reported, not
        // omitted: zero is an answer about state.
        for member in ["NumberOfDevicesRequested", "NumberOfDevicesInGroup"] {
            assert_eq!(
                fetched["LoRaWAN"][member],
                json!(0),
                "case {:?}: expected LoRaWAN.{member} = 0 for a group with no devices, got {:?}",
                case.case,
                fetched["LoRaWAN"][member]
            );
        }
    }
}

/// A minted ARN carries the region the request's SigV4 credential scope names, and
/// falls back to the default region when the request names none.
#[tokio::test]
async fn a_minted_arn_carries_the_region_the_request_names() {
    let router = router();

    for scope_region in [Some("us-east-2"), Some("ap-south-1"), None] {
        let (status, created) = call_signed_for(
            &router,
            scope_region,
            Method::POST,
            "/multicast-groups",
            Some(json!({ "LoRaWAN": { "RfRegion": "EU868" } })),
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
                "arn:aws:iotwireless:{expected_region}:{ACCOUNT}:MulticastGroup/{id}"
            )),
            "case {case:?}: expected the group ARN in {expected_region}, got {:?}",
            created["Arn"]
        );
    }
}

/// Every entry of the modelled `SupportedRfRegion` and `DlClass` catalogs is accepted
/// and read back, and a value outside either catalog is refused.
#[tokio::test]
async fn every_catalog_entry_round_trips_and_an_unmodelled_one_is_refused() {
    let router = router();

    for rf_region in RF_REGIONS {
        for dl_class in DL_CLASSES {
            let (status, created) = create_group(
                &router,
                json!({ "LoRaWAN": { "RfRegion": rf_region, "DlClass": dl_class } }),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {rf_region}/{dl_class}: expected 201, got {status} {created}"
            );
            let (status, fetched) = get_group(&router, created["Id"].as_str().expect("an Id")).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {rf_region}/{dl_class}: expected 200 from the read, got {status} {fetched}"
            );
            assert_eq!(
                fetched["LoRaWAN"]["RfRegion"],
                json!(rf_region),
                "case {rf_region}/{dl_class}: expected RfRegion read back, got {:?}",
                fetched["LoRaWAN"]["RfRegion"]
            );
            assert_eq!(
                fetched["LoRaWAN"]["DlClass"],
                json!(dl_class),
                "case {rf_region}/{dl_class}: expected DlClass read back, got {:?}",
                fetched["LoRaWAN"]["DlClass"]
            );
        }
    }

    for (member, value) in [
        ("RfRegion", "EU868-NOT-A-REGION"),
        ("RfRegion", "eu868"),
        ("DlClass", "ClassA"),
    ] {
        let (status, body) =
            create_group(&router, json!({ "LoRaWAN": { member: value } })).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {member}={value:?}: expected 400, got {status} {body}"
        );
        assert!(
            body.get("Id").is_none(),
            "case {member}={value:?}: a refused create must mint nothing, got {body}"
        );
    }
}

/// `DlDr`, `DlFreq` and `TransmissionInterval` are accepted on each published bound and
/// refused one past either end; an accepted value reads back.
#[tokio::test]
async fn a_session_parameter_outside_its_published_range_is_refused() {
    let router = router();

    // (member path in the LoRaWAN body, value, whether the service accepts it)
    let cases: Vec<(&str, &str, i64, bool)> = vec![
        ("DefaultSessionParameters", "DlDr", DL_DR_MIN, true),
        ("DefaultSessionParameters", "DlDr", DL_DR_MAX, true),
        ("DefaultSessionParameters", "DlDr", DL_DR_MIN - 1, false),
        ("DefaultSessionParameters", "DlDr", DL_DR_MAX + 1, false),
        ("DefaultSessionParameters", "DlFreq", DL_FREQ_MIN, true),
        ("DefaultSessionParameters", "DlFreq", DL_FREQ_MAX, true),
        ("DefaultSessionParameters", "DlFreq", DL_FREQ_MIN - 1, false),
        ("DefaultSessionParameters", "DlFreq", DL_FREQ_MAX + 1, false),
        (
            "ParticipatingGateways",
            "TransmissionInterval",
            TRANSMISSION_INTERVAL_MIN,
            true,
        ),
        (
            "ParticipatingGateways",
            "TransmissionInterval",
            TRANSMISSION_INTERVAL_MAX,
            true,
        ),
        (
            "ParticipatingGateways",
            "TransmissionInterval",
            TRANSMISSION_INTERVAL_MIN - 1,
            false,
        ),
        (
            "ParticipatingGateways",
            "TransmissionInterval",
            TRANSMISSION_INTERVAL_MAX + 1,
            false,
        ),
    ];

    for (parent, member, value, accepted) in cases {
        let body = json!({ "LoRaWAN": { parent: { member: value } } });
        let (status, answered) = create_group(&router, body).await;
        if accepted {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {parent}.{member}={value}: expected 201, got {status} {answered}"
            );
            let (status, fetched) =
                get_group(&router, answered["Id"].as_str().expect("an Id")).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {parent}.{member}={value}: expected 200 from the read, got {status} {fetched}"
            );
            assert_eq!(
                fetched["LoRaWAN"][parent][member],
                json!(value),
                "case {parent}.{member}={value}: expected it read back, got {:?}",
                fetched["LoRaWAN"][parent][member]
            );
        } else {
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "case {parent}.{member}={value}: expected 400, got {status} {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "case {parent}.{member}={value}: expected ValidationException, got {answered}"
            );
        }
    }
}

/// Replaying a `ClientRequestToken` with the same parameters returns the group the
/// first call minted; replaying it with different parameters is a 409 that changes
/// nothing.
#[tokio::test]
async fn replaying_a_client_request_token_is_idempotent() {
    let router = router();

    for case in cases() {
        let token = format!("token-{}", case.case);
        let mut body = body_for(&case);
        body["ClientRequestToken"] = json!(token);

        let (status, first) = create_group(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: first create, got {status} {first}",
            case.case
        );

        let (status, replay) = create_group(&router, body.clone()).await;
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
        changed["Description"] = json!(format!("Changed-{}", case.case));
        let (status, conflict) = create_group(&router, changed).await;
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
        let (_, fetched) = get_group(&router, id).await;
        assert_eq!(
            fetched.get("Description").cloned(),
            case.description.as_ref().map(|text| json!(text)),
            "case {:?}: the refused create must not have overwritten Description, got {:?}",
            case.case,
            fetched.get("Description")
        );
    }
}

/// `GetMulticastGroup` answers 404 for an `Id` the store does not hold — whatever the
/// id looks like — while the stored group keeps resolving.
#[tokio::test]
async fn an_unknown_multicast_group_id_is_not_found() {
    let router = router();
    let (_, created) = create_group(&router, json!({ "LoRaWAN": {} })).await;
    let present = created["Id"].as_str().expect("an Id").to_string();

    for id in [
        "00000000-0000-4000-8000-000000000000",
        "not-a-uuid",
        "A",
        &present.to_uppercase(),
    ] {
        let (status, body) = get_group(&router, id).await;
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

    let (status, _) = get_group(&router, &present).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the stored group must still resolve after the refused reads"
    );
}

/// The membership counts a group reports are DERIVED from the devices that name it: as
/// devices join and leave, every read answers from the device store rather than from a
/// tally frozen at create time, and one group's members never count towards another's.
///
/// The association operations are still scaffolded, so the membership is written
/// directly on the devices in the SHARED engine the router hands the handlers — the same
/// records an association will write.
#[tokio::test]
async fn the_membership_counts_follow_the_device_store() {
    let (router, engine) = router_with_engine();

    let (_, first) = create_group(&router, json!({ "LoRaWAN": { "RfRegion": "US915" } })).await;
    let (_, second) = create_group(&router, json!({ "LoRaWAN": { "RfRegion": "EU868" } })).await;
    let first = first["Id"].as_str().expect("an Id").to_string();
    let second = second["Id"].as_str().expect("an Id").to_string();

    let (status, _) = call(
        &router,
        Method::POST,
        "/destinations",
        Some(json!({
            "Name": "MulticastDest",
            "Expression": "RuleOne",
            "ExpressionType": "RuleName",
            "RoleArn": "arn:aws:iam::000000000000:role/iotwireless",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "the destination must be created");

    let mut devices = Vec::new();
    for index in 0..3 {
        let (status, created) = call(
            &router,
            Method::POST,
            "/wireless-devices",
            Some(json!({
                "Type": "LoRaWAN",
                "Name": format!("MulticastDevice{index}"),
                "DestinationName": "MulticastDest",
            })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {index}: expected 201 from CreateWirelessDevice, got {created}"
        );
        devices.push(created["Id"].as_str().expect("an Id").to_string());
    }

    /// Put `device` in `group` (`None` takes it out of every group), the way an
    /// association writes it.
    fn set_membership(
        engine: &Arc<engine::InMemoryEngine>,
        device: &str,
        group: Option<&str>,
    ) {
        let state = service_state(&**engine).expect("the iotwireless store");
        let mut store = state.lock();
        store
            .wireless_devices
            .get_mut(device)
            .expect("a stored device")
            .multicast_group_id = group.map(str::to_string);
    }

    // One step of the table: the memberships it applies first, then the counts each
    // group must report once they are applied.
    struct Step<'a> {
        case: &'static str,
        /// `(index into `devices`, the group it now belongs to — `None` = no group)`.
        memberships: Vec<(usize, Option<&'a str>)>,
        expected_first: i64,
        expected_second: i64,
    }

    let steps = vec![
        Step {
            case: "no members",
            memberships: vec![],
            expected_first: 0,
            expected_second: 0,
        },
        Step {
            case: "one joins",
            memberships: vec![(0, Some(first.as_str()))],
            expected_first: 1,
            expected_second: 0,
        },
        Step {
            case: "two more join",
            memberships: vec![(1, Some(first.as_str())), (2, Some(first.as_str()))],
            expected_first: 3,
            expected_second: 0,
        },
        Step {
            case: "one moves to the other group",
            memberships: vec![(2, Some(second.as_str()))],
            expected_first: 2,
            expected_second: 1,
        },
        Step {
            case: "one leaves",
            memberships: vec![(0, None)],
            expected_first: 1,
            expected_second: 1,
        },
        Step {
            case: "every member leaves",
            memberships: vec![(1, None), (2, None)],
            expected_first: 0,
            expected_second: 0,
        },
    ];

    for step in steps {
        let case = step.case;
        for (device, group) in step.memberships {
            set_membership(&engine, &devices[device], group);
        }

        for (group, expected) in [
            (&first, step.expected_first),
            (&second, step.expected_second),
        ] {
            let (status, fetched) = get_group(&router, group).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 for group {group}, got {status} {fetched}"
            );
            for member in ["NumberOfDevicesRequested", "NumberOfDevicesInGroup"] {
                assert_eq!(
                    fetched["LoRaWAN"][member],
                    json!(expected),
                    "case {case:?}: expected LoRaWAN.{member} = {expected} for group {group}, \
                     got {:?}",
                    fetched["LoRaWAN"][member]
                );
            }
        }
    }
}

/// The multicast-group store is BOUNDED: the create refuses once it is full, and every
/// group it accepted up to that point still reads back.
#[tokio::test]
async fn the_multicast_group_store_is_bounded() {
    let router = router();
    let mut ids = Vec::new();

    for index in 0..MAX_MULTICAST_GROUPS {
        let (status, body) = create_group(
            &router,
            json!({ "Name": format!("Bounded{index}"), "LoRaWAN": { "RfRegion": "US915" } }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {index}: expected 201 below the ceiling, got {status} {body}"
        );
        ids.push(body["Id"].as_str().expect("an Id").to_string());
    }

    let (status, body) = create_group(&router, json!({ "LoRaWAN": {} })).await;
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
        let (status, fetched) = get_group(&router, id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {index}: group {id} must still read back, got {status} {fetched}"
        );
    }
}

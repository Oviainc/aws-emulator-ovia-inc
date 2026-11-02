// SPDX-License-Identifier: CC0-1.0

//! The wireless-gateway lifecycle, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `CreateWirelessGateway` persists the gateway it was given and mints its identity,
//!   so `GetWirelessGateway` reports back every member that was stored — over both
//!   presence states of each optional member, every `GatewayEui` separator form the
//!   model's pattern admits, and both ends of the published `MaxEirp`, `SubBand`,
//!   `BeaconingDataRate` and `BeaconingFrequency` ranges;
//! * a minted `Id` is a fresh uuid and its ARN carries the region the request names;
//! * every entry of the modelled `WirelessGatewayIdType` catalog addresses the gateway
//!   by one of its own persisted fields, and an identifier nothing holds is a 404;
//! * a `GatewayEui` identifies at most one gateway, so a second claimant is refused
//!   rather than overwriting the first;
//! * an `@idempotencyToken` replay with the same parameters returns the first call's
//!   resource, and with different parameters is a 409;
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

/// The store ceiling and the name rule the service enforces, so this suite asserts what
/// the implementation declares rather than a value copied by hand.
use service_com_amazonaws_iotwireless_iotwireless::state::{
    DEFAULT_REGION, FORBIDDEN_NAME_CHARS, MAX_WIRELESS_GATEWAYS,
};

// The ONE wire harness this crate's suites share, and the region and account its
// signed requests mint ARNs in.
use super::destination_and_wireless_device_lifecycle::{
    call, error_code, is_uuid_shaped, router, ACCOUNT, REGION,
};
use super::multicast_group_lifecycle::{assert_echoes, call_signed_for};

/// The identifier kinds `GetWirelessGateway` accepts — the model-generated constant the
/// handler validates against, so the suite and the service cannot drift apart.
const WIRELESS_GATEWAY_ID_TYPES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::WIRELESS_GATEWAY_ID_TYPE_VALUES;

/// `GatewayMaxEirp`'s published range.
/// Source: `smithy_query(query="shape", shape="GatewayMaxEirp")`.
const MAX_EIRP_MIN: f64 = 0.0;
const MAX_EIRP_MAX: f64 = 30.0;

/// `SubBand`'s published range.
/// Source: `smithy_query(query="shape", shape="SubBand")`.
const SUB_BAND_MIN: i64 = 1;
const SUB_BAND_MAX: i64 = 8;

/// `BeaconingDataRate`'s published range.
/// Source: `smithy_query(query="shape", shape="BeaconingDataRate")`.
const DATA_RATE_MIN: i64 = 0;
const DATA_RATE_MAX: i64 = 15;

/// `BeaconingFrequency`'s published range.
/// Source: `smithy_query(query="shape", shape="BeaconingFrequencies")`.
const FREQUENCY_MIN: i64 = 100_000_000;
const FREQUENCY_MAX: i64 = 1_000_000_000;

/// `WirelessGatewayName`'s published length ceiling, and `RfRegion`'s.
/// Source: `smithy_query(query="operation", operation="CreateWirelessGateway")` and
/// `smithy_query(query="shape", shape="RfRegion")`.
const NAME_MAX_LEN: usize = 256;
const RF_REGION_MAX_LEN: usize = 64;

async fn create_gateway(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/wireless-gateways", Some(body)).await
}

/// Read a gateway back by one of the modelled identifier kinds.
async fn get_gateway(
    router: &MultiServiceRouter,
    identifier_type: &str,
    identifier: &str,
) -> (StatusCode, Value) {
    let escaped: String = identifier
        .chars()
        .map(|c| if c == ' ' { "%20".to_string() } else { c.to_string() })
        .collect();
    call(
        router,
        Method::GET,
        &format!("/wireless-gateways/{escaped}?identifierType={identifier_type}"),
        None,
    )
    .await
}

/// Read a gateway back by the `Id` the create minted.
async fn get_gateway_by_id(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    get_gateway(router, "WirelessGatewayId", id).await
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

/// The cases spanning the class: each optional member present and absent, every
/// `GatewayEui` separator form the model's pattern admits, and both ends of every
/// published numeric range.
fn cases() -> Vec<Case> {
    vec![
        // The whole LoRaWAN member set, every number at the top of its published range,
        // and the unseparated `GatewayEui` form.
        Case {
            case: "full-high-bounds",
            name: Some("Gateway_1".to_string()),
            description: Some("the whole member set".to_string()),
            lorawan: json!({
                "GatewayEui": "0102030405060708",
                "RfRegion": "US915",
                "JoinEuiFilters": [["0000000000000001", "00000000000000ff"]],
                "NetIdFilters": ["000001", "ffffff"],
                "SubBands": [SUB_BAND_MIN, SUB_BAND_MAX],
                "Beaconing": { "DataRate": DATA_RATE_MAX, "Frequencies": [FREQUENCY_MAX] },
                "MaxEirp": MAX_EIRP_MAX,
            }),
            tags: Some(json!([{ "Key": "stage", "Value": "test" }])),
        },
        // The same member set at the other end of every range, no name, and the
        // colon-separated `GatewayEui` form.
        Case {
            case: "full-low-bounds-no-name",
            name: None,
            description: Some(String::new()),
            lorawan: json!({
                "GatewayEui": "aa:bb:cc:dd:ee:ff:00:11",
                "RfRegion": "EU868",
                "JoinEuiFilters": [],
                "NetIdFilters": [],
                "SubBands": [],
                "Beaconing": { "DataRate": DATA_RATE_MIN, "Frequencies": [FREQUENCY_MIN] },
                "MaxEirp": MAX_EIRP_MIN,
            }),
            tags: None,
        },
        // The dash-separated form, mixed hex case, and only the EUI named.
        Case {
            case: "eui-only-dash-form",
            name: Some("AS923 gateway".to_string()),
            description: None,
            lorawan: json!({ "GatewayEui": "01-02-03-04-05-06-07-aB" }),
            tags: Some(json!([])),
        },
        // The required member present and EMPTY: the gateway carries no LoRaWAN
        // settings at all, so it has no `GatewayEui` to be addressed by either.
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
        // A name and an `RfRegion` at their published ceilings, the space-separated EUI
        // form, a mid-range float `MaxEirp`, and the filter lists at their ceilings.
        Case {
            case: "name-at-ceiling",
            name: Some("x".repeat(NAME_MAX_LEN)),
            description: None,
            lorawan: json!({
                "GatewayEui": "0a 0b 0c 0d 0e 0f 10 11",
                "RfRegion": "r".repeat(RF_REGION_MAX_LEN),
                "JoinEuiFilters": [
                    ["0000000000000001", "0000000000000002"],
                    ["1000000000000001", "1000000000000002"],
                    ["2000000000000001", "2000000000000002"],
                ],
                "NetIdFilters": [
                    "000000", "000001", "000002", "000003", "000004",
                    "000005", "000006", "000007", "000008", "000009",
                ],
                "SubBands": [1, 2, 3, 4, 5, 6, 7, 8],
                "Beaconing": { "DataRate": 7, "Frequencies": [500000000, 600000000] },
                "MaxEirp": 12.5,
            }),
            tags: None,
        },
    ]
}

/// The body a case sends to `CreateWirelessGateway`.
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

/// A create persists the gateway and `GetWirelessGateway` reports exactly what was
/// stored, across both presence states of every optional member, every `GatewayEui`
/// separator form and both ends of the published ranges.
#[tokio::test]
async fn a_wireless_gateway_round_trips_through_get() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for case in cases() {
        let (status, created) = create_gateway(&router, body_for(&case)).await;
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
            "case {:?}: Id {id:?} was already minted for another gateway",
            case.case
        );
        let expected_arn = format!("arn:aws:iotwireless:{REGION}:{ACCOUNT}:WirelessGateway/{id}");
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {:?}: expected Arn {expected_arn}, got {:?}",
            case.case,
            created["Arn"]
        );
        minted.push(id.clone());

        let (status, fetched) = get_gateway_by_id(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: expected 200 from GetWirelessGateway, got {status} {fetched}",
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
            "case {:?}: GetWirelessGateway Arn must match the created ARN, got {:?}",
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
        // `AssociateWirelessGatewayWithThing` is a separate operation, so a gateway the
        // create just minted is associated with no thing.
        for member in ["ThingName", "ThingArn"] {
            assert_eq!(
                fetched.get(member),
                None,
                "case {:?}: a gateway no association has touched must report no {member}, \
                 got {:?}",
                case.case,
                fetched.get(member)
            );
        }
    }
}

/// A minted ARN carries the region the request's SigV4 credential scope names, and falls
/// back to the default region when the request names none.
#[tokio::test]
async fn a_minted_gateway_arn_carries_the_region_the_request_names() {
    let router = router();

    for scope_region in [Some("us-east-2"), Some("ap-south-1"), None] {
        let (status, created) = call_signed_for(
            &router,
            scope_region,
            Method::POST,
            "/wireless-gateways",
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
                "arn:aws:iotwireless:{expected_region}:{ACCOUNT}:WirelessGateway/{id}"
            )),
            "case {case:?}: expected the gateway ARN in {expected_region}, got {:?}",
            created["Arn"]
        );
    }
}

/// Every entry of the modelled `WirelessGatewayIdType` catalog addresses the gateway by
/// one of its OWN persisted fields: `WirelessGatewayId` and `GatewayEui` resolve the
/// stored record, `ThingName` resolves nothing while no association has been made, and
/// an identifier kind the catalog does not list is refused.
#[tokio::test]
async fn every_modelled_identifier_type_addresses_a_persisted_field() {
    let router = router();

    // Every separator form the `GatewayEui` pattern admits, so the lookup is exercised
    // on each of them rather than only the unseparated one.
    let euis = [
        ("unseparated", "1122334455667788"),
        ("dashed", "11-22-33-44-55-66-77-99"),
        ("colons", "11:22:33:44:55:66:77:aa"),
        ("spaces", "11 22 33 44 55 66 77 bb"),
    ];

    for (form, eui) in euis {
        let (status, created) = create_gateway(
            &router,
            json!({ "Name": format!("Gateway-{form}"), "LoRaWAN": { "GatewayEui": eui } }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {form}: expected 201, got {status} {created}"
        );
        let id = created["Id"].as_str().expect("an Id").to_string();

        // (identifier kind, the value that addresses this gateway, whether it resolves)
        let lookups = [
            ("WirelessGatewayId", id.as_str(), true),
            ("GatewayEui", eui, true),
            ("ThingName", "a-thing-nothing-is-associated-with", false),
        ];
        for (identifier_type, identifier, resolves) in lookups {
            assert!(
                WIRELESS_GATEWAY_ID_TYPES.contains(&identifier_type),
                "case {form}/{identifier_type}: not in the modelled catalog \
                 {WIRELESS_GATEWAY_ID_TYPES:?}"
            );
            let (status, fetched) = get_gateway(&router, identifier_type, identifier).await;
            if resolves {
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "case {form}/{identifier_type}: expected 200, got {status} {fetched}"
                );
                assert_eq!(
                    fetched["Id"],
                    json!(id),
                    "case {form}/{identifier_type}: expected Id {id:?}, got {:?}",
                    fetched["Id"]
                );
                assert_eq!(
                    fetched["LoRaWAN"]["GatewayEui"],
                    json!(eui),
                    "case {form}/{identifier_type}: expected GatewayEui {eui:?} read back, \
                     got {:?}",
                    fetched["LoRaWAN"]["GatewayEui"]
                );
            } else {
                assert_eq!(
                    status,
                    StatusCode::NOT_FOUND,
                    "case {form}/{identifier_type}: expected 404, got {status} {fetched}"
                );
                assert_eq!(
                    error_code(&fetched),
                    "ResourceNotFoundException",
                    "case {form}/{identifier_type}: expected ResourceNotFoundException, \
                     got {fetched}"
                );
            }
        }

        // An identifier kind outside the modelled catalog is a bad request, never a
        // lookup that quietly resolves nothing.
        for bad in ["wirelessGatewayId", "DevEui", "Arn", ""] {
            let (status, body) = get_gateway(&router, bad, &id).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "case {form}/identifierType={bad:?}: expected 400, got {status} {body}"
            );
        }
    }
}

/// A `GatewayEui` identifies at most one gateway, so a second gateway claiming one
/// already provisioned is refused and the first keeps the identifier.
#[tokio::test]
async fn a_second_claimant_of_a_gateway_eui_is_refused() {
    let router = router();

    for (case, eui) in [
        ("unseparated", "a1a2a3a4a5a6a7a8"),
        ("dashed", "b1-b2-b3-b4-b5-b6-b7-b8"),
        ("colons", "c1:c2:c3:c4:c5:c6:c7:c8"),
        ("spaces", "d1 d2 d3 d4 d5 d6 d7 d8"),
    ] {
        let (status, first) = create_gateway(
            &router,
            json!({ "Name": format!("First-{case}"), "LoRaWAN": { "GatewayEui": eui } }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case}: expected 201 for the first claimant, got {status} {first}"
        );
        let owner = first["Id"].as_str().expect("an Id").to_string();

        let (status, conflict) = create_gateway(
            &router,
            json!({ "Name": format!("Second-{case}"), "LoRaWAN": { "GatewayEui": eui } }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {case}: a second claimant must be 409, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {case}: expected ConflictException, got {conflict}"
        );
        assert!(
            conflict.get("Id").is_none(),
            "case {case}: a refused create must mint nothing, got {conflict}"
        );

        let (status, fetched) = get_gateway(&router, "GatewayEui", eui).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case}: the EUI must still resolve, got {status} {fetched}"
        );
        assert_eq!(
            fetched["Id"],
            json!(owner),
            "case {case}: the EUI must still name the first gateway {owner:?}, got {:?}",
            fetched["Id"]
        );
        assert_eq!(
            fetched["Name"],
            json!(format!("First-{case}")),
            "case {case}: the refused create must not have overwritten Name, got {:?}",
            fetched["Name"]
        );
    }
}

/// Replaying a `ClientRequestToken` with the same parameters returns the gateway the
/// first call minted; replaying it with different parameters is a 409 that changes
/// nothing.
#[tokio::test]
async fn replaying_a_client_request_token_is_idempotent() {
    let router = router();

    for case in cases() {
        let token = format!("token-{}", case.case);
        let mut body = body_for(&case);
        body["ClientRequestToken"] = json!(token);

        let (status, first) = create_gateway(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: first create, got {status} {first}",
            case.case
        );

        let (status, replay) = create_gateway(&router, body.clone()).await;
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
        let (status, conflict) = create_gateway(&router, changed).await;
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
        let (_, fetched) = get_gateway_by_id(&router, id).await;
        assert_eq!(
            fetched.get("Description").cloned(),
            case.description.as_ref().map(|text| json!(text)),
            "case {:?}: the refused create must not have overwritten Description, got {:?}",
            case.case,
            fetched.get("Description")
        );
    }
}

/// `GetWirelessGateway` answers 404 for an identifier the store does not hold — whatever
/// it looks like — while the stored gateway keeps resolving.
#[tokio::test]
async fn an_unknown_gateway_identifier_is_not_found() {
    let router = router();
    let (_, created) = create_gateway(
        &router,
        json!({ "LoRaWAN": { "GatewayEui": "0f0e0d0c0b0a0908" } }),
    )
    .await;
    let present = created["Id"].as_str().expect("an Id").to_string();

    for (identifier_type, identifier) in [
        ("WirelessGatewayId", "00000000-0000-4000-8000-000000000000"),
        ("WirelessGatewayId", "not-a-uuid"),
        ("WirelessGatewayId", "A"),
        ("WirelessGatewayId", &present.to_uppercase()),
        ("GatewayEui", "0f0e0d0c0b0a0909"),
        ("GatewayEui", "0F0E0D0C0B0A0908"),
        ("ThingName", "no-such-thing"),
    ] {
        let (status, body) = get_gateway(&router, identifier_type, identifier).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {identifier_type}={identifier:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {identifier_type}={identifier:?}: expected ResourceNotFoundException, \
             got {body}"
        );
    }

    let (status, _) = get_gateway_by_id(&router, &present).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the stored gateway must still resolve after the refused reads"
    );
}

/// Each published constraint on the create's members is enforced: a value on either
/// bound is accepted and reads back, one past a bound or outside a pattern is refused
/// and mints nothing.
#[tokio::test]
async fn a_value_outside_a_published_constraint_is_refused() {
    let router = router();

    // (case, the LoRaWAN member set, whether the service accepts it)
    let cases: Vec<(String, Value, bool)> = vec![
        ("MaxEirp at min".to_string(), json!({ "MaxEirp": MAX_EIRP_MIN }), true),
        ("MaxEirp at max".to_string(), json!({ "MaxEirp": MAX_EIRP_MAX }), true),
        (
            "MaxEirp below min".to_string(),
            json!({ "MaxEirp": MAX_EIRP_MIN - 1.0 }),
            false,
        ),
        (
            "MaxEirp above max".to_string(),
            json!({ "MaxEirp": MAX_EIRP_MAX + 1.0 }),
            false,
        ),
        (
            "SubBand at min".to_string(),
            json!({ "SubBands": [SUB_BAND_MIN] }),
            true,
        ),
        (
            "SubBand at max".to_string(),
            json!({ "SubBands": [SUB_BAND_MAX] }),
            true,
        ),
        (
            "SubBand below min".to_string(),
            json!({ "SubBands": [SUB_BAND_MIN - 1] }),
            false,
        ),
        (
            "SubBand above max".to_string(),
            json!({ "SubBands": [SUB_BAND_MAX + 1] }),
            false,
        ),
        (
            "DataRate at min".to_string(),
            json!({ "Beaconing": { "DataRate": DATA_RATE_MIN } }),
            true,
        ),
        (
            "DataRate at max".to_string(),
            json!({ "Beaconing": { "DataRate": DATA_RATE_MAX } }),
            true,
        ),
        (
            "DataRate below min".to_string(),
            json!({ "Beaconing": { "DataRate": DATA_RATE_MIN - 1 } }),
            false,
        ),
        (
            "DataRate above max".to_string(),
            json!({ "Beaconing": { "DataRate": DATA_RATE_MAX + 1 } }),
            false,
        ),
        (
            "Frequency at min".to_string(),
            json!({ "Beaconing": { "Frequencies": [FREQUENCY_MIN] } }),
            true,
        ),
        (
            "Frequency at max".to_string(),
            json!({ "Beaconing": { "Frequencies": [FREQUENCY_MAX] } }),
            true,
        ),
        (
            "Frequency below min".to_string(),
            json!({ "Beaconing": { "Frequencies": [FREQUENCY_MIN - 1] } }),
            false,
        ),
        (
            "Frequency above max".to_string(),
            json!({ "Beaconing": { "Frequencies": [FREQUENCY_MAX + 1] } }),
            false,
        ),
        (
            "NetId in pattern".to_string(),
            json!({ "NetIdFilters": ["00aBcF"] }),
            true,
        ),
        (
            "NetId too short".to_string(),
            json!({ "NetIdFilters": ["00abc"] }),
            false,
        ),
        (
            "NetId not hex".to_string(),
            json!({ "NetIdFilters": ["00abcg"] }),
            false,
        ),
        (
            "GatewayEui not hex".to_string(),
            json!({ "GatewayEui": "not-a-gateway-eui" }),
            false,
        ),
        (
            "GatewayEui too long".to_string(),
            json!({ "GatewayEui": "0102030405060708aa" }),
            false,
        ),
        (
            "GatewayEui mixed separators".to_string(),
            json!({ "GatewayEui": "01-02:03-04:05-06:07-08" }),
            false,
        ),
        (
            "RfRegion at its length ceiling".to_string(),
            json!({ "RfRegion": "r".repeat(RF_REGION_MAX_LEN) }),
            true,
        ),
        (
            "RfRegion past its length ceiling".to_string(),
            json!({ "RfRegion": "r".repeat(RF_REGION_MAX_LEN + 1) }),
            false,
        ),
    ];

    for (case, lorawan, accepted) in cases {
        let (status, answered) = create_gateway(&router, json!({ "LoRaWAN": lorawan })).await;
        if accepted {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {case:?}: expected 201, got {status} {answered}"
            );
            let id = answered["Id"].as_str().expect("an Id");
            let (status, fetched) = get_gateway_by_id(&router, id).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 from the read, got {status} {fetched}"
            );
            assert_echoes(&case, "LoRaWAN", &lorawan, &fetched);
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

/// `Name`'s documented exclusion list is enforced: a name carrying one of the excluded
/// characters is refused and mints nothing, while the same name without it is accepted.
#[tokio::test]
async fn a_name_carrying_an_excluded_character_is_refused() {
    let router = router();

    for excluded in FORBIDDEN_NAME_CHARS {
        let name = format!("Gateway{excluded}One");
        let (status, body) =
            create_gateway(&router, json!({ "Name": name, "LoRaWAN": {} })).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case Name={name:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case Name={name:?}: expected ValidationException, got {body}"
        );
        assert!(
            body.get("Id").is_none(),
            "case Name={name:?}: a refused create must mint nothing, got {body}"
        );
    }

    let (status, body) =
        create_gateway(&router, json!({ "Name": "GatewayOne", "LoRaWAN": {} })).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the same name without an excluded character must be accepted, got {status} {body}"
    );
}

/// The wireless-gateway store is BOUNDED: the create refuses once it is full, and every
/// gateway it accepted up to that point still reads back.
#[tokio::test]
async fn the_wireless_gateway_store_is_bounded() {
    let router = router();
    let mut ids = Vec::new();

    for index in 0..MAX_WIRELESS_GATEWAYS {
        let (status, body) = create_gateway(
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

    let (status, body) = create_gateway(&router, json!({ "LoRaWAN": {} })).await;
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
        let (status, fetched) = get_gateway_by_id(&router, id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {index}: gateway {id} must still read back, got {status} {fetched}"
        );
    }
}

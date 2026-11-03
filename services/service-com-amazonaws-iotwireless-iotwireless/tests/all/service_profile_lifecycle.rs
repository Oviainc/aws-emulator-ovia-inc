// SPDX-License-Identifier: CC0-1.0

//! The service-profile lifecycle, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `CreateServiceProfile` persists the resource it was given — the settings the
//!   request named, plus the default each member's model doc publishes for the ones it
//!   left out — and mints its identity, so `GetServiceProfile` reports back exactly what
//!   was stored and nothing the emulator never set;
//! * a minted `Id` is a fresh uuid and its ARN carries the region the request names;
//! * an `@idempotencyToken` replay with the same parameters returns the first call's
//!   resource, and with different parameters is a 409;
//! * a request a modelled constraint or a documented name rule refuses persists
//!   nothing;
//! * an `Id` the store does not hold is a 404.
//!
//! The wire harness is the one the sibling suites already have: `tests/all/*.rs` are
//! modules of ONE test target, so `router`, `request_signed_for`, `call`, `error_code`,
//! `is_uuid_shaped` and `call_signed_for` are shared from there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The store ceiling the service enforces, so this suite asserts the bound the
/// implementation declares rather than a number copied by hand.
use service_com_amazonaws_iotwireless_iotwireless::state::{DEFAULT_REGION, MAX_SERVICE_PROFILES};

// The ONE wire harness this crate's suites share, and the region and account its
// signed requests mint ARNs in.
use super::destination_and_wireless_device_lifecycle::{
    call, error_code, is_uuid_shaped, router, ACCOUNT, REGION,
};
// The signed-for-a-chosen-region dispatch the sibling profile suite already has.
use super::device_profile_lifecycle::call_signed_for;

/// The characters every `Name` member's model doc excludes ("The following special
/// characters aren't accepted: <>^#~$").
const FORBIDDEN_NAME_CHARS: &[char] =
    service_com_amazonaws_iotwireless_iotwireless::state::FORBIDDEN_NAME_CHARS;

/// `ServiceProfileName`'s published length ceiling.
/// Source: `smithy_query(query="operation", operation="CreateServiceProfile")`.
const NAME_MAX_LEN: usize = 256;

/// The settings a service profile carries when its create named none.
/// Source: each member's doc on `smithy_query(query="shape",
/// shape="LoRaWANServiceProfile")` — `TxPowerIndexMin` "Default: 0",
/// `TxPowerIndexMax` "Default: 15", `NbTransMin` "Default: 0", `NbTransMax`
/// "Default: 3" — and the `DrMin`/`DrMax` defaults the reported
/// `LoRaWANGetServiceProfileInfo` declares.
fn default_settings() -> Value {
    json!({
        "AddGwMetadata": false,
        "DrMin": 0,
        "DrMax": 0,
        "PrAllowed": false,
        "RaAllowed": false,
        "TxPowerIndexMin": 0,
        "TxPowerIndexMax": 15,
        "NbTransMin": 0,
        "NbTransMax": 3,
    })
}

/// [`default_settings`] with `overrides` applied — what the reader must report for a
/// create that named exactly `overrides`.
fn settings_with(overrides: &Value) -> Value {
    let mut expected = default_settings();
    for (key, value) in overrides.as_object().expect("an object of settings") {
        expected[key] = value.clone();
    }
    expected
}

/// The `LoRaWANGetServiceProfileInfo` members no operation this service models can set,
/// so a read must report none of them rather than a value nothing computed.
/// Source: `smithy_query(query="shape", shape="LoRaWANGetServiceProfileInfo")` minus the
/// members `LoRaWANServiceProfile` carries.
const UNSET_REPORTED_MEMBERS: &[&str] = &[
    "UlRate",
    "UlBucketSize",
    "UlRatePolicy",
    "DlRate",
    "DlBucketSize",
    "DlRatePolicy",
    "DevStatusReqFreq",
    "ChannelMask",
    "MinGwDiversity",
];

/// Every published bound of every `LoRaWANServiceProfile` integer member, and the value
/// one step outside each end.
/// Source: `smithy_query(query="shape", shape="LoRaWANServiceProfile")` — `DrMin`,
/// `DrMax`, `TxPowerIndexMin`, `TxPowerIndexMax`, `NbTransMin` and `NbTransMax` all
/// range 0..15.
const RANGED_MEMBERS: &[&str] = &[
    "DrMin",
    "DrMax",
    "TxPowerIndexMin",
    "TxPowerIndexMax",
    "NbTransMin",
    "NbTransMax",
];
const RANGE_MIN: i64 = 0;
const RANGE_MAX: i64 = 15;

async fn create_profile(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/service-profiles", Some(body)).await
}

async fn get_profile(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(router, Method::GET, &format!("/service-profiles/{id}"), None).await
}

/// One create and the read that must report exactly what it stored.
struct Case {
    case: &'static str,
    name: Option<String>,
    /// The `LoRaWAN` member the create carries, if any.
    lorawan: Option<Value>,
    /// What `GetServiceProfile` must report under `LoRaWAN`, if anything.
    expected: Option<Value>,
    tags: Option<Value>,
}

fn cases() -> Vec<Case> {
    vec![
        // Every settable member named, each integer at the top of its published range.
        Case {
            case: "every-member-at-the-high-bound",
            name: Some("ServiceProfile_1".to_string()),
            lorawan: Some(json!({
                "AddGwMetadata": true,
                "DrMin": RANGE_MAX,
                "DrMax": RANGE_MAX,
                "PrAllowed": true,
                "RaAllowed": true,
                "TxPowerIndexMin": RANGE_MAX,
                "TxPowerIndexMax": RANGE_MAX,
                "NbTransMin": RANGE_MAX,
                "NbTransMax": RANGE_MAX,
            })),
            expected: Some(json!({
                "AddGwMetadata": true,
                "DrMin": RANGE_MAX,
                "DrMax": RANGE_MAX,
                "PrAllowed": true,
                "RaAllowed": true,
                "TxPowerIndexMin": RANGE_MAX,
                "TxPowerIndexMax": RANGE_MAX,
                "NbTransMin": RANGE_MAX,
                "NbTransMax": RANGE_MAX,
            })),
            tags: Some(json!([{ "Key": "stage", "Value": "test" }])),
        },
        // The other end of every range, and no name.
        Case {
            case: "every-member-at-the-low-bound-no-name",
            name: None,
            lorawan: Some(json!({
                "AddGwMetadata": false,
                "DrMin": RANGE_MIN,
                "DrMax": RANGE_MIN,
                "PrAllowed": false,
                "RaAllowed": false,
                "TxPowerIndexMin": RANGE_MIN,
                "TxPowerIndexMax": RANGE_MIN,
                "NbTransMin": RANGE_MIN,
                "NbTransMax": RANGE_MIN,
            })),
            expected: Some(settings_with(&json!({
                "TxPowerIndexMax": RANGE_MIN,
                "NbTransMax": RANGE_MIN,
            }))),
            tags: None,
        },
        // No settings at all: the member is present and empty, so the stored resource
        // carries exactly the documented defaults.
        Case {
            case: "no-settings",
            name: Some(String::new()),
            lorawan: Some(json!({})),
            expected: Some(default_settings()),
            tags: Some(json!([])),
        },
        // A subset of the members, mixing booleans and mid-range integers: the named
        // ones are stored, the rest default.
        Case {
            case: "some-members",
            name: Some("Mixed Profile".to_string()),
            lorawan: Some(json!({
                "AddGwMetadata": true,
                "DrMin": 3,
                "DrMax": 7,
                "NbTransMax": 1,
            })),
            expected: Some(settings_with(&json!({
                "AddGwMetadata": true,
                "DrMin": 3,
                "DrMax": 7,
                "NbTransMax": 1,
            }))),
            tags: Some(json!([
                { "Key": "owner", "Value": "fleet-a" },
                { "Key": "stage", "Value": "prod" },
            ])),
        },
        // No technology member at all: the profile carries no LoRaWAN settings, so the
        // reader reports none rather than inventing a default set.
        Case {
            case: "no-lorawan-member",
            name: Some("x".repeat(NAME_MAX_LEN)),
            lorawan: None,
            expected: None,
            tags: None,
        },
    ]
}

/// The body a case sends to `CreateServiceProfile`.
fn body_for(case: &Case) -> Value {
    let mut body = json!({});
    if let Some(name) = &case.name {
        body["Name"] = json!(name);
    }
    if let Some(lorawan) = &case.lorawan {
        body["LoRaWAN"] = lorawan.clone();
    }
    if let Some(tags) = &case.tags {
        body["Tags"] = tags.clone();
    }
    body
}

/// A create persists the profile and `GetServiceProfile` reports exactly what was
/// stored — the named settings, the documented default for every omitted one, and
/// nothing for the members no modelled operation sets.
#[tokio::test]
async fn a_service_profile_round_trips_through_get() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for case in cases() {
        let (status, created) = create_profile(&router, body_for(&case)).await;
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
            "case {:?}: Id {id:?} was already minted for another profile",
            case.case
        );
        let expected_arn = format!("arn:aws:iotwireless:{REGION}:{ACCOUNT}:ServiceProfile/{id}");
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {:?}: expected Arn {expected_arn}, got {:?}",
            case.case,
            created["Arn"]
        );
        minted.push(id.clone());

        let (status, fetched) = get_profile(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: expected 200 from GetServiceProfile, got {status} {fetched}",
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
            "case {:?}: GetServiceProfile Arn must match the created ARN, got {:?}",
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

        match &case.expected {
            Some(expected) => {
                for (key, value) in expected.as_object().expect("an object of settings") {
                    assert_eq!(
                        fetched.get("LoRaWAN").and_then(|found| found.get(key)),
                        Some(value),
                        "case {:?}: expected LoRaWAN.{key} = {value}, got {:?}",
                        case.case,
                        fetched.get("LoRaWAN").and_then(|found| found.get(key))
                    );
                }
                for member in UNSET_REPORTED_MEMBERS {
                    assert!(
                        fetched
                            .get("LoRaWAN")
                            .and_then(|found| found.get(member))
                            .is_none(),
                        "case {:?}: nothing sets LoRaWAN.{member}, so it must not be \
                         reported, got {:?}",
                        case.case,
                        fetched.get("LoRaWAN").and_then(|found| found.get(member))
                    );
                }
            }
            None => assert!(
                fetched.get("LoRaWAN").is_none(),
                "case {:?}: a profile created without LoRaWAN settings must report none, got {:?}",
                case.case,
                fetched.get("LoRaWAN")
            ),
        }
    }
}

/// A minted ARN carries the region the request's SigV4 credential scope names, and
/// falls back to the default region when the request names none.
#[tokio::test]
async fn a_minted_service_profile_arn_carries_the_region_the_request_names() {
    let router = router();

    for scope_region in [Some("us-east-2"), Some("ap-south-1"), None] {
        let (status, created) = call_signed_for(
            &router,
            scope_region,
            Method::POST,
            "/service-profiles",
            Some(json!({ "LoRaWAN": { "AddGwMetadata": true } })),
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
                "arn:aws:iotwireless:{expected_region}:{ACCOUNT}:ServiceProfile/{id}"
            )),
            "case {case:?}: expected the profile ARN in {expected_region}, got {:?}",
            created["Arn"]
        );
    }
}

/// `GetServiceProfile` answers 404 for an `Id` the store does not hold — whatever the
/// id looks like — while the stored profile keeps resolving.
#[tokio::test]
async fn an_unknown_service_profile_id_is_not_found() {
    let router = router();
    let (_, created) = create_profile(&router, json!({ "LoRaWAN": {} })).await;
    let present = created["Id"].as_str().expect("an Id").to_string();

    for id in [
        "00000000-0000-4000-8000-000000000000",
        "not-a-uuid",
        "A",
        &present.to_uppercase(),
    ] {
        let (status, body) = get_profile(&router, id).await;
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

    let (status, _) = get_profile(&router, &present).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the stored profile must still resolve after the refused reads"
    );
}

/// Replaying a `ClientRequestToken` with the same parameters returns the profile the
/// first call minted; replaying it with different parameters is a 409 that changes
/// nothing.
#[tokio::test]
async fn replaying_a_client_request_token_is_idempotent() {
    let router = router();

    for case in cases() {
        let token = format!("token-{}", case.case);
        let mut body = body_for(&case);
        body["ClientRequestToken"] = json!(token);

        let (status, first) = create_profile(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: first create, got {status} {first}",
            case.case
        );

        let (status, replay) = create_profile(&router, body.clone()).await;
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
        changed["Name"] = json!(format!("Changed-{}", case.case));
        let (status, conflict) = create_profile(&router, changed).await;
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
        let (_, fetched) = get_profile(&router, id).await;
        assert_eq!(
            fetched.get("Name").cloned(),
            case.name.as_ref().map(|name| json!(name)),
            "case {:?}: the refused create must not have overwritten Name, got {:?}",
            case.case,
            fetched.get("Name")
        );
    }
}

/// A name carrying any character the model doc excludes is refused, and nothing is
/// stored for it; the same name without that character is accepted.
#[tokio::test]
async fn a_name_with_an_excluded_character_is_refused() {
    let router = router();

    for rejected in FORBIDDEN_NAME_CHARS {
        for name in [
            format!("{rejected}LeadingProfile"),
            format!("Profile{rejected}Middle"),
            format!("TrailingProfile{rejected}"),
        ] {
            let (status, body) =
                create_profile(&router, json!({ "Name": name, "LoRaWAN": {} })).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "case {name:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {name:?}: expected ValidationException, got {body}"
            );
            assert!(
                body.get("Id").is_none(),
                "case {name:?}: a refused create must mint nothing, got {body}"
            );

            let accepted: String = name.chars().filter(|c| c != rejected).collect();
            let (status, created) =
                create_profile(&router, json!({ "Name": accepted, "LoRaWAN": {} })).await;
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {accepted:?}: the same name without {rejected:?} must be accepted, \
                 got {status} {created}"
            );
        }
    }
}

/// Every ranged `LoRaWAN` member is accepted on each published bound — and read back —
/// and refused one step outside either end, storing nothing.
#[tokio::test]
async fn a_setting_outside_its_published_range_is_refused() {
    let router = router();

    for member in RANGED_MEMBERS {
        let member = *member;
        for (value, accepted) in [
            (RANGE_MIN, true),
            (RANGE_MIN + 1, true),
            (RANGE_MAX, true),
            (RANGE_MIN - 1, false),
            (RANGE_MAX + 1, false),
        ] {
            let (status, body) =
                create_profile(&router, json!({ "LoRaWAN": { member: value } })).await;
            if accepted {
                assert_eq!(
                    status,
                    StatusCode::CREATED,
                    "case {member}={value}: expected 201, got {status} {body}"
                );
                let (status, fetched) =
                    get_profile(&router, body["Id"].as_str().expect("an Id")).await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "case {member}={value}: expected 200 from the read, got {status} {fetched}"
                );
                assert_eq!(
                    fetched["LoRaWAN"][member],
                    json!(value),
                    "case {member}={value}: expected it read back, got {:?}",
                    fetched["LoRaWAN"][member]
                );
            } else {
                assert_eq!(
                    status,
                    StatusCode::BAD_REQUEST,
                    "case {member}={value}: expected 400, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {member}={value}: expected ValidationException, got {body}"
                );
            }
        }
    }
}

/// The service-profile store is BOUNDED: the create refuses once it is full, and every
/// profile it accepted up to that point still reads back.
#[tokio::test]
async fn the_service_profile_store_is_bounded() {
    let router = router();
    let mut ids = Vec::new();

    for index in 0..MAX_SERVICE_PROFILES {
        let (status, body) = create_profile(
            &router,
            json!({ "Name": format!("Bounded{index}"), "LoRaWAN": {} }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {index}: expected 201 below the ceiling, got {status} {body}"
        );
        ids.push(body["Id"].as_str().expect("an Id").to_string());
    }

    let (status, body) = create_profile(&router, json!({ "LoRaWAN": {} })).await;
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
        let (status, fetched) = get_profile(&router, id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {index}: profile {id} must still read back, got {status} {fetched}"
        );
    }
}

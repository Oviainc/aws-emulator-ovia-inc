// SPDX-License-Identifier: CC0-1.0

//! The resource log-level override lifecycle, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * a log-level override is held per `(ResourceType, ResourceIdentifier)` pair, so
//!   `GetResourceLogLevel` reports the level `PutResourceLogLevel` last stored for that
//!   pair and nothing else — over every documented resource type and every modelled
//!   `LogLevel`;
//! * the reader fetches the override "if any", so a resource carrying none is answered
//!   with no `LogLevel` member rather than with a level nothing set;
//! * `ResetResourceLogLevel` removes the override "if any": the removal is observable
//!   through the reader and repeating it changes nothing;
//! * an override is an attribute OF a resource, so a `ResourceIdentifier` the account
//!   does not hold is a 404 on all three operations and stores nothing;
//! * a `ResourceType` outside the documented set, and a `LogLevel` outside the modelled
//!   enum, are refused and change no stored override;
//! * the override store is bounded by the per-account limit the operation documents.
//!
//! The wire harness is the one the sibling suites already have: `tests/all/*.rs` are
//! modules of ONE test target, so `call`, `error_code` and `router` are shared from
//! there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The documented resource kinds and the per-account ceiling the service declares, so
/// this suite asserts what the implementation publishes rather than values copied by
/// hand.
use service_com_amazonaws_iotwireless_iotwireless::state::{
    LOG_LEVEL_RESOURCE_TYPES, MAX_RESOURCE_LOG_LEVEL_OVERRIDES,
};

// The ONE wire harness this crate's suites share.
use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// `LogLevel`'s modelled values — the model-generated constant, so the suite and the
/// service cannot drift apart.
const LOG_LEVELS: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::LOG_LEVEL_VALUES;

/// An S3 URI and a role the FUOTA tasks this suite seeds reuse.
const IMAGE: &str = "s3://fuota-images/firmware-v1.bin";
const ROLE: &str = "arn:aws:iam::000000000000:role/iotwireless-fuota";

/// The URI every log-level operation addresses one resource through.
fn log_level_uri(resource_identifier: &str, resource_type: &str) -> String {
    format!("/log-levels/{resource_identifier}?resourceType={resource_type}")
}

async fn put_log_level(
    router: &MultiServiceRouter,
    resource_type: &str,
    resource_identifier: &str,
    log_level: &str,
) -> (StatusCode, Value) {
    call(
        router,
        Method::PUT,
        &log_level_uri(resource_identifier, resource_type),
        Some(json!({ "LogLevel": log_level })),
    )
    .await
}

async fn get_log_level(
    router: &MultiServiceRouter,
    resource_type: &str,
    resource_identifier: &str,
) -> (StatusCode, Value) {
    call(
        router,
        Method::GET,
        &log_level_uri(resource_identifier, resource_type),
        None,
    )
    .await
}

async fn reset_log_level(
    router: &MultiServiceRouter,
    resource_type: &str,
    resource_identifier: &str,
) -> (StatusCode, Value) {
    call(
        router,
        Method::DELETE,
        &log_level_uri(resource_identifier, resource_type),
        None,
    )
    .await
}

/// Seed a destination, panicking on anything but 201 — a wireless device needs one.
async fn seed_destination(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        Method::POST,
        "/destinations",
        Some(json!({
            "Name": name,
            "ExpressionType": "RuleName",
            "Expression": format!("rule-for-{name}"),
            "RoleArn": "arn:aws:iam::000000000000:role/iotwireless-destination",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding destination {name}: expected 201, got {status} {body}"
    );
}

/// Create one resource of `resource_type` and return the id the service minted for it,
/// so every case addresses a resource the account really holds.
async fn seed_resource(
    router: &MultiServiceRouter,
    resource_type: &str,
    label: &str,
    destination: &str,
) -> String {
    let (uri, body) = match resource_type {
        "WirelessDevice" => (
            "/wireless-devices",
            json!({ "Type": "LoRaWAN", "Name": label, "DestinationName": destination }),
        ),
        "WirelessGateway" => (
            "/wireless-gateways",
            json!({ "Name": label, "LoRaWAN": { "RfRegion": "US915" } }),
        ),
        "FuotaTask" => (
            "/fuota-tasks",
            json!({
                "Name": label,
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
            }),
        ),
        other => panic!("case {label}: {other:?} is not a documented log-level resource type"),
    };
    let (status, created) = call(router, Method::POST, uri, Some(body)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding {resource_type} {label}: expected 201, got {status} {created}"
    );
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("seeding {resource_type} {label}: no Id in {created}"))
        .to_string()
}

/// One seeded resource per documented resource type, labelled by `label`.
async fn seed_one_of_each(
    router: &MultiServiceRouter,
    label: &str,
    destination: &str,
) -> Vec<(&'static str, String)> {
    let mut seeded = Vec::new();
    for resource_type in LOG_LEVEL_RESOURCE_TYPES {
        let id = seed_resource(router, resource_type, label, destination).await;
        seeded.push((*resource_type, id));
    }
    seeded
}

/// The level `GetResourceLogLevel` reports for a pair, or `None` when it reports no
/// level at all.
fn reported_level(body: &Value) -> Option<&str> {
    body.get("LogLevel").and_then(Value::as_str)
}

/// `PutResourceLogLevel` persists the override and `GetResourceLogLevel` reports it
/// back, for every documented resource type crossed with every modelled `LogLevel`; a
/// second put replaces the stored level rather than adding a second one.
#[tokio::test]
async fn an_override_round_trips_through_get_for_every_type_and_level() {
    let router = router();
    seed_destination(&router, "LogLevelDest").await;

    for resource_type in LOG_LEVEL_RESOURCE_TYPES {
        for log_level in LOG_LEVELS {
            let case = format!("{resource_type}/{log_level}");
            let id = seed_resource(&router, resource_type, &case, "LogLevelDest").await;

            // Before anything set an override, the reader fetches "if any": the
            // resource resolves and carries no level.
            let (status, body) = get_log_level(&router, resource_type, &id).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 reading a resource with no override, got \
                 {status} {body}"
            );
            assert_eq!(
                reported_level(&body),
                None,
                "case {case:?}: a resource with no override must report no LogLevel, got {body}"
            );

            let (status, body) = put_log_level(&router, resource_type, &id, log_level).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 from PutResourceLogLevel, got {status} {body}"
            );

            let (status, body) = get_log_level(&router, resource_type, &id).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 from GetResourceLogLevel, got {status} {body}"
            );
            assert_eq!(
                reported_level(&body),
                Some(*log_level),
                "case {case:?}: expected LogLevel {log_level:?} read back, got {body}"
            );

            // Every other level, applied in turn to the SAME pair: the last write is
            // what the reader reports, and nothing accumulates.
            for replacement in LOG_LEVELS.iter().filter(|level| *level != log_level) {
                let (status, body) = put_log_level(&router, resource_type, &id, replacement).await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "case {case:?} -> {replacement}: expected 200 from the replacing put, got \
                     {status} {body}"
                );
                let (status, body) = get_log_level(&router, resource_type, &id).await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "case {case:?} -> {replacement}: expected 200 from the read, got {status} \
                     {body}"
                );
                assert_eq!(
                    reported_level(&body),
                    Some(*replacement),
                    "case {case:?} -> {replacement}: expected the replacing level read back, got \
                     {body}"
                );
            }
        }
    }
}

/// `ResetResourceLogLevel` removes the override, which the reader observes; repeating
/// the reset changes nothing, and a later put sets a fresh override.
#[tokio::test]
async fn a_reset_removes_the_override_and_repeats_harmlessly() {
    let router = router();
    seed_destination(&router, "ResetDest").await;

    for (resource_type, id) in seed_one_of_each(&router, "reset", "ResetDest").await {
        for log_level in LOG_LEVELS {
            let case = format!("{resource_type}/{log_level}");
            let (status, body) = put_log_level(&router, resource_type, &id, log_level).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 from the put, got {status} {body}"
            );

            // Two resets in a row: the first removes the override, the second finds
            // none to remove and still succeeds.
            for attempt in 1..=2 {
                let (status, body) = reset_log_level(&router, resource_type, &id).await;
                assert_eq!(
                    status,
                    StatusCode::NO_CONTENT,
                    "case {case:?} reset #{attempt}: expected 204, got {status} {body}"
                );
                let (status, body) = get_log_level(&router, resource_type, &id).await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "case {case:?} reset #{attempt}: expected 200 from the read, got {status} \
                     {body}"
                );
                assert_eq!(
                    reported_level(&body),
                    None,
                    "case {case:?} reset #{attempt}: a reset resource must report no LogLevel, \
                     got {body}"
                );
            }
        }
    }
}

/// An override belongs to ONE `(ResourceType, ResourceIdentifier)` pair: setting or
/// resetting one leaves every other pair exactly as it was.
#[tokio::test]
async fn an_override_is_held_per_resource() {
    let router = router();
    seed_destination(&router, "PerResourceDest").await;

    // Two resources of every documented type, so the independence holds within a type
    // as well as across types.
    let mut pairs: Vec<(&str, String, &str)> = Vec::new();
    for (index, log_level) in [LOG_LEVELS[0], LOG_LEVELS[LOG_LEVELS.len() - 1]]
        .iter()
        .enumerate()
    {
        for resource_type in LOG_LEVEL_RESOURCE_TYPES {
            let label = format!("per-resource-{resource_type}-{index}");
            let id = seed_resource(&router, resource_type, &label, "PerResourceDest").await;
            pairs.push((resource_type, id, log_level));
        }
    }

    for (resource_type, id, log_level) in &pairs {
        let (status, body) = put_log_level(&router, resource_type, id, log_level).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {resource_type}/{id}: expected 200 from the put, got {status} {body}"
        );
    }

    for (resource_type, id, log_level) in &pairs {
        let (status, body) = get_log_level(&router, resource_type, id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {resource_type}/{id}: expected 200 from the read, got {status} {body}"
        );
        assert_eq!(
            reported_level(&body),
            Some(*log_level),
            "case {resource_type}/{id}: expected its own level {log_level:?}, got {body}"
        );
    }

    // Reset the FIRST pair only: every other pair must keep its own level.
    let (reset_type, reset_id, _) = pairs[0].clone();
    let (status, body) = reset_log_level(&router, reset_type, &reset_id).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "resetting {reset_type}/{reset_id}: expected 204, got {status} {body}"
    );

    for (resource_type, id, log_level) in &pairs {
        let (status, body) = get_log_level(&router, resource_type, id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {resource_type}/{id}: expected 200 after the unrelated reset, got {status} \
             {body}"
        );
        let expected = if resource_type == &reset_type && id == &reset_id {
            None
        } else {
            Some(*log_level)
        };
        assert_eq!(
            reported_level(&body),
            expected,
            "case {resource_type}/{id}: expected {expected:?} after resetting \
             {reset_type}/{reset_id}, got {body}"
        );
    }
}

/// A `ResourceIdentifier` the account does not hold is a 404 on every log-level
/// operation, whatever the identifier looks like, and stores nothing — the override of
/// a resource the account does hold keeps resolving.
#[tokio::test]
async fn an_identifier_the_account_does_not_hold_is_not_found() {
    let router = router();
    seed_destination(&router, "AbsentDest").await;
    let seeded = seed_one_of_each(&router, "absent", "AbsentDest").await;

    for (resource_type, present) in &seeded {
        let (status, body) = put_log_level(&router, resource_type, present, LOG_LEVELS[0]).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "seeding the override for {resource_type}: expected 200, got {status} {body}"
        );
    }

    // An id nothing holds: a fresh uuid, a non-uuid string, one character, and a
    // stored id belonging to ANOTHER resource type.
    for (resource_type, present) in &seeded {
        let foreign = seeded
            .iter()
            .find(|(other_type, _)| other_type != resource_type)
            .map(|(_, id)| id.clone())
            .expect("another resource type");
        let absent = [
            "00000000-0000-4000-8000-000000000000".to_string(),
            "not-a-uuid".to_string(),
            "A".to_string(),
            present.to_uppercase(),
            foreign,
        ];

        for identifier in absent {
            let calls = [
                ("Put", put_log_level(&router, resource_type, &identifier, LOG_LEVELS[0]).await),
                ("Get", get_log_level(&router, resource_type, &identifier).await),
                ("Reset", reset_log_level(&router, resource_type, &identifier).await),
            ];
            for (operation, (status, body)) in calls {
                let case = format!("{operation} {resource_type}/{identifier}");
                assert_eq!(
                    status,
                    StatusCode::NOT_FOUND,
                    "case {case:?}: expected 404, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {body}"
                );
            }
        }
    }

    for (resource_type, present) in &seeded {
        let (status, body) = get_log_level(&router, resource_type, present).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {resource_type}: the stored override must still resolve, got {status} {body}"
        );
        assert_eq!(
            reported_level(&body),
            Some(LOG_LEVELS[0]),
            "case {resource_type}: the refused calls must not have changed the override, got \
             {body}"
        );
    }
}

/// A `ResourceType` outside the documented set — and a request that names none at all —
/// is refused on every log-level operation, and changes no stored override.
#[tokio::test]
async fn a_resource_type_outside_the_documented_set_is_refused() {
    let router = router();
    seed_destination(&router, "BadTypeDest").await;
    let id = seed_resource(&router, "WirelessDevice", "bad-type", "BadTypeDest").await;
    let (status, body) = put_log_level(&router, "WirelessDevice", &id, LOG_LEVELS[1]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "seeding the override: expected 200, got {status} {body}"
    );

    // Kinds the service addresses elsewhere but NOT through a log-level override, a
    // mis-cased documented kind, and the empty string.
    for resource_type in [
        "Destination",
        "DeviceProfile",
        "MulticastGroup",
        "SidewalkAccount",
        "wirelessdevice",
        "",
    ] {
        assert!(
            !LOG_LEVEL_RESOURCE_TYPES.contains(&resource_type),
            "case {resource_type:?} is in the documented set {LOG_LEVEL_RESOURCE_TYPES:?}, so it \
             is not a refusal case"
        );
        let calls = [
            ("Put", put_log_level(&router, resource_type, &id, LOG_LEVELS[0]).await),
            ("Get", get_log_level(&router, resource_type, &id).await),
            ("Reset", reset_log_level(&router, resource_type, &id).await),
        ];
        for (operation, (status, body)) in calls {
            let case = format!("{operation} resourceType={resource_type:?}");
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "case {case:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            let message = body
                .get("Message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                message.contains("resourceType"),
                "case {case:?}: the refusal must name the member it refused, got {body}"
            );
        }
    }

    let (status, body) = get_log_level(&router, "WirelessDevice", &id).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the stored override must still resolve, got {status} {body}"
    );
    assert_eq!(
        reported_level(&body),
        Some(LOG_LEVELS[1]),
        "the refused calls must not have changed the stored override, got {body}"
    );
}

/// A `LogLevel` outside the modelled enum, or absent, is refused and leaves the stored
/// override alone.
#[tokio::test]
async fn a_log_level_outside_the_modelled_enum_is_refused() {
    let router = router();
    seed_destination(&router, "BadLevelDest").await;
    let id = seed_resource(&router, "WirelessDevice", "bad-level", "BadLevelDest").await;
    let (status, body) = put_log_level(&router, "WirelessDevice", &id, LOG_LEVELS[0]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "seeding the override: expected 200, got {status} {body}"
    );

    for rejected in ["VERBOSE", "info", "Error", "", "DISABLED "] {
        assert!(
            !LOG_LEVELS.contains(&rejected),
            "case {rejected:?} is in the modelled catalog {LOG_LEVELS:?}, so it is not a refusal \
             case"
        );
        let (status, body) = put_log_level(&router, "WirelessDevice", &id, rejected).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {rejected:?}: expected 400, got {status} {body}"
        );
    }

    // The required `LogLevel` left out entirely.
    let (status, body) = call(
        &router,
        Method::PUT,
        &log_level_uri(&id, "WirelessDevice"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "case missing LogLevel: expected 400, got {status} {body}"
    );

    let (status, body) = get_log_level(&router, "WirelessDevice", &id).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the stored override must still resolve, got {status} {body}"
    );
    assert_eq!(
        reported_level(&body),
        Some(LOG_LEVELS[0]),
        "a refused put must leave the stored override alone, got {body}"
    );
}

/// The override store is BOUNDED by the per-account limit the operation documents: a
/// fresh override is refused once the account is full, replacing one already held is
/// still accepted, and every override the account holds still reads back.
#[tokio::test]
async fn the_override_store_is_bounded() {
    let router = router();
    seed_destination(&router, "BoundedDest").await;

    let mut ids = Vec::new();
    for index in 0..MAX_RESOURCE_LOG_LEVEL_OVERRIDES {
        let id = seed_resource(
            &router,
            "WirelessDevice",
            &format!("bounded-{index}"),
            "BoundedDest",
        )
        .await;
        let log_level = LOG_LEVELS[index % LOG_LEVELS.len()];
        let (status, body) = put_log_level(&router, "WirelessDevice", &id, log_level).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {index}: expected 200 below the ceiling, got {status} {body}"
        );
        ids.push((id, log_level));
    }

    // One more resource, with no override of its own: a NEW override is what the
    // ceiling refuses.
    let overflow = seed_resource(&router, "WirelessDevice", "overflow", "BoundedDest").await;
    let (status, body) = put_log_level(&router, "WirelessDevice", &overflow, LOG_LEVELS[0]).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a full account must refuse a new override, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "a full account must refuse with ValidationException, got {body}"
    );
    let (status, body) = get_log_level(&router, "WirelessDevice", &overflow).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the refused resource must still resolve, got {status} {body}"
    );
    assert_eq!(
        reported_level(&body),
        None,
        "a refused put must store nothing, got {body}"
    );

    // Replacing an override the account already holds adds no entry, so the ceiling
    // cannot bind on it.
    let (held, _) = ids[0].clone();
    let replacement = LOG_LEVELS[LOG_LEVELS.len() - 1];
    let (status, body) = put_log_level(&router, "WirelessDevice", &held, replacement).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "replacing an override at the ceiling must be accepted, got {status} {body}"
    );
    ids[0].1 = replacement;

    for (index, (id, log_level)) in ids.iter().enumerate() {
        let (status, body) = get_log_level(&router, "WirelessDevice", id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {index}: override {id} must still read back, got {status} {body}"
        );
        assert_eq!(
            reported_level(&body),
            Some(*log_level),
            "case {index}: expected LogLevel {log_level:?}, got {body}"
        );
    }

    // Resetting one override frees exactly one slot, so the refused resource can take
    // an override now.
    let (freed, _) = ids[0].clone();
    let (status, body) = reset_log_level(&router, "WirelessDevice", &freed).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "resetting {freed}: expected 204, got {status} {body}"
    );
    let (status, body) = put_log_level(&router, "WirelessDevice", &overflow, LOG_LEVELS[0]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a freed slot must accept a new override, got {status} {body}"
    );
}

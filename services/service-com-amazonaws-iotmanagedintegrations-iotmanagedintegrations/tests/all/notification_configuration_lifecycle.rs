// SPDX-License-Identifier: CC0-1.0

//! The notification-configuration lifecycle, end to end over the wire:
//! `CreateDestination` -> `GetDestination` -> `CreateNotificationConfiguration` ->
//! `GetNotificationConfiguration`.
//!
//! RULE: a notification configuration says WHERE this account's notifications of one
//! `EventType` are delivered, and the only place it can name is a destination the account
//! already has. `CreateDestination` (POST `/destinations`, success 201) mints a destination
//! under the `Name` it was given and `GetDestination` (GET `/destinations/{Name}`,
//! `@readonly`) reports back exactly what was stored.
//! `CreateNotificationConfiguration` (POST `/notification-configurations`, success 201)
//! RESOLVES its `DestinationName` against that store: a name the account has no destination
//! under is refused with the modelled `ValidationException` (HTTP 400) — the model declares
//! no `ResourceNotFoundException` for this operation — and creates nothing, so
//! `GetNotificationConfiguration` (GET `/notification-configurations/{EventType}`,
//! `@readonly`) still reports the modelled `ResourceNotFoundException` (HTTP 404) for that
//! event type — stated BARE, carrying none of the exception's optional members, as every
//! 404 of this surface and of the destination surface is. Against a destination the account
//! does have, the create is accepted and the
//! get reports the event type, the destination it was pointed at and its timestamps. One
//! configuration exists per event type, so a second create for the same event type is the
//! modelled `ConflictException` (HTTP 409), while a retry carrying the `ClientToken`
//! (`@idempotencyToken`) of a create that already succeeded answers with that create's
//! event type and performs no further action.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.notification-configuration-lifecycle`) is ONE row of the
//! tables below. The rows differ along what the rule generalises over: the `EventType` (the
//! model's whole value set), the form and length of the `DestinationName` (the characters
//! the `@pattern` admits, one character, and the modelled maximum), why the name resolves to
//! nothing (never created, deleted-region mismatch, differing only in case or spacing), the
//! optional members a destination carries, and the region the request is signed for.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::ops::types::{
    DELIVERY_DESTINATION_TYPE_VALUES, EVENT_TYPE_VALUES,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The wire error codes the model's `@httpError` shapes are rendered under.
const VALIDATION: &str = "ValidationException";
const CONFLICT: &str = "ConflictException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The `@length` maximum of `DestinationName`, in characters
/// (`smithy_query(query="shape",shape="DestinationName")` -> length 1..128, pattern
/// `^[\p{L}\p{N} ._-]+$`).
const DESTINATION_NAME_MAX_LENGTH: usize = 128;

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    fn seconds(&self, name: &str) -> Option<f64> {
        self.member(name).and_then(Value::as_f64)
    }

    /// The reason an error body carries, on the member the model declares for it.
    fn message(&self) -> &str {
        self.string("Message")
            .or_else(|| self.string("message"))
            .unwrap_or_default()
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// The create request body of a destination called `name`, carrying only the four required
/// members. The ARNs satisfy the modelled `DeliveryDestinationArn` pattern
/// (`^arn:aws:[0-9a-zA-Z]+:[0-9a-zA-Z-]+:[0-9]+:[0-9a-zA-Z]+/[0-9a-zA-Z._-]+$`), and the
/// delivery target lives in the SAME region as the call, which is what a destination
/// requires of it.
fn destination_request(name: &str) -> Value {
    destination_request_in(REGION, name)
}

/// A `DeliveryDestinationRoleArn` naming the IAM role `role` of THIS account
/// (`engine::EMULATOR_ACCOUNT_ID`): a destination call resolves its `RoleArn` as a role of
/// the account the call is made for, so a value naming no role of it is refused
/// (`tests/all/referenced_resource_validation.rs`).
fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{}:role/{role}", engine::EMULATOR_ACCOUNT_ID)
}

/// [`destination_request`] whose delivery target lives in `region`, so a destination can
/// be created through another region's endpoint.
fn destination_request_in(region: &str, name: &str) -> Value {
    json!({
        "Name": name,
        "DeliveryDestinationArn":
            format!("arn:aws:kinesis:{region}:123456789012:stream/notifications"),
        "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
        "RoleArn": role_arn("managed-integrations"),
    })
}

/// `CreateDestination` with `body`, signed for `region`.
async fn create_destination_in(router: &MultiServiceRouter, region: &str, body: Value) -> Reply {
    call_in(router, region, "POST", "/destinations", Some(body)).await
}

/// Create the destination `name` in [`REGION`], asserting the modelled 201.
async fn destination(router: &MultiServiceRouter, case: &str, name: &str) {
    let created = create_destination_in(router, REGION, destination_request(name)).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateDestination, got {} with body {:?}",
        created.status, created.body
    );
    assert_eq!(
        created.string("Name"),
        Some(name),
        "case {case:?}: expected the create to answer the destination name {name:?}, got {:?}",
        created.string("Name")
    );
}

/// `GetDestination` for `name`, signed for `region`. The name goes into the URI as the
/// modelled `@httpLabel`, so a case that reads one back names it in URI-safe characters.
async fn get_destination_in(router: &MultiServiceRouter, region: &str, name: &str) -> Reply {
    call_in(router, region, "GET", &format!("/destinations/{name}"), None).await
}

/// `CreateNotificationConfiguration` with `body`, signed for `region`.
async fn create_configuration_in(router: &MultiServiceRouter, region: &str, body: Value) -> Reply {
    call_in(
        router,
        region,
        "POST",
        "/notification-configurations",
        Some(body),
    )
    .await
}

/// [`create_configuration_in`] in [`REGION`], for `event_type` pointed at `destination`.
async fn create_configuration(
    router: &MultiServiceRouter,
    event_type: &str,
    destination: &str,
) -> Reply {
    create_configuration_in(
        router,
        REGION,
        json!({"EventType": event_type, "DestinationName": destination}),
    )
    .await
}

/// `GetNotificationConfiguration` for `event_type`, signed for `region`.
async fn get_configuration_in(router: &MultiServiceRouter, region: &str, event_type: &str) -> Reply {
    call_in(
        router,
        region,
        "GET",
        &format!("/notification-configurations/{event_type}"),
        None,
    )
    .await
}

/// [`get_configuration_in`] in [`REGION`].
async fn get_configuration(router: &MultiServiceRouter, event_type: &str) -> Reply {
    get_configuration_in(router, REGION, event_type).await
}

/// `UpdateNotificationConfiguration` of `event_type` to `destination`, signed for [`REGION`].
async fn update_configuration(
    router: &MultiServiceRouter,
    event_type: &str,
    destination: &str,
) -> Reply {
    call_in(
        router,
        REGION,
        "PUT",
        &format!("/notification-configurations/{event_type}"),
        Some(json!({"DestinationName": destination})),
    )
    .await
}

/// The members the modelled `ResourceNotFoundException` declares, all optional
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")` -> `Message`,
/// `ResourceId`, `ResourceType`, `httpError` 404) — none of which a 404 of the
/// notification-configuration or the destination surface carries.
const NOT_FOUND_MEMBERS: [&str; 3] = ["Message", "ResourceId", "ResourceType"];

/// Assert one answer is the modelled refusal `status`/`code`. Every refusal of this surface
/// states a reason on the modelled `Message` member EXCEPT the 404 for a resource the region
/// holds none of, which is the BARE exception — the status and the error code alone, with
/// none of [`NOT_FOUND_MEMBERS`] on the body a client reads.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected the modelled {code} code, got {:?}",
        reply.error_type
    );
    if code == NOT_FOUND {
        for member in NOT_FOUND_MEMBERS {
            assert!(
                reply.member(member).is_none(),
                "case {case:?}: a 404 of this surface carries no {member}, got {:?} in body {:?}",
                reply.member(member),
                reply.body
            );
        }
    } else {
        assert!(
            !reply.message().is_empty(),
            "case {case:?}: expected the modelled Message member to carry a reason, got body {:?}",
            reply.body
        );
    }
}

/// Assert one get reports the configuration of `event_type` pointed at `destination`.
fn assert_reports(case: &str, reply: &Reply, event_type: &str, destination: &str) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("EventType"),
        Some(event_type),
        "case {case:?}: expected EventType {event_type:?}, got {:?} in body {:?}",
        reply.string("EventType"),
        reply.body
    );
    assert_eq!(
        reply.string("DestinationName"),
        Some(destination),
        "case {case:?}: expected DestinationName {destination:?}, got {:?} in body {:?}",
        reply.string("DestinationName"),
        reply.body
    );
    for stamp in ["CreatedAt", "UpdatedAt"] {
        assert!(
            reply.seconds(stamp).is_some_and(|secs| secs > 0.0),
            "case {case:?}: expected a {stamp} timestamp, got {:?}",
            reply.member(stamp)
        );
    }
}

/// Destination names spanning what the `DestinationName` `@pattern` (`^[\p{L}\p{N} ._-]+$`,
/// length 1..128) admits: letters and digits, every punctuation character it allows, a
/// space, a non-ASCII letter (`\p{L}` is not ASCII-only), one character, and exactly the
/// modelled maximum.
fn destination_names() -> Vec<String> {
    vec![
        "notifications".to_string(),
        "Notifications1".to_string(),
        "notify.stream_1-a".to_string(),
        "two words".to_string(),
        "benachrichtigungenüber".to_string(),
        "d".to_string(),
        "n".repeat(DESTINATION_NAME_MAX_LENGTH),
    ]
}

/// RULE: a create against a destination the account has is accepted, and the get reports the
/// event type and the destination it was pointed at — for every `EventType` the model
/// declares and for destination names spanning the whole admitted name class.
#[tokio::test]
async fn a_create_against_an_existing_destination_round_trips() {
    let names = destination_names();
    for (row, event_type) in EVENT_TYPE_VALUES.iter().enumerate() {
        // A fresh store per row, so the only configuration it holds is the row's own.
        let router = emulator();
        // The destination-name forms cycle over the event types, so every row pairs a
        // different event type with a different name form.
        let name = &names[row % names.len()];
        let case = format!("{event_type} -> {name}");

        // Before the destination exists the create is refused and nothing is created.
        assert_refused(
            &format!("{case}/before the destination exists"),
            &create_configuration(&router, event_type, name).await,
            400,
            VALIDATION,
        );
        assert_refused(
            &format!("{case}/nothing created"),
            &get_configuration(&router, event_type).await,
            404,
            NOT_FOUND,
        );

        destination(&router, &case, name).await;
        let created = create_configuration(&router, event_type, name).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateNotificationConfiguration, got {} \
             with body {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.string("EventType"),
            Some(*event_type),
            "case {case:?}: expected the create to answer the event type it configured, got {:?}",
            created.string("EventType")
        );

        assert_reports(&case, &get_configuration(&router, event_type).await, event_type, name);
        // A get is a pure read: repeating it reports the same thing.
        assert_reports(
            &format!("{case}/reread"),
            &get_configuration(&router, event_type).await,
            event_type,
            name,
        );

        // Only the configured event type has one: every other event type still reports the
        // modelled 404.
        for other in EVENT_TYPE_VALUES.iter().filter(|other| *other != event_type) {
            let read = get_configuration(&router, other).await;
            assert_refused(&format!("{case}/{other} unconfigured"), &read, 404, NOT_FOUND);
        }
    }
}

/// RULE: a `DestinationName` the account has no destination under is the modelled 400 and
/// creates nothing — whatever the reason the name resolves to nothing.
#[tokio::test]
async fn a_create_naming_a_destination_that_does_not_exist_is_refused() {
    let router = emulator();

    // One destination that DOES exist, so the refusals below are about the name and not
    // about an empty store, plus one in another region to show the resolution is regional.
    destination(&router, "existing", "notifications").await;
    let elsewhere = create_destination_in(
        &router,
        "eu-central-1",
        destination_request_in("eu-central-1", "elsewhere.stream"),
    )
    .await;
    assert_eq!(
        elsewhere.status, 201,
        "case \"eu-central-1 destination\": expected HTTP 201, got {} with body {:?}",
        elsewhere.status, elsewhere.body
    );

    // The rows differ along why the name names no destination of this region: nothing ever
    // created it, it differs from an existing name only in case or in spacing, or it belongs
    // to another region's endpoint.
    let cases: Vec<(&str, &str)> = vec![
        ("never created", "sim2real-bogus-destination-name"),
        ("never created, one character", "z"),
        ("differs in case", "Notifications"),
        ("differs by a space", "notifications 2"),
        ("another region's destination", "elsewhere.stream"),
    ];

    for (why, name) in cases {
        let refused = create_configuration(&router, "DEVICE_STATE", name).await;
        assert_refused(why, &refused, 400, VALIDATION);
        assert!(
            refused.string("EventType").is_none(),
            "case {why:?}: a refused create configures no event type, got {:?}",
            refused.body
        );
        // Nothing was created: the event type still has no configuration.
        assert_refused(
            &format!("{why}/nothing created"),
            &get_configuration(&router, "DEVICE_STATE").await,
            404,
            NOT_FOUND,
        );
    }

    // The name that DOES resolve is accepted in the same store, so the refusals above are
    // the resolution and nothing else.
    let accepted = create_configuration(&router, "DEVICE_STATE", "notifications").await;
    assert_eq!(
        accepted.status, 201,
        "case \"the existing destination\": expected HTTP 201, got {} with body {:?}",
        accepted.status, accepted.body
    );
    assert_reports(
        "the existing destination",
        &get_configuration(&router, "DEVICE_STATE").await,
        "DEVICE_STATE",
        "notifications",
    );

    // And the other region's destination resolves through its own endpoint.
    let there = create_configuration_in(
        &router,
        "eu-central-1",
        json!({"EventType": "DEVICE_STATE", "DestinationName": "elsewhere.stream"}),
    )
    .await;
    assert_eq!(
        there.status, 201,
        "case \"eu-central-1 configuration\": expected HTTP 201, got {} with body {:?}",
        there.status, there.body
    );
}

/// RULE: an event type carries ONE configuration, so a second create for it is the modelled
/// 409 and leaves the stored configuration untouched — while a retry under the `ClientToken`
/// of a create that already succeeded answers with that create's event type and performs no
/// further action.
#[tokio::test]
async fn a_second_create_for_the_same_event_type_conflicts_and_a_replayed_token_does_not() {
    // The rows differ along the token's form (the `ClientToken` `@pattern` is
    // `^[a-zA-Z0-9=_-]+$`, length 1..64) and along the event type that is repeated.
    let cases: Vec<(&str, &str)> = vec![
        ("t", "DEVICE_STATE"),
        ("token-1", "DEVICE_OTA"),
        ("TOKEN_2=", "ACCOUNT_ASSOCIATION"),
        (
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "DEVICE_LIFE_CYCLE",
        ),
    ];

    for (token, event_type) in cases {
        let router = emulator();
        let case = format!("{token}/{event_type}");
        destination(&router, &case, "notifications").await;
        destination(&router, &case, "other.stream").await;

        let body = json!({
            "EventType": event_type,
            "DestinationName": "notifications",
            "ClientToken": token,
        });
        let first = create_configuration_in(&router, REGION, body.clone()).await;
        assert_eq!(
            first.status, 201,
            "case {case:?}: expected HTTP 201 from the first create, got {} with body {:?}",
            first.status, first.body
        );

        // Every retry under the same token answers with the first call's event type.
        for attempt in 2..=3 {
            let retry = create_configuration_in(&router, REGION, body.clone()).await;
            assert_eq!(
                retry.status, 201,
                "case {case:?}: expected the retry (attempt {attempt}) to be HTTP 201, got {} \
                 with body {:?}",
                retry.status, retry.body
            );
            assert_eq!(
                retry.string("EventType"),
                Some(event_type),
                "case {case:?}: expected the retry to answer {event_type:?}, got {:?}",
                retry.string("EventType")
            );
        }

        // The same event type under another token, or with no token at all, is an ordinary
        // duplicate: the modelled conflict.
        let other_token = format!("z{}", &token[1..]);
        for (why, mut duplicate) in [
            ("another token", body.clone()),
            (
                "no token",
                json!({"EventType": event_type, "DestinationName": "other.stream"}),
            ),
        ] {
            if why == "another token" {
                duplicate["ClientToken"] = json!(other_token);
                duplicate["DestinationName"] = json!("other.stream");
            }
            let refused = create_configuration_in(&router, REGION, duplicate).await;
            assert_refused(&format!("{case}/{why}"), &refused, 409, CONFLICT);
        }

        // The stored configuration is still the first one: neither the retries nor the
        // refusals re-pointed it.
        assert_reports(
            &case,
            &get_configuration(&router, event_type).await,
            event_type,
            "notifications",
        );
    }
}

/// RULE: an update RE-POINTS an existing configuration at another destination the account
/// has, and changes nothing when the configuration does not exist or the destination does
/// not — so a get after a refused update still reports the destination that was stored.
#[tokio::test]
async fn an_update_re_points_an_existing_configuration() {
    let router = emulator();

    destination(&router, "first", "notifications").await;
    destination(&router, "second", "other.stream").await;

    // The rows differ along the event type whose configuration is re-pointed, exercising
    // the whole `EventType` value set.
    for event_type in EVENT_TYPE_VALUES {
        let case = (*event_type).to_string();

        // An event type with no configuration is not created by an update.
        let missing = update_configuration(&router, event_type, "notifications").await;
        assert_refused(&format!("{case}/before the create"), &missing, 404, NOT_FOUND);
        assert_refused(
            &format!("{case}/nothing created"),
            &get_configuration(&router, event_type).await,
            404,
            NOT_FOUND,
        );

        let created = create_configuration(&router, event_type, "notifications").await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from the create, got {} with body {:?}",
            created.status, created.body
        );

        // A destination the account does not have is refused and re-points nothing.
        let refused = update_configuration(&router, event_type, "nosuch.stream").await;
        assert_refused(&format!("{case}/unknown destination"), &refused, 400, VALIDATION);
        assert_reports(
            &format!("{case}/unchanged"),
            &get_configuration(&router, event_type).await,
            event_type,
            "notifications",
        );

        // A destination it does have re-points the configuration, and the get reports the
        // new one.
        let updated = update_configuration(&router, event_type, "other.stream").await;
        assert_eq!(
            updated.status, 201,
            "case {case:?}: expected HTTP 201 from the update, got {} with body {:?}",
            updated.status, updated.body
        );
        assert_reports(
            &format!("{case}/re-pointed"),
            &get_configuration(&router, event_type).await,
            event_type,
            "other.stream",
        );

        // The update is `@idempotent`: repeating it leaves the same destination.
        let again = update_configuration(&router, event_type, "other.stream").await;
        assert_eq!(
            again.status, 201,
            "case {case:?}: expected the repeated update to be HTTP 201, got {} with body {:?}",
            again.status, again.body
        );
        assert_reports(
            &format!("{case}/re-pointed again"),
            &get_configuration(&router, event_type).await,
            event_type,
            "other.stream",
        );
    }
}

/// RULE: a create mints a destination under the name it was given and a get reports back
/// exactly what was stored — including the optional members — while a name the region holds
/// no destination under is the modelled 404 and a repeated name is the modelled 409.
#[tokio::test]
async fn a_destination_round_trips_and_its_name_identifies_it() {
    let router = emulator();

    // The rows differ along which optional members the create carries. Names stay inside
    // URI-safe characters here because `GetDestination` carries the name as an
    // `@httpLabel`; the whole admitted name class is covered by the table above.
    let cases: Vec<(&str, Value)> = vec![
        ("required members only", destination_request("plain.stream")),
        (
            "with a description",
            json!({
                "Name": "described.stream",
                "DeliveryDestinationArn":
                    "arn:aws:kinesis:us-east-1:123456789012:stream/other_notifications",
                "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
                "RoleArn": role_arn("other-role"),
                // `DestinationDescription` is modelled pattern `^[0-9A-Za-z_\- ]+$`.
                "Description": "events for the test",
            }),
        ),
        (
            "with tags",
            json!({
                "Name": "tagged.stream",
                "DeliveryDestinationArn":
                    "arn:aws:kinesis:us-east-1:123456789012:stream/tagged",
                "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
                "RoleArn": role_arn("tagged-role"),
                "Tags": {"env": "test"},
            }),
        ),
    ];

    for (case, request) in cases {
        let created = create_destination_in(&router, REGION, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {:?}",
            created.status, created.body
        );
        let name = request["Name"].as_str().expect("a name in the request");

        let read = get_destination_in(&router, REGION, name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetDestination, got {} with body {:?}",
            read.status, read.body
        );
        for member in [
            "Name",
            "DeliveryDestinationArn",
            "DeliveryDestinationType",
            "RoleArn",
            "Description",
        ] {
            assert_eq!(
                read.string(member),
                request.get(member).and_then(Value::as_str),
                "case {case:?}: expected {member} to be what the create stored, got {:?}",
                read.string(member)
            );
        }
        assert_eq!(
            read.member("Tags"),
            request.get("Tags"),
            "case {case:?}: expected Tags to be what the create stored, got {:?}",
            read.member("Tags")
        );
        for stamp in ["CreatedAt", "UpdatedAt"] {
            assert!(
                read.seconds(stamp).is_some_and(|secs| secs > 0.0),
                "case {case:?}: expected a {stamp} timestamp, got {:?}",
                read.member(stamp)
            );
        }

        // The name identifies the destination, so creating it again is the modelled
        // conflict and the stored record is untouched.
        let again = create_destination_in(&router, REGION, request.clone()).await;
        assert_refused(&format!("{case}/again"), &again, 409, CONFLICT);
        let reread = get_destination_in(&router, REGION, name).await;
        assert_eq!(
            reread.string("DeliveryDestinationArn"),
            read.string("DeliveryDestinationArn"),
            "case {case:?}: the refused create left the stored destination untouched"
        );
    }

    // A name this region holds no destination under is the modelled 404, stated with the
    // BARE exception: none of the shape's optional members is on the body.
    for name in ["nothing.here", "plain.stream.2", "z"] {
        let missing = get_destination_in(&router, REGION, name).await;
        assert_refused(name, &missing, 404, NOT_FOUND);
    }
}

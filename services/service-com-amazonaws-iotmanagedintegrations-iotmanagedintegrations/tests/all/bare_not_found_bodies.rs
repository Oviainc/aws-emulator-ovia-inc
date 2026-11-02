// SPDX-License-Identifier: CC0-1.0

//! Which members a `ResourceNotFoundException` body carries, over the wire.
//!
//! RULE: the members of the modelled `ResourceNotFoundException` that a 404 fills belong to
//! the RESOURCE whose absence is being stated, not to the operation that resolved it — all
//! three (`Message`, `ResourceId`, `ResourceType`) are optional on the shape
//! (`smithy_query(query="shape",shape="ResourceNotFoundException")` -> members `Message`,
//! `ResourceId`, `ResourceType`, `httpError` 404). The destination resource and the
//! notification-configuration resource are stated BARE: HTTP 404 and the
//! `x-amzn-errortype` code alone, with none of the three on the body, by EVERY operation
//! that resolves one and finds none — a read and a mutation alike, whatever made the
//! resource absent. The managed thing is the contrast: its own path fills all three, so the
//! rule is scoped to the resource and is not a blanket emptying of every 404.
//!
//! The recorded units this was derived from
//! (`iot-managed-integrations-destination-lifecycle` tasks 1-3 and
//! `…-notification-configuration-error-paths` tasks 1-2) are five rows of the tables below.
//! The rows differ along what the rule generalises over: the operation that resolves the
//! resource (`GetDestination`, `UpdateDestination`, `DeleteDestination`,
//! `GetNotificationConfiguration`, `UpdateNotificationConfiguration`), the form and length
//! of the key inside its modelled constraints, the whole `EventType` value set, and why the
//! resource is absent (never created, deleted, or held only by another region).
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the statuses, headers and bodies asserted here are the ones a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::ops::types::{
    DELIVERY_DESTINATION_TYPE_VALUES, EVENT_TYPE_VALUES,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::MANAGED_THING_RESOURCE_TYPE;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The wire error code the model's `@httpError` 404 shape is rendered under.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// Every member the modelled `ResourceNotFoundException` declares, all optional
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`).
const NOT_FOUND_MEMBERS: [&str; 3] = ["Message", "ResourceId", "ResourceType"];

/// The `@length` maximum of `DestinationName`, in characters
/// (`smithy_query(query="operation",operation="GetDestination")` -> `Name` len 1..128,
/// pattern `^[\p{L}\p{N} ._-]+$`).
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

/// [`call_in`] in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    call_in(router, REGION, method, path, body).await
}

/// `name` as an SDK puts it into the URI: every character outside the unreserved set
/// (RFC 3986 `ALPHA / DIGIT / "-" / "." / "_" / "~"`) percent-encoded, which is what a
/// `@httpLabel` carrying a space or a non-ASCII letter looks like on the wire.
fn encode_label(name: &str) -> String {
    let mut encoded = String::new();
    for byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// One operation that resolves a resource by the key its URI carries: its name, its HTTP
/// method, the path the key goes into, and the body it sends (`None` for the methods that
/// carry none).
struct Resolving {
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// The three operations of the destination surface, addressing `name`
/// (`smithy_query(query="digest")`: GET, PUT and DELETE on `/destinations/{Name}`).
fn destination_operations(name: &str) -> Vec<Resolving> {
    let path = format!("/destinations/{}", encode_label(name));
    vec![
        Resolving {
            operation: "GetDestination",
            method: "GET",
            path: path.clone(),
            body: None,
        },
        // An update that asks for one member to change: the 404 comes from resolving the
        // name, not from what the body holds.
        Resolving {
            operation: "UpdateDestination",
            method: "PUT",
            path: path.clone(),
            body: Some(json!({ "Description": "a description" })),
        },
        Resolving {
            operation: "DeleteDestination",
            method: "DELETE",
            path,
            body: None,
        },
    ]
}

/// The two operations of the notification-configuration surface that resolve `event_type`
/// (GET and PUT on `/notification-configurations/{EventType}`). The update's required
/// `DestinationName` names a destination the region DOES hold, so the only thing missing in
/// such a case is the configuration itself.
fn notification_configuration_operations(event_type: &str, destination: &str) -> Vec<Resolving> {
    let path = format!("/notification-configurations/{}", encode_label(event_type));
    vec![
        Resolving {
            operation: "GetNotificationConfiguration",
            method: "GET",
            path: path.clone(),
            body: None,
        },
        Resolving {
            operation: "UpdateNotificationConfiguration",
            method: "PUT",
            path,
            body: Some(json!({ "DestinationName": destination })),
        },
    ]
}

/// The create body of a destination called `name` delivering into `region`. A destination
/// delivers to a target of the SAME region as the call, which is what its create requires,
/// AS an IAM role of this account (`engine::EMULATOR_ACCOUNT_ID`), which is what its
/// `RoleArn` has to name.
fn destination_request(region: &str, name: &str) -> Value {
    json!({
        "Name": name,
        "DeliveryDestinationArn":
            format!("arn:aws:kinesis:{region}:123456789012:stream/bare-not-found"),
        "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
        "RoleArn":
            format!("arn:aws:iam::{}:role/managed-integrations", engine::EMULATOR_ACCOUNT_ID),
    })
}

/// Create the destination `name` in `region`, asserting the modelled 201.
async fn seed_destination(router: &MultiServiceRouter, case: &str, region: &str, name: &str) {
    let created = call_in(
        router,
        region,
        "POST",
        "/destinations",
        Some(destination_request(region, name)),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateDestination, got {} with body {:?}",
        created.status, created.body
    );
}

/// The members of the modelled exception the body actually carries, matched without regard
/// to case so a lower-cased spelling is caught as well as the modelled one.
fn declared_members_present(reply: &Reply) -> Vec<String> {
    let Some(object) = reply.body.as_object() else {
        return Vec::new();
    };
    object
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, _)| key.clone())
        .filter(|key| {
            NOT_FOUND_MEMBERS
                .iter()
                .any(|member| member.eq_ignore_ascii_case(key))
        })
        .collect()
}

/// Assert `reply` states the absence BARE: the 404 and the modelled error code, and not one
/// of the members the exception declares.
fn assert_bare_not_found(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(RESOURCE_NOT_FOUND),
        "case {case:?}: expected the modelled {RESOURCE_NOT_FOUND} code, got {:?}",
        reply.error_type
    );
    let present = declared_members_present(reply);
    assert!(
        present.is_empty(),
        "case {case:?}: expected none of {NOT_FOUND_MEMBERS:?} on the body, got {present:?} \
         in body {:?}",
        reply.body
    );
}

/// RULE: every operation of the destination surface states a `Name` the caller's region
/// holds no destination under with the bare exception, for every form of name the model
/// admits.
#[tokio::test]
async fn every_destination_operation_states_an_absent_name_bare() {
    let router = emulator();

    // One destination that DOES exist, so the refusals below are about the name asked for
    // and not about an empty store.
    seed_destination(&router, "seed", REGION, "bare.not.found.seed").await;

    // `DestinationName`: `@length` 1..128, `@pattern` `^[\p{L}\p{N} ._-]+$`. The rows differ
    // along the name's form: both ends of the length bound, a space, the punctuation the
    // pattern admits, a non-ASCII letter, and digits only.
    let longest = "d".repeat(DESTINATION_NAME_MAX_LENGTH);
    let names: Vec<(&str, String)> = vec![
        ("one character, the length minimum", "z".to_string()),
        ("128 characters, the length maximum", longest),
        ("a space", "two words".to_string()),
        (
            "the punctuation the pattern admits",
            "dot.and-dash_1".to_string(),
        ),
        ("a non-ASCII letter", "caffè".to_string()),
        ("digits only", "0123456789".to_string()),
    ];

    for (form, name) in &names {
        for resolving in destination_operations(name) {
            let case = format!("{}/{form}", resolving.operation);
            let reply = call(&router, resolving.method, &resolving.path, resolving.body).await;
            assert_bare_not_found(&case, &reply);
        }
    }

    // The store still holds the destination that was seeded, so none of the refusals above
    // was about an empty store.
    let seeded = call(&router, "GET", "/destinations/bare.not.found.seed", None).await;
    assert_eq!(
        seeded.status, 200,
        "case \"seed\": expected the seeded destination to still resolve, got {} with body {:?}",
        seeded.status, seeded.body
    );
}

/// RULE: the same bare body states a destination that is absent for any other cause — one
/// that was created and deleted, and one that exists only in another region — so nothing
/// about the body depends on the record never having existed.
#[tokio::test]
async fn a_deleted_or_out_of_region_destination_is_stated_bare_too() {
    let router = emulator();

    let deleted = "bare.not.found.deleted";
    seed_destination(&router, "deleted", REGION, deleted).await;
    let removed = call(
        &router,
        "DELETE",
        &format!("/destinations/{}", encode_label(deleted)),
        None,
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "case \"deleted\": expected HTTP 200 from DeleteDestination, got {} with body {:?}",
        removed.status, removed.body
    );

    // A destination that exists, addressed through another region's endpoint: a destination
    // is regional, so that region holds none under this name.
    let elsewhere = "bare.not.found.elsewhere";
    seed_destination(&router, "other region", "eu-central-1", elsewhere).await;

    for (why, name) in [("deleted", deleted), ("other region", elsewhere)] {
        for resolving in destination_operations(name) {
            let case = format!("{}/{why}", resolving.operation);
            let reply = call(&router, resolving.method, &resolving.path, resolving.body).await;
            assert_bare_not_found(&case, &reply);
        }
    }
}

/// RULE: every operation of the notification-configuration surface states an `EventType` the
/// caller's region holds no configuration for with the bare exception, over the model's
/// whole `EventType` value set and whichever region the configuration lives in.
#[tokio::test]
async fn every_notification_configuration_operation_states_an_unconfigured_event_type_bare() {
    let router = emulator();

    // A destination the update can legitimately point at, so a refusal below is about the
    // configuration being absent and nothing else.
    let destination = "bare.not.found.notifications";
    seed_destination(&router, "seed", REGION, destination).await;

    // The rows are the whole `EventType` value set, model-generated rather than written out
    // here (`crate::ops::types::EVENT_TYPE_VALUES`), so a value added to the model is
    // covered as soon as it is regenerated.
    for event_type in EVENT_TYPE_VALUES {
        for resolving in notification_configuration_operations(event_type, destination) {
            let case = format!("{}/{event_type}", resolving.operation);
            let reply = call(&router, resolving.method, &resolving.path, resolving.body).await;
            assert_bare_not_found(&case, &reply);
        }
    }

    // A configuration that DOES exist, in another region: a notification configuration is
    // regional, so this region still holds none for that event type and states it the same
    // way, while the owning region reports it.
    let configured = EVENT_TYPE_VALUES[0];
    let other = "eu-central-1";
    seed_destination(&router, "other region", other, destination).await;
    let created = call_in(
        &router,
        other,
        "POST",
        "/notification-configurations",
        Some(json!({ "EventType": configured, "DestinationName": destination })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case \"other region\": expected HTTP 201 from CreateNotificationConfiguration, got {} \
         with body {:?}",
        created.status, created.body
    );
    for resolving in notification_configuration_operations(configured, destination) {
        let case = format!("{}/other region", resolving.operation);
        let reply = call(&router, resolving.method, &resolving.path, resolving.body).await;
        assert_bare_not_found(&case, &reply);
    }
    let owned = call_in(
        &router,
        other,
        "GET",
        &format!("/notification-configurations/{configured}"),
        None,
    )
    .await;
    assert_eq!(
        owned.status, 200,
        "case \"other region/owner\": expected the owning region to resolve it, got {} with \
         body {:?}",
        owned.status, owned.body
    );
}

/// RULE, the other half: filling the members is the RESOURCE's property, so the managed
/// thing — whose own path reports all three — keeps reporting them. A change that emptied
/// every 404 of this service rather than these two resources' fails here.
#[tokio::test]
async fn a_managed_things_404_still_carries_the_members_its_own_path_reports() {
    let router = emulator();

    for identifier in ["bare404thing", "0123456789", "MiXeDcAsEiD"] {
        for (operation, method, body) in [
            ("GetManagedThing", "GET", None),
            (
                "UpdateManagedThing",
                "PUT",
                Some(json!({ "Name": "lamp one" })),
            ),
            ("DeleteManagedThing", "DELETE", None),
        ] {
            let case = format!("{operation}/{identifier}");
            let reply = call(
                &router,
                method,
                &format!("/managed-things/{identifier}"),
                body,
            )
            .await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected HTTP 404, got {} with body {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some(RESOURCE_NOT_FOUND),
                "case {case:?}: expected the modelled {RESOURCE_NOT_FOUND} code, got {:?}",
                reply.error_type
            );
            assert_eq!(
                declared_members_present(&reply).len(),
                NOT_FOUND_MEMBERS.len(),
                "case {case:?}: expected all of {NOT_FOUND_MEMBERS:?}, got {:?} in body {:?}",
                declared_members_present(&reply),
                reply.body
            );
            assert_eq!(
                reply.string("ResourceId"),
                Some(identifier),
                "case {case:?}: expected ResourceId to report the identifier asked for, got {:?}",
                reply.string("ResourceId")
            );
            assert_eq!(
                reply.string("ResourceType"),
                Some(MANAGED_THING_RESOURCE_TYPE),
                "case {case:?}: expected ResourceType {MANAGED_THING_RESOURCE_TYPE:?}, got {:?}",
                reply.string("ResourceType")
            );
        }
    }
}

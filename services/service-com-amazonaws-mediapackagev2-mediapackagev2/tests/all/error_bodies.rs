// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the BODY of a MediaPackageV2 refusal, driven over HTTP through
//! the same router the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. A refusal carries the members the model declares for its error shape:
//!    `ResourceNotFoundException` sets `Message` and `ResourceTypeNotFound`,
//!    `ConflictException` sets `Message` and `ConflictExceptionType`, and
//!    `ValidationException` sets `Message` and `ValidationExceptionType` — including on
//!    the constraint-violation path, which never reaches a handler.
//! 2. The sentence depends only on WHAT was refused — the kind of resource, the
//!    constraint class — never on the identifiers the caller sent. So no refusal
//!    message contains a name from the request, and two requests refused for the same
//!    reason at different names read identically.
//! 3. A delete refused because children remain names the CHILD KIND that has to go
//!    first, and not how many children there are.
//! 4. A constraint violation names the member path that failed and restates the
//!    constraint it failed, and its `ValidationExceptionType` is the discriminator for
//!    the violated constraint class (`MEMBER_*`).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. It travels in the SigV4
/// credential scope, which is also what the service reads its region from.
const REGION: &str = "us-west-2";

/// The fixed sentence MediaPackage answers a miss with, per resource kind. Quoted from
/// the service's own wording, which names the KIND and no identifier.
const CHANNEL_GROUP_NOT_FOUND: &str = "MediaPackage can't process your request because \
     we can't find your channel group. Verify your channel group name or add a channel \
     group and then try again";
const CHANNEL_NOT_FOUND: &str = "MediaPackage can't process your request because we \
     can't find your channel. Verify your channel name or add a channel and then try \
     again";
const ORIGIN_ENDPOINT_NOT_FOUND: &str = "MediaPackage can't process your request \
     because we can't find your origin endpoint. Verify your origin endpoint name or \
     add an origin endpoint and then try again";

/// The fixed, resource-agnostic sentence a create answers when the name is taken.
const ALREADY_EXISTS: &str = "MediaPackage can't create the resource you requested \
     because a resource with that name already exists. Verify the name of the resource \
     and then try again";

/// A MediaPackageV2 emulator plus a tiny restJson1 client for it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // This suite signs nothing: it exercises refusal bodies, not authentication.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediapackagev2_mediapackagev2::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null` for an
    /// empty body).
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("mediapackagev2.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/mediapackagev2/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{method} {path} answered {status} with a body that is not JSON: {e}: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Create a channel group -> channel -> origin endpoint chain, asserting each step
    /// succeeded: the refusals under test are only meaningful against a store that the
    /// valid requests reached.
    async fn seed(&self, group: &str, channel: &str, endpoint: &str) {
        let created = self
            .call("POST", "/channelGroup", Some(json!({"ChannelGroupName": group})))
            .await;
        assert_eq!(created.0, 200, "seeding group {group}: {:?}", created.1);
        let created = self
            .call(
                "POST",
                &format!("/channelGroup/{group}/channel"),
                Some(json!({"ChannelName": channel})),
            )
            .await;
        assert_eq!(created.0, 200, "seeding channel {channel}: {:?}", created.1);
        let created = self
            .call(
                "POST",
                &format!("/channelGroup/{group}/channel/{channel}/originEndpoint"),
                Some(json!({
                    "OriginEndpointName": endpoint,
                    "ContainerType": "CMAF",
                    // An origin endpoint has to carry at least one manifest, so every
                    // seeded endpoint publishes one.
                    "HlsManifests": [{"ManifestName": "index"}],
                })),
            )
            .await;
        assert_eq!(created.0, 200, "seeding endpoint {endpoint}: {:?}", created.1);
    }
}

/// The `__type` discriminator of an error envelope.
fn error_type(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// A modeled error's `Message` member — the capitalized member name the model declares,
/// not the framework's generic `message`.
fn message(body: &Value) -> &str {
    body.get("Message").and_then(Value::as_str).unwrap_or("")
}

/// A string member of an error body (`ResourceTypeNotFound`, `ConflictExceptionType`,
/// `ValidationExceptionType`).
fn member<'b>(body: &'b Value, name: &str) -> &'b str {
    body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// One not-found case: the request that misses, and the kind of resource it misses on.
struct MissCase {
    /// What makes this row different from the others, for an assertion message.
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The sentence this kind of miss states.
    expected_message: &'static str,
    /// The `ResourceTypeNotFound` discriminator for that kind.
    expected_type: &'static str,
    /// Every identifier the request carried: none of them may come back in the message.
    identifiers: Vec<String>,
}

/// Rule 1 + 2 for the 404s: whichever operation misses, and whichever name it was
/// given, the body states the KIND of resource that is absent, carries that kind's
/// discriminator, and echoes none of the request's identifiers.
#[tokio::test]
async fn a_miss_names_the_resource_kind_and_echoes_no_identifier() {
    let emulator = Emulator::new();
    // Two seeded hierarchies, so a miss can be aimed at a name that does not exist
    // while its parents do — and at parents that do not exist at all. The names differ
    // in shape (length, case, separators, digits) because the message must not depend
    // on them.
    emulator.seed("probe-group-1", "probe-channel-1", "probe-endpoint-1").await;
    emulator.seed("G", "C", "E").await;

    let absent_group = "A_Very-Long_Absent-GROUP-0123456789";
    let absent_channel = "absent_CHANNEL-42";
    let absent_endpoint = "absent-endpoint_42";
    let cases = vec![
        MissCase {
            case: "GetChannelGroup, group absent",
            method: "GET",
            path: format!("/channelGroup/{absent_group}"),
            body: None,
            expected_message: CHANNEL_GROUP_NOT_FOUND,
            expected_type: "CHANNEL_GROUP",
            identifiers: vec![absent_group.to_string()],
        },
        MissCase {
            case: "GetChannelGroup, a one-character absent name",
            method: "GET",
            path: "/channelGroup/Z".to_string(),
            body: None,
            expected_message: CHANNEL_GROUP_NOT_FOUND,
            expected_type: "CHANNEL_GROUP",
            identifiers: vec!["Z".to_string()],
        },
        MissCase {
            case: "CreateChannel, parent group absent",
            method: "POST",
            path: format!("/channelGroup/{absent_group}/channel"),
            body: Some(json!({"ChannelName": "probe-channel-2"})),
            expected_message: CHANNEL_GROUP_NOT_FOUND,
            expected_type: "CHANNEL_GROUP",
            identifiers: vec![absent_group.to_string(), "probe-channel-2".to_string()],
        },
        MissCase {
            case: "GetChannel, channel absent from a group that exists",
            method: "GET",
            path: format!("/channelGroup/probe-group-1/channel/{absent_channel}/"),
            body: None,
            expected_message: CHANNEL_NOT_FOUND,
            expected_type: "CHANNEL",
            identifiers: vec!["probe-group-1".to_string(), absent_channel.to_string()],
        },
        MissCase {
            case: "GetChannel, group absent",
            method: "GET",
            path: format!("/channelGroup/{absent_group}/channel/{absent_channel}/"),
            body: None,
            expected_message: CHANNEL_GROUP_NOT_FOUND,
            expected_type: "CHANNEL_GROUP",
            identifiers: vec![absent_group.to_string(), absent_channel.to_string()],
        },
        MissCase {
            case: "CreateOriginEndpoint, parent channel absent",
            method: "POST",
            path: format!("/channelGroup/G/channel/{absent_channel}/originEndpoint"),
            body: Some(json!({
                "OriginEndpointName": "probe-endpoint-2",
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
            })),
            expected_message: CHANNEL_NOT_FOUND,
            expected_type: "CHANNEL",
            identifiers: vec![absent_channel.to_string(), "probe-endpoint-2".to_string()],
        },
        MissCase {
            case: "GetOriginEndpoint, endpoint absent from a channel that exists",
            method: "GET",
            path: format!("/channelGroup/G/channel/C/originEndpoint/{absent_endpoint}"),
            body: None,
            expected_message: ORIGIN_ENDPOINT_NOT_FOUND,
            expected_type: "ORIGIN_ENDPOINT",
            identifiers: vec![absent_endpoint.to_string()],
        },
        MissCase {
            case: "ListOriginEndpoints, parent channel absent",
            method: "GET",
            path: format!("/channelGroup/probe-group-1/channel/{absent_channel}/originEndpoint"),
            body: None,
            expected_message: CHANNEL_NOT_FOUND,
            expected_type: "CHANNEL",
            identifiers: vec!["probe-group-1".to_string(), absent_channel.to_string()],
        },
        MissCase {
            case: "GetChannelPolicy, parent group absent",
            method: "GET",
            path: format!("/channelGroup/{absent_group}/channel/{absent_channel}/policy"),
            body: None,
            expected_message: CHANNEL_GROUP_NOT_FOUND,
            expected_type: "CHANNEL_GROUP",
            identifiers: vec![absent_group.to_string(), absent_channel.to_string()],
        },
        MissCase {
            case: "PutChannelPolicy, parent channel absent",
            method: "PUT",
            path: format!("/channelGroup/G/channel/{absent_channel}/policy"),
            body: Some(json!({
                "Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            })),
            expected_message: CHANNEL_NOT_FOUND,
            expected_type: "CHANNEL",
            identifiers: vec![absent_channel.to_string()],
        },
        MissCase {
            case: "ResetChannelState, channel absent from a group that exists",
            method: "POST",
            path: format!("/channelGroup/G/channel/{absent_channel}/reset"),
            body: None,
            expected_message: CHANNEL_NOT_FOUND,
            expected_type: "CHANNEL",
            identifiers: vec![absent_channel.to_string()],
        },
    ];

    for case in cases {
        let (status, body) = emulator
            .call(case.method, &case.path, case.body.clone())
            .await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404, got {status} with {body:?}",
            case.case
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {:?}: expected a ResourceNotFoundException envelope, got {body:?}",
            case.case
        );
        assert_eq!(
            message(&body),
            case.expected_message,
            "case {:?}: wrong Message, got {:?}",
            case.case,
            message(&body)
        );
        assert_eq!(
            member(&body, "ResourceTypeNotFound"),
            case.expected_type,
            "case {:?}: wrong ResourceTypeNotFound, got {:?}",
            case.case,
            member(&body, "ResourceTypeNotFound")
        );
        for identifier in &case.identifiers {
            assert!(
                !message(&body).contains(identifier.as_str()),
                "case {:?}: the message must not echo the identifier {identifier:?}, got {:?}",
                case.case,
                message(&body)
            );
        }
    }
}

/// Rule 1 + 2 for the 409s on a create: every level of the hierarchy answers the ONE
/// resource-agnostic already-exists sentence, with `RESOURCE_ALREADY_EXISTS`.
#[tokio::test]
async fn a_duplicate_create_states_one_fixed_already_exists() {
    let emulator = Emulator::new();
    emulator.seed("dup-group", "dup-channel", "dup-endpoint").await;
    // A second hierarchy with differently shaped names: the collision reads the same.
    emulator.seed("DUP_GROUP_2", "DUP_CHANNEL_2", "DUP_ENDPOINT_2").await;

    let cases = [
        (
            "CreateChannelGroup, name taken",
            "POST",
            "/channelGroup".to_string(),
            json!({"ChannelGroupName": "dup-group"}),
            vec!["dup-group"],
        ),
        (
            "CreateChannelGroup, differently shaped name taken",
            "POST",
            "/channelGroup".to_string(),
            json!({"ChannelGroupName": "DUP_GROUP_2"}),
            vec!["DUP_GROUP_2"],
        ),
        (
            "CreateChannel, name taken in that group",
            "POST",
            "/channelGroup/dup-group/channel".to_string(),
            json!({"ChannelName": "dup-channel"}),
            vec!["dup-group", "dup-channel"],
        ),
        (
            "CreateOriginEndpoint, name taken on that channel",
            "POST",
            "/channelGroup/dup-group/channel/dup-channel/originEndpoint".to_string(),
            json!({
                "OriginEndpointName": "dup-endpoint",
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": "index"}],
            }),
            vec!["dup-group", "dup-channel", "dup-endpoint"],
        ),
    ];

    for (case, method, path, body, identifiers) in cases {
        let (status, answered) = emulator.call(method, &path, Some(body)).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} with {answered:?}"
        );
        assert_eq!(
            error_type(&answered),
            "ConflictException",
            "case {case:?}: expected a ConflictException envelope, got {answered:?}"
        );
        assert_eq!(
            message(&answered),
            ALREADY_EXISTS,
            "case {case:?}: wrong Message, got {:?}",
            message(&answered)
        );
        assert_eq!(
            member(&answered, "ConflictExceptionType"),
            "RESOURCE_ALREADY_EXISTS",
            "case {case:?}: wrong ConflictExceptionType, got {:?}",
            member(&answered, "ConflictExceptionType")
        );
        for identifier in identifiers {
            assert!(
                !message(&answered).contains(identifier),
                "case {case:?}: the message must not echo {identifier:?}, got {:?}",
                message(&answered)
            );
        }
    }
}

/// Rule 3: a delete refused while children remain names the child KIND, carries
/// `RESOURCE_IN_USE`, and reports neither the resource's name nor how many children it
/// has — so the sentence is the same whether one child is left or several.
#[tokio::test]
async fn a_delete_with_children_names_the_child_kind_not_the_count() {
    let emulator = Emulator::new();
    emulator.seed("busy-group", "busy-channel", "busy-endpoint-1").await;
    // A second channel under the same group, and a second endpoint under the same
    // channel, so each refusal below is exercised with more than one child too.
    for (path, body) in [
        (
            "/channelGroup/busy-group/channel".to_string(),
            json!({"ChannelName": "busy-channel-2"}),
        ),
        (
            "/channelGroup/busy-group/channel/busy-channel/originEndpoint".to_string(),
            json!({
                "OriginEndpointName": "busy-endpoint-2",
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
            }),
        ),
    ] {
        let created = emulator.call("POST", &path, Some(body)).await;
        assert_eq!(created.0, 200, "seeding {path}: {:?}", created.1);
    }

    let cases = [
        (
            "DeleteChannelGroup with two channels attached",
            "/channelGroup/busy-group".to_string(),
            "channels",
            vec!["busy-group"],
        ),
        (
            "DeleteChannel with two origin endpoints attached",
            "/channelGroup/busy-group/channel/busy-channel/".to_string(),
            "origin endpoints",
            vec!["busy-group", "busy-channel"],
        ),
    ];

    for (case, path, children, identifiers) in cases {
        let (status, body) = emulator.call("DELETE", &path, None).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} with {body:?}"
        );
        assert_eq!(
            error_type(&body),
            "ConflictException",
            "case {case:?}: expected a ConflictException envelope, got {body:?}"
        );
        assert_eq!(
            member(&body, "ConflictExceptionType"),
            "RESOURCE_IN_USE",
            "case {case:?}: wrong ConflictExceptionType, got {:?}",
            member(&body, "ConflictExceptionType")
        );
        assert!(
            message(&body).contains(children),
            "case {case:?}: expected the message to name the {children} that must go first, got {:?}",
            message(&body)
        );
        assert!(
            !message(&body).chars().any(char::is_numeric),
            "case {case:?}: the message must not report a child count, got {:?}",
            message(&body)
        );
        for identifier in identifiers {
            assert!(
                !message(&body).contains(identifier),
                "case {case:?}: the message must not echo {identifier:?}, got {:?}",
                message(&body)
            );
        }
    }
}

/// One constraint-violation case: a request that breaks a rule the model declares, the
/// member path that breaks it, and the `MEMBER_*` discriminator for its class.
struct ViolationCase {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The dotted member path the message must name.
    member_path: &'static str,
    /// The `ValidationExceptionType` for the violated constraint class.
    expected_type: &'static str,
    /// Text the message must NOT contain: the offending value the caller sent, or the
    /// framework's own `Value '...' at '...'` form, which echoes it.
    must_not_appear: String,
}

/// Rule 1 + 4: a value outside a constraint the model declares is refused with the
/// MODELED `ValidationException` — both members set — naming the member path that
/// failed, with the discriminator for the violated class. One row per constraint class
/// the model uses, over body members, URI labels, query parameters and a nested shape.
#[tokio::test]
async fn a_constraint_violation_carries_both_modeled_members() {
    let emulator = Emulator::new();
    // `ResourceName` is `@length(1, 256)` with `@pattern("^[a-zA-Z0-9_-]+$")`;
    // `ListResourceMaxResults` is `@range(1, 100)`; `ContainerType` is an enum of
    // `TS`, `CMAF`, `ISM`; `Segment$SegmentDurationSeconds` is `@range(1, 30)`.
    let too_long = "x".repeat(257);
    let cases = vec![
        ViolationCase {
            case: "CreateChannelGroup, unsupported character in a body member",
            method: "POST",
            path: "/channelGroup".to_string(),
            body: Some(json!({"ChannelGroupName": "bad name!"})),
            member_path: "channelGroupName",
            expected_type: "MEMBER_DOES_NOT_MATCH_PATTERN",
            must_not_appear: "bad name!".to_string(),
        },
        ViolationCase {
            case: "GetChannelGroup, unsupported character in a URI label",
            method: "GET",
            path: "/channelGroup/bad.name".to_string(),
            body: None,
            member_path: "channelGroupName",
            expected_type: "MEMBER_DOES_NOT_MATCH_PATTERN",
            must_not_appear: "bad.name".to_string(),
        },
        ViolationCase {
            case: "CreateChannelGroup, longer than the declared maximum",
            method: "POST",
            path: "/channelGroup".to_string(),
            body: Some(json!({"ChannelGroupName": too_long.clone()})),
            member_path: "channelGroupName",
            expected_type: "MEMBER_MAX_LENGTH",
            must_not_appear: too_long.clone(),
        },
        ViolationCase {
            case: "CreateChannelGroup, shorter than the declared minimum",
            method: "POST",
            path: "/channelGroup".to_string(),
            body: Some(json!({"ChannelGroupName": ""})),
            member_path: "channelGroupName",
            expected_type: "MEMBER_MIN_LENGTH",
            // The value is empty, so what must not appear is the framework's echoing
            // form for it.
            must_not_appear: "Value ''".to_string(),
        },
        ViolationCase {
            case: "CreateChannel, required member absent",
            method: "POST",
            path: "/channelGroup/any-group/channel".to_string(),
            body: Some(json!({"Description": "no name given"})),
            member_path: "channelName",
            expected_type: "MEMBER_MISSING",
            must_not_appear: "no name given".to_string(),
        },
        ViolationCase {
            case: "ListOriginEndpoints, query parameter below the declared range",
            method: "GET",
            path: "/channelGroup/any-group/channel/any-channel/originEndpoint?maxResults=0"
                .to_string(),
            body: None,
            member_path: "maxResults",
            expected_type: "MEMBER_MIN_VALUE",
            must_not_appear: "any-channel".to_string(),
        },
        ViolationCase {
            case: "ListOriginEndpoints, query parameter above the declared range",
            method: "GET",
            path: "/channelGroup/any-group/channel/any-channel/originEndpoint?maxResults=101"
                .to_string(),
            body: None,
            member_path: "maxResults",
            expected_type: "MEMBER_MAX_VALUE",
            must_not_appear: "any-channel".to_string(),
        },
        ViolationCase {
            case: "CreateOriginEndpoint, value outside an enum",
            method: "POST",
            path: "/channelGroup/any-group/channel/any-channel/originEndpoint".to_string(),
            body: Some(json!({"OriginEndpointName": "any-endpoint", "ContainerType": "DVD"})),
            member_path: "containerType",
            expected_type: "MEMBER_INVALID_ENUM_VALUE",
            must_not_appear: "DVD".to_string(),
        },
    ];

    for case in cases {
        let (status, body) = emulator
            .call(case.method, &case.path, case.body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} with {body:?}",
            case.case
        );
        assert_eq!(
            error_type(&body),
            "ValidationException",
            "case {:?}: expected a ValidationException envelope, got {body:?}",
            case.case
        );
        assert_eq!(
            member(&body, "ValidationExceptionType"),
            case.expected_type,
            "case {:?}: wrong ValidationExceptionType, got {body:?}",
            case.case
        );
        let expected_prefix =
            format!("Validation error encountered at request.{}: ", case.member_path);
        assert!(
            message(&body).starts_with(&expected_prefix),
            "case {:?}: expected a message starting {expected_prefix:?}, got {:?}",
            case.case,
            message(&body)
        );
        assert!(
            message(&body).len() > expected_prefix.len(),
            "case {:?}: the message must restate the constraint, got {:?}",
            case.case,
            message(&body)
        );
        assert!(
            !message(&body).contains(case.must_not_appear.as_str()),
            "case {:?}: the message must not echo the request's value {:?}, got {:?}",
            case.case,
            case.must_not_appear,
            message(&body)
        );
    }
}

/// The pattern refusal is where AWS states the characters a name may hold, so that
/// sentence is pinned: it lists the supported characters of the violated pattern and
/// nothing about the request.
#[tokio::test]
async fn an_unsupported_character_is_refused_with_the_supported_set() {
    let emulator = Emulator::new();
    // Every row is an unsupported character in a name the model constrains with
    // `ResourceName`'s `^[a-zA-Z0-9_-]+$`, so every row reads the same.
    for name in ["bad name", "bad!", "bad.name", "bad/name", "bad+name", "ünïcode"] {
        let (status, body) = emulator
            .call(
                "POST",
                "/channelGroup",
                Some(json!({"ChannelGroupName": name})),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            message(&body),
            "Validation error encountered at request.channelGroupName: Member must \
             contain only supported characters. Supported characters are A-Z, a-z, 0-9, \
             _ (underscore), and - (hyphen). Check the value you specified in your \
             request and then try again",
            "case {name:?}: wrong Message, got {:?}",
            message(&body)
        );
        assert_eq!(
            member(&body, "ValidationExceptionType"),
            "MEMBER_DOES_NOT_MATCH_PATTERN",
            "case {name:?}: wrong ValidationExceptionType, got {body:?}"
        );
    }
}

/// The gate that answers those refusals must not stand in the way of a request the
/// model accepts: the whole hierarchy still creates and reads back.
#[tokio::test]
async fn a_request_the_model_accepts_is_served_by_its_operation() {
    let emulator = Emulator::new();
    emulator.seed("ok-group", "ok-channel", "ok-endpoint").await;
    let (status, body) = emulator
        .call(
            "GET",
            "/channelGroup/ok-group/channel/ok-channel/originEndpoint/ok-endpoint",
            None,
        )
        .await;
    assert_eq!(status, 200, "reading the seeded endpoint back: {body:?}");
    assert_eq!(
        body.get("OriginEndpointName").and_then(Value::as_str),
        Some("ok-endpoint"),
        "reading the seeded endpoint back: got {body:?}"
    );
}

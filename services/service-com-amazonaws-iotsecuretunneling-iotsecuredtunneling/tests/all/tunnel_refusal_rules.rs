// SPDX-License-Identifier: CC0-1.0

//! The refusal rules of the tunnel API, each asserted over a class of inputs
//! rather than over one request:
//!
//! 1. `RotateTunnelAccessToken` reissues the credentials of a LIVE tunnel, so it
//!    is refused — `ValidationException`, HTTP 400 — whenever the resolved
//!    tunnel is not `OPEN`, however it stopped being open, and mints nothing.
//! 2. A `destinationConfig` configures the destination end of the tunnel, so
//!    `RotateTunnelAccessToken` refuses it whenever the requested `clientMode`
//!    does not rotate that end.
//! 3. `ResourceNotFoundException` names the TUNNEL by its id in one sentence
//!    form, identical across the read, the mutating and the ARN-addressed tag
//!    paths.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for, so an ARN's region segment is observable.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// `ClientMode`'s modelled values, from
/// `smithy_query(query="shape", shape="com.amazonaws.iotsecuretunneling#ClientMode")`:
/// `["SOURCE", "DESTINATION", "ALL"]`.
const CLIENT_MODES: [&str; 3] = ["SOURCE", "DESTINATION", "ALL"];

/// A router carrying only this service, over the engine the test also inspects.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/IoTSecuredTunneling/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code (`__type` on awsJson) of a refusal body.
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or_default()
}

/// The `message` member of a refusal body.
fn message(body: &Value) -> &str {
    body["message"].as_str().unwrap_or_default()
}

/// Open one tunnel and return `(tunnelId, tunnelArn)`. `minutes` is the
/// `maxLifetimeTimeoutMinutes` to configure, or `None` for the modelled default.
async fn open_tunnel(
    router: &MultiServiceRouter,
    case: &str,
    minutes: Option<i64>,
) -> (String, String) {
    let mut input = json!({
        "description": case,
        "destinationConfig": {"thingName": "thing-refusal", "services": ["SSH"]},
    });
    if let Some(minutes) = minutes {
        input["timeoutConfig"] = json!({"maxLifetimeTimeoutMinutes": minutes});
    }
    let (status, opened) = call(router, "OpenTunnel", &input).await;
    assert_eq!(status, 200, "case {case:?}: OpenTunnel answered {opened:?}");
    (
        opened["tunnelId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        opened["tunnelArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// The stored access-token pair of `tunnel_id`.
fn stored_tokens(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str) -> (String, String) {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnel(REGION, tunnel_id)
        .map(|tunnel| {
            (
                tunnel.source_access_token.clone(),
                tunnel.destination_access_token.clone(),
            )
        })
        .unwrap_or_else(|| panic!("tunnel {tunnel_id} is not in the store"))
}

/// How a tunnel can stop being `OPEN`: the explicit close in either of its
/// record-preserving forms, and the close the clock owes once
/// `maxLifetimeTimeoutMinutes` has run out.
#[derive(Debug, Clone, Copy)]
enum HowItClosed {
    /// `CloseTunnel` with the `delete` member absent.
    CloseDeleteOmitted,
    /// `CloseTunnel` with `delete=false`.
    CloseDeleteFalse,
    /// The expiry sweep, after the tunnel's lifetime of `minutes` ran out.
    LifetimeElapsed { minutes: i64 },
}

/// Bring `tunnel_id` to `CLOSED` the way `how` says, leaving the record readable.
async fn close_tunnel(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    tunnel_id: &str,
    how: HowItClosed,
    case: &str,
) {
    match how {
        HowItClosed::CloseDeleteOmitted | HowItClosed::CloseDeleteFalse => {
            let mut input = json!({"tunnelId": tunnel_id});
            if matches!(how, HowItClosed::CloseDeleteFalse) {
                input["delete"] = Value::Bool(false);
            }
            let (status, closed) = call(router, "CloseTunnel", &input).await;
            assert_eq!(status, 200, "case {case:?}: CloseTunnel answered {closed:?}");
        }
        HowItClosed::LifetimeElapsed { minutes } => {
            // Move the tunnel's whole timeline into the past, so its lifetime
            // ran out a second ago exactly as the clock would have done.
            let shift = Duration::from_secs(u64::try_from(minutes).unwrap_or(1) * 60 + 1);
            let state = engine.state::<TunnelState>();
            let mut store = state.lock();
            let tunnel = store
                .tunnel_mut(REGION, tunnel_id)
                .unwrap_or_else(|| panic!("case {case:?}: the opened tunnel was not persisted"));
            tunnel.created_at -= shift;
            tunnel.last_updated_at -= shift;
            tunnel.expires_at -= shift;
            let expires_at = tunnel.expires_at;
            store.note_open_expiry(expires_at);
        }
    }

    let (status, described) = call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a closed tunnel stays describable, answered {described:?}"
    );
    assert_eq!(
        described["tunnel"]["status"].as_str(),
        Some("CLOSED"),
        "case {case:?}: status before rotate was {:?}",
        described["tunnel"]["status"]
    );
}

#[tokio::test]
async fn rotate_is_refused_once_the_tunnel_is_no_longer_open() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // Every way a tunnel stops being OPEN while staying describable, crossed
    // with every modelled clientMode: the refusal is about the tunnel's status,
    // not about which end was asked for or about how it closed.
    let closings = [
        HowItClosed::CloseDeleteOmitted,
        HowItClosed::CloseDeleteFalse,
        // Each published bound of `maxLifetimeTimeoutMinutes` (`range(min: 1,
        // max: 720)`) and an interior value.
        HowItClosed::LifetimeElapsed { minutes: 1 },
        HowItClosed::LifetimeElapsed { minutes: 60 },
        HowItClosed::LifetimeElapsed { minutes: 720 },
    ];

    for how in closings {
        for mode in CLIENT_MODES {
            let case = format!("{how:?} + clientMode {mode}");
            let lifetime = match how {
                HowItClosed::LifetimeElapsed { minutes } => Some(minutes),
                _ => None,
            };
            let (tunnel_id, _) = open_tunnel(&router, &case, lifetime).await;
            close_tunnel(&router, &engine, &tunnel_id, how, &case).await;
            let before = stored_tokens(&engine, &tunnel_id);

            let (status, refused) = call(
                &router,
                "RotateTunnelAccessToken",
                &json!({"tunnelId": tunnel_id, "clientMode": mode}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: rotating a closed tunnel must be refused, answered {status} {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?}: answered {refused:?}"
            );
            assert!(
                message(&refused).contains(&tunnel_id),
                "case {case:?}: the refusal must name the tunnel, got {:?}",
                message(&refused)
            );
            assert!(
                refused.get("sourceAccessToken").is_none()
                    && refused.get("destinationAccessToken").is_none(),
                "case {case:?}: a refusal must carry no access token, got {refused:?}"
            );
            assert_eq!(
                stored_tokens(&engine, &tunnel_id),
                before,
                "case {case:?}: a refused rotate must mint and store nothing"
            );
        }
    }
}

#[tokio::test]
async fn rotate_refuses_a_destination_config_its_client_mode_does_not_cover() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // `DestinationConfig` as the model shapes it: `services` is required
    // (`length(min: 1)`), `thingName` optional — so the rows differ in the
    // optional member's presence and in the number of services, which is what
    // the rule generalises over. The rule itself is about the MEMBER's presence
    // against the clientMode, so every form must be refused with SOURCE and
    // accepted with the two modes that rotate the destination end.
    let configs = [
        ("services only", json!({"services": ["SSH"]})),
        (
            "thingName and one service",
            json!({"thingName": "thing-pairing", "services": ["SSH"]}),
        ),
        (
            "several services",
            json!({"thingName": "thing-pairing", "services": ["SSH", "HTTP", "DB"]}),
        ),
    ];

    for (config_label, config) in configs {
        for mode in CLIENT_MODES {
            // A destinationConfig rides along only on a rotation that covers the
            // destination end.
            let allowed = mode != "SOURCE";
            let case = format!("{config_label} + clientMode {mode}");
            let (tunnel_id, _) = open_tunnel(&router, &case, None).await;
            let before = stored_tokens(&engine, &tunnel_id);

            let (status, body) = call(
                &router,
                "RotateTunnelAccessToken",
                &json!({
                    "tunnelId": tunnel_id,
                    "clientMode": mode,
                    "destinationConfig": config,
                }),
            )
            .await;

            if allowed {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the pairing is legal, answered {status} {body:?}"
                );
                let (_, described) =
                    call(&router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
                let expected: Vec<&str> = config["services"]
                    .as_array()
                    .map(|entries| entries.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let services: Vec<&str> = described["tunnel"]["destinationConfig"]["services"]
                    .as_array()
                    .map(|entries| entries.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                assert_eq!(
                    services, expected,
                    "case {case:?}: a legal pairing must apply the config, got {services:?}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: destinationConfig with a source-only rotation must be \
                     refused, answered {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: answered {body:?}"
                );
                assert!(
                    body.get("sourceAccessToken").is_none(),
                    "case {case:?}: a refusal must carry no access token, got {body:?}"
                );
                assert_eq!(
                    stored_tokens(&engine, &tunnel_id),
                    before,
                    "case {case:?}: a refused rotate must mint and store nothing"
                );
                let (_, described) =
                    call(&router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
                let services: Vec<&str> = described["tunnel"]["destinationConfig"]["services"]
                    .as_array()
                    .map(|entries| entries.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                assert_eq!(
                    services,
                    vec!["SSH"],
                    "case {case:?}: a refused rotate must not apply the config, got {services:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_source_only_rotation_without_a_destination_config_is_accepted() {
    // The companion of the rule above: it is the MEMBER that SOURCE refuses, so
    // the same mode on the same tunnel succeeds when the request sends none.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let (tunnel_id, _) = open_tunnel(&router, "source without a config", None).await;
    let before = stored_tokens(&engine, &tunnel_id);
    let (status, rotated) = call(
        &router,
        "RotateTunnelAccessToken",
        &json!({"tunnelId": tunnel_id, "clientMode": "SOURCE"}),
    )
    .await;
    assert_eq!(status, 200, "SOURCE alone answered {status} {rotated:?}");
    let after = stored_tokens(&engine, &tunnel_id);
    assert_ne!(
        after.0, before.0,
        "SOURCE alone must replace the stored source token"
    );
    assert_eq!(
        rotated["sourceAccessToken"].as_str(),
        Some(after.0.as_str()),
        "SOURCE alone returned {:?}",
        rotated.get("sourceAccessToken")
    );
    assert_eq!(
        after.1, before.1,
        "SOURCE alone must leave the destination token alone"
    );
}

#[tokio::test]
async fn every_not_found_refusal_names_the_tunnel_by_its_id() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // A real tunnel exists throughout, so each refusal below is about the id or
    // ARN named and not about an empty store.
    let (live_id, live_arn) = open_tunnel(&router, "the live tunnel", None).await;

    // One tunnel is opened and then deleted, so a deleted id is covered beside
    // never-created ones.
    let (deleted_id, _) = open_tunnel(&router, "the deleted tunnel", None).await;
    let (status, closed) = call(
        &router,
        "CloseTunnel",
        &json!({"tunnelId": deleted_id, "delete": true}),
    )
    .await;
    assert_eq!(status, 200, "CloseTunnel(delete) answered {closed:?}");

    // Ids that satisfy `TunnelId`'s modelled pattern `^[a-zA-Z0-9_\-+=:]{1,128}$`
    // but name no tunnel: the UUID form AWS mints, a name-like id, one using the
    // pattern's punctuation (`:` included, which an ARN's resource segment must
    // therefore not split on), the shortest the pattern admits and the longest.
    let long_id = "z".repeat(128);
    let unknown_ids: Vec<String> = vec![
        "0f1e2d3c-4b5a-4987-8765-432112345678".to_string(),
        "nonexistent-tunnel-661c815074cb".to_string(),
        "weird_id+with=every:char-class".to_string(),
        "a".to_string(),
        long_id,
        deleted_id.clone(),
    ];

    for tunnel_id in &unknown_ids {
        let arn = format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/{tunnel_id}");
        // Every operation that models ResourceNotFoundException, by both of the
        // ways a request addresses a tunnel: by id, and by resource ARN.
        let calls: Vec<(&str, Value)> = vec![
            ("DescribeTunnel", json!({"tunnelId": tunnel_id})),
            ("CloseTunnel", json!({"tunnelId": tunnel_id})),
            (
                "CloseTunnel",
                json!({"tunnelId": tunnel_id, "delete": true}),
            ),
            (
                "RotateTunnelAccessToken",
                json!({"tunnelId": tunnel_id, "clientMode": "SOURCE"}),
            ),
            (
                "RotateTunnelAccessToken",
                json!({"tunnelId": tunnel_id, "clientMode": "ALL"}),
            ),
            (
                "TagResource",
                json!({"resourceArn": arn, "tags": [{"key": "env", "value": "test"}]}),
            ),
            (
                "UntagResource",
                json!({"resourceArn": arn, "tagKeys": ["env"]}),
            ),
            ("ListTagsForResource", json!({"resourceArn": arn})),
        ];

        // The one message form every path must produce: the tunnel named by its
        // id, whichever member the request used to address it.
        let expected = format!("Tunnel {tunnel_id} does not exist.");
        for (operation, input) in calls {
            let case = format!("{operation} {input}");
            let (status, body) = call(&router, operation, &input).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, answered {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: answered {body:?}"
            );
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                message(&body)
            );
        }
    }

    // The live tunnel is untouched by all of that: the refusals resolved nothing
    // into existence and removed nothing.
    let (status, described) = call(&router, "DescribeTunnel", &json!({"tunnelId": live_id})).await;
    assert_eq!(
        status, 200,
        "the live tunnel must still be readable, answered {described:?}"
    );
    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({"resourceArn": live_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the live tunnel's ARN must still resolve, answered {listed:?}"
    );
}

#[tokio::test]
async fn a_not_found_refusal_on_an_arn_that_names_no_tunnel_still_refuses() {
    // An ARN the tag paths cannot resolve to a tunnel is still refused with the
    // modelled code: the message names the only identifier the request offered.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);
    let _ = open_tunnel(&router, "the live tunnel", None).await;

    let arns = [
        // A different resource type in the same namespace.
        format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/thing-1"),
        // The tunnel's namespace but another account's.
        "arn:aws:iot:us-west-2:111111111111:tunnel/0f1e2d3c-4b5a".to_string(),
        // Not an ARN at all.
        "just-a-string".to_string(),
    ];

    for arn in arns {
        for (operation, input) in [
            (
                "TagResource",
                json!({"resourceArn": arn, "tags": [{"key": "env", "value": "test"}]}),
            ),
            ("ListTagsForResource", json!({"resourceArn": arn})),
            (
                "UntagResource",
                json!({"resourceArn": arn, "tagKeys": ["env"]}),
            ),
        ] {
            let case = format!("{operation} {arn}");
            let (status, body) = call(&router, operation, &input).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, answered {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: answered {body:?}"
            );
            assert!(
                message(&body).starts_with("Tunnel ") && message(&body).ends_with("does not exist."),
                "case {case:?}: every not-found message takes the tunnel-scoped form, got {:?}",
                message(&body)
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The bound on a destination configuration's service names, asserted over a
//! class of lists rather than over the one request that exposed it.
//!
//! RULE: a tunnel is multiplexed over at most
//! `state::MAX_SERVICES_PER_TUNNEL` simultaneous data streams, so a
//! `destinationConfig` whose `services` list is longer is refused with a 400
//! `ValidationException` at EVERY writer of the field — `OpenTunnel` and
//! `RotateTunnelAccessToken` — and the refused request mints no id, ARN or
//! access token and leaves the store exactly as it was. A list at or under the
//! bound is accepted and round-trips through `DescribeTunnel`.
//!
//! The bound itself is read from the crate rather than re-spelled here, so the
//! rows and the handlers cannot drift apart.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::MAX_SERVICES_PER_TUNNEL;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

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

/// The number of tunnel records the store holds.
fn stored_tunnel_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine.state::<TunnelState>().lock().tunnels.len()
}

/// The stored access-token pair of `tunnel_id`.
fn stored_tokens(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str) -> (String, String) {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnels
        .get(tunnel_id)
        .map(|tunnel| {
            (
                tunnel.source_access_token.clone(),
                tunnel.destination_access_token.clone(),
            )
        })
        .unwrap_or_else(|| panic!("tunnel {tunnel_id} is not in the store"))
}

/// The `destinationConfig.services` list `DescribeTunnel` reports for `tunnel_id`.
async fn described_services(router: &MultiServiceRouter, tunnel_id: &str) -> Vec<String> {
    let (status, described) = call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
    assert_eq!(
        status, 200,
        "DescribeTunnel({tunnel_id}) answered {status} {described:?}"
    );
    described["tunnel"]["destinationConfig"]["services"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A service name of `len` characters, well inside `Service`'s modelled
/// `length(min: 1, max: 128)` / `pattern ^[a-zA-Z0-9:_-]+$` (from
/// `smithy_query(query="shape", shape="com.amazonaws.iotsecuretunneling#Service")`).
fn service_name(len: usize) -> String {
    "s".repeat(len)
}

/// One row of the table: a `services` list, and whether the rule admits it.
fn service_lists() -> Vec<(String, Vec<String>)> {
    let mut rows: Vec<(String, Vec<String>)> = Vec::new();

    // Every count the rule admits, from the modelled minimum (`ServiceList`
    // carries `length(min: 1)`) up to and including the bound itself.
    for count in 1..=MAX_SERVICES_PER_TUNNEL {
        rows.push((
            format!("{count} distinct services (admitted)"),
            (0..count).map(|i| format!("svc-{i}")).collect(),
        ));
    }

    // At the bound, with names exercising the member's published extremes and
    // its punctuation rather than another spelling of the same shape.
    rows.push((
        "at the bound, extreme member forms".to_string(),
        vec![
            service_name(1),
            service_name(128),
            "SSH:1_x-y".to_string(),
        ],
    ));

    // One past the bound, and well past it.
    rows.push((
        "one past the bound".to_string(),
        (0..=MAX_SERVICES_PER_TUNNEL)
            .map(|i| format!("svc-{i}"))
            .collect(),
    ));
    rows.push((
        "ten services".to_string(),
        (0..10).map(|i| format!("svc-{i}")).collect(),
    ));
    // Past the bound by repeating ONE name: it is the number of streams the
    // request configures that the rule bounds, not the number of distinct ones.
    rows.push((
        "one name repeated past the bound".to_string(),
        vec!["SSH".to_string(); MAX_SERVICES_PER_TUNNEL + 1],
    ));

    rows
}

#[tokio::test]
async fn open_tunnel_bounds_the_destination_services_it_accepts() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for (label, services) in service_lists() {
        let admitted = services.len() <= MAX_SERVICES_PER_TUNNEL;
        // The optional `thingName` is sent on half the rows: the rule is about
        // `services` alone, so its presence must not change the verdict.
        let with_thing_name = services.len() % 2 == 0;
        let case = format!("OpenTunnel: {label} (thingName: {with_thing_name})");

        let mut config = json!({"services": services});
        if with_thing_name {
            config["thingName"] = json!("thing-limit");
        }
        let before = stored_tunnel_count(&engine);
        let (status, body) = call(
            &router,
            "OpenTunnel",
            &json!({"description": label, "destinationConfig": config}),
        )
        .await;

        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, answered {status} {body:?}"
            );
            let tunnel_id = body["tunnelId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no tunnelId in {body:?}"))
                .to_string();
            assert_eq!(
                stored_tunnel_count(&engine),
                before + 1,
                "case {case:?}: an accepted open must store exactly one tunnel"
            );
            assert_eq!(
                described_services(&router, &tunnel_id).await,
                services,
                "case {case:?}: the accepted list must round-trip, got {:?}",
                described_services(&router, &tunnel_id).await
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a 400 refusal, answered {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: answered {body:?}"
            );
            assert!(
                body.get("tunnelId").is_none()
                    && body.get("tunnelArn").is_none()
                    && body.get("sourceAccessToken").is_none()
                    && body.get("destinationAccessToken").is_none(),
                "case {case:?}: a refused open must mint no id, ARN or token, got {body:?}"
            );
            assert_eq!(
                stored_tunnel_count(&engine),
                before,
                "case {case:?}: a refused open must store no tunnel"
            );
        }
    }
}

#[tokio::test]
async fn rotate_bounds_the_destination_services_it_accepts() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // The clientModes that rotate the destination end, i.e. the ones a
    // `destinationConfig` may ride along on at all.
    let modes = ["DESTINATION", "ALL"];

    for (label, services) in service_lists() {
        for mode in modes {
            let admitted = services.len() <= MAX_SERVICES_PER_TUNNEL;
            let case = format!("RotateTunnelAccessToken {mode}: {label}");

            let (status, opened) = call(
                &router,
                "OpenTunnel",
                &json!({
                    "description": case,
                    "destinationConfig": {"thingName": "thing-limit", "services": ["SSH"]},
                }),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: OpenTunnel answered {opened:?}");
            let tunnel_id = opened["tunnelId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no tunnelId in {opened:?}"))
                .to_string();
            let tokens_before = stored_tokens(&engine, &tunnel_id);

            let (status, body) = call(
                &router,
                "RotateTunnelAccessToken",
                &json!({
                    "tunnelId": tunnel_id,
                    "clientMode": mode,
                    "destinationConfig": {"services": services},
                }),
            )
            .await;

            if admitted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, answered {status} {body:?}"
                );
                assert_eq!(
                    described_services(&router, &tunnel_id).await,
                    services,
                    "case {case:?}: an accepted rotate must apply the config"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a 400 refusal, answered {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: answered {body:?}"
                );
                assert!(
                    body.get("sourceAccessToken").is_none()
                        && body.get("destinationAccessToken").is_none(),
                    "case {case:?}: a refused rotate must mint no token, got {body:?}"
                );
                assert_eq!(
                    stored_tokens(&engine, &tunnel_id),
                    tokens_before,
                    "case {case:?}: a refused rotate must leave the stored tokens alone"
                );
                assert_eq!(
                    described_services(&router, &tunnel_id).await,
                    vec!["SSH".to_string()],
                    "case {case:?}: a refused rotate must not apply the config"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_refusal_names_the_member_and_the_count_the_request_carried() {
    // The refusal is built from the request, so a caller can fix it from the
    // text alone: it names the member and how many services the list carried.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for count in [MAX_SERVICES_PER_TUNNEL + 1, MAX_SERVICES_PER_TUNNEL + 4] {
        let services: Vec<String> = (0..count).map(|i| format!("svc-{i}")).collect();
        let case = format!("{count} services");
        let (status, body) = call(
            &router,
            "OpenTunnel",
            &json!({"destinationConfig": {"services": services}}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, answered {status} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("destinationConfig.services") && message.contains(&count.to_string()),
            "case {case:?}: the refusal must name the member and the count, got {message:?}"
        );
    }
}

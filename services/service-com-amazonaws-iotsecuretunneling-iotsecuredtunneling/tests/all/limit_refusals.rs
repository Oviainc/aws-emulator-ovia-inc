// SPDX-License-Identifier: CC0-1.0

//! How this service reports an exhausted limit, at every entry point that has one.
//!
//! RULE: when a request would take the service past a count it caps — the tunnels
//! this account holds OPEN in one Region (`state::MAX_CONCURRENT_TUNNELS`) or the
//! distinct tag keys one tunnel carries (`state::MAX_TAGS_PER_TUNNEL`) — it is refused with the error
//! this service models for an exhausted limit, `LimitExceededException` at this
//! protocol's client status, whose message names the limit and the count the
//! request would reach; the
//! refusal stores nothing and mints nothing. The error code is the modelled
//! shape's own name, so no refusal of a capped count carries a code the service's
//! model does not define.
//!
//! Rows differ along what the rule generalises over: which cap is exhausted,
//! which operation carries the request, how far past the cap it goes, and whether
//! the tunnel was already holding keys. The caps themselves are read from the
//! crate rather than re-spelled here, and `TagList`'s modelled bound
//! (`length(min: 1, max: 200)`, from
//! `smithy_query(query="shape", shape="com.amazonaws.iotsecuretunneling#TagList")`)
//! is what keeps every row's list within 200 entries, so the cap under test is
//! the per-tunnel key count and not the request's list length.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::{
    StoredTunnel, MAX_CONCURRENT_TUNNELS, MAX_TAGS_PER_TUNNEL,
};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// The wire code of this service's limit refusal — the modelled shape's own name,
/// `com.amazonaws.iotsecuretunneling#LimitExceededException` — and the status it
/// arrives with: the `awsJson1_1` client-error status, since the model's
/// `@httpError` is inert on this service's protocol.
const LIMIT_CODE: &str = "LimitExceededException";
const LIMIT_STATUS: u16 = 400;

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

/// Assert that `body`/`status` is this service's modelled limit refusal, and that
/// its message names the cap that was exhausted and the count the request reached.
fn assert_limit_refusal(case: &str, status: u16, body: &Value, limit: usize, reached: usize) {
    assert_eq!(
        status, LIMIT_STATUS,
        "case {case:?}: expected the modelled limit refusal, answered {status} {body:?}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some(LIMIT_CODE),
        "case {case:?}: expected the modelled limit error code, answered {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&limit.to_string()),
        "case {case:?}: the message must name the limit {limit}, got {message:?}"
    );
    assert!(
        message.contains(&reached.to_string()),
        "case {case:?}: the message must name the count {reached} the request would reach, got \
         {message:?}"
    );
}

/// A modelled `TagList` naming `count` distinct keys, each prefixed `prefix`.
fn tags_of(count: usize, prefix: &str) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"key": format!("{prefix}{index}"), "value": "v"}))
            .collect(),
    )
}

/// Open a tunnel carrying `tags` and return `(status, body)`.
async fn open_with_tags(router: &MultiServiceRouter, tags: Value) -> (u16, Value) {
    call(
        router,
        "OpenTunnel",
        &json!({
            "destinationConfig": {"thingName": "thing-limits", "services": ["SSH"]},
            "tags": tags,
        }),
    )
    .await
}

#[tokio::test]
async fn the_tag_limit_is_refused_with_the_modelled_limit_error_at_both_entry_points() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // Rows: how many distinct keys past the cap the request goes. The last row
    // fills the modelled `TagList` to its maximum, so the refusal is the per-tunnel
    // cap and not the list bound.
    let overshoots: [usize; 4] = [1, 2, 25, 200 - MAX_TAGS_PER_TUNNEL];

    for overshoot in overshoots {
        let count = MAX_TAGS_PER_TUNNEL + overshoot;

        // Entry point 1: the tags arrive with `OpenTunnel`.
        let case = format!("{count} tags via OpenTunnel");
        let before = engine.state::<TunnelState>().lock().tunnels.len();
        let (status, body) = open_with_tags(&router, tags_of(count, "k")).await;
        assert_limit_refusal(&case, status, &body, MAX_TAGS_PER_TUNNEL, count);
        for field in [
            "tunnelId",
            "tunnelArn",
            "sourceAccessToken",
            "destinationAccessToken",
        ] {
            assert!(
                body.get(field).is_none(),
                "case {case:?}: a refused open must hand out no {field}, got {:?}",
                body.get(field)
            );
        }
        let after = engine.state::<TunnelState>().lock().tunnels.len();
        assert_eq!(
            after, before,
            "case {case:?}: a refused open must store no tunnel, went from {before} to {after}"
        );

        // Entry point 2: the same overshoot through `TagResource`, on a tunnel
        // opened at the cap, so the only difference is which call carried the keys.
        let case = format!("{overshoot} new keys via TagResource on a tunnel at the cap");
        let (status, opened) = open_with_tags(&router, tags_of(MAX_TAGS_PER_TUNNEL, "seed")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a tunnel at the cap must open, answered {status} {opened:?}"
        );
        let arn = opened["tunnelArn"].as_str().unwrap_or_default().to_string();
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({"resourceArn": arn, "tags": tags_of(overshoot, "fresh")}),
        )
        .await;
        assert_limit_refusal(&case, status, &body, MAX_TAGS_PER_TUNNEL, count);
        let (status, listed) =
            call(&router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource answered {status} {listed:?}"
        );
        let stored = listed["tags"].as_array().map(Vec::len).unwrap_or_default();
        assert_eq!(
            stored, MAX_TAGS_PER_TUNNEL,
            "case {case:?}: a refused TagResource must store none of its tags, got {stored} \
             stored tags"
        );
    }
}

#[tokio::test]
async fn the_open_tunnel_quota_is_refused_with_the_same_modelled_limit_error() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // One real tunnel, then the account filled to its concurrency quota with OPEN
    // copies of it, so the refusal below is about the open count and nothing else.
    let (status, opened) = open_with_tags(&router, tags_of(1, "k")).await;
    assert_eq!(status, 200, "OpenTunnel answered {status} {opened:?}");
    let seed_id = opened["tunnelId"].as_str().unwrap_or_default().to_string();
    {
        let state = engine.state::<TunnelState>();
        let mut store = state.lock();
        let seed: StoredTunnel = store
            .tunnel(REGION, &seed_id)
            .cloned()
            .expect("the opened tunnel was not persisted");
        let mut filler = 0_usize;
        while store.open_tunnel_count(REGION) < MAX_CONCURRENT_TUNNELS {
            let tunnel_id = format!("filler-{filler}");
            let tunnel_arn = format!("arn:aws:iot:{REGION}:000000000000:tunnel/{tunnel_id}");
            let mut tunnel = seed.clone();
            tunnel.tunnel_id = tunnel_id.clone();
            tunnel.tunnel_arn = tunnel_arn.clone();
            store.insert_tunnel(REGION, tunnel);
            filler += 1;
        }
    }

    let case = "an account holding its quota of open tunnels";
    let stored_before = engine.state::<TunnelState>().lock().tunnels.len();
    let (status, body) = open_with_tags(&router, tags_of(1, "k")).await;
    assert_limit_refusal(
        case,
        status,
        &body,
        MAX_CONCURRENT_TUNNELS,
        MAX_CONCURRENT_TUNNELS,
    );
    assert_eq!(
        engine.state::<TunnelState>().lock().tunnels.len(),
        stored_before,
        "case {case:?}: a refused open must store no record"
    );
}

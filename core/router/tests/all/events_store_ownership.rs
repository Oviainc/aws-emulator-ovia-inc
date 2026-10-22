// SPDX-License-Identifier: CC0-1.0

//! Which MODEL's store an `events`-signed request lands in, with every claimant mounted.
//!
//! Three mounted models resolve to the key `events`: EventBridge
//! (`com.amazonaws.eventbridge#AWSEvents`, `awsJson1_1`, endpoint prefix `events`), the
//! LEGACY CloudWatch Events model of that same service
//! (`com.amazonaws.cloudwatchevents#AWSEvents` — same version, same protocol, same
//! endpoint prefix, its 51 operations a subset of the other's 57) and EventBridge V2
//! (`com.amazonaws.eventbridgev2#AWSEventsV2`, `rpcv2Cbor`, endpoint prefix `eventsv2`).
//! While each claimed on the signing name alone the router had several claimants for
//! every such request and refused all of them with `500 AmbiguousService` — a code no AWS
//! model declares — so no EventBridge operation was reachable at all.
//!
//! `events_signing_name_routing.rs` pins WHICH service claims each request form. This
//! suite pins the consequence only a STORE can observe, and which a claim table cannot:
//! with all three mounted over ONE shared engine, what a request WROTE is in exactly one
//! model's state — the one that answered it — and in none of the others. The rows differ
//! along the route key the request names (`events` and the legacy model's own
//! sdkId-derived `cloudwatchevents`), the operation, and which model must hold the
//! result; the wire form is held constant (`POST /` with the same `X-Amz-Target`), so the
//! key is the only thing that decides.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and the authority they arrive on — both
/// awsJson models publish endpoint prefix `events`, which is exactly why the route key
/// has to tell them apart.
const REGION: &str = "eu-west-1";
const HOST: &str = "events.eu-west-1.amazonaws.com";

/// A SigV4 credential scope naming the `events` key, which the EventBridge model owns.
/// The signature is never checked: the router below runs with request authentication off.
const SIGNED_FOR_EVENTS: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/events/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The same scope naming the LEGACY model's own sdkId-derived key, which only it claims.
const SIGNED_FOR_LEGACY: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/cloudwatchevents/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The bus every account owns, used when a request names none.
const DEFAULT_BUS: &str = "default";

/// The router's own refusals, which no request below may answer: `UnknownService` when no
/// mounted service claimed it, `AmbiguousService` when more than one did
/// (`vendor/smithy-server-runtime/src/multi_service_router.rs`).
const ROUTER_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// All three models that resolve to the `events` key, over ONE shared engine, exactly as
/// `core/router/src/lib.rs` mounts them.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchevents_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_eventbridgev2_awseventsv2::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// One awsJson1_1 call under `authorization` — `POST /` with `X-Amz-Target:
/// AWSEvents.<Operation>`, the wire form BOTH awsJson models publish — as its status, its
/// `__type` error code (the empty string on a success) and its parsed body.
async fn call(
    router: &MultiServiceRouter,
    authorization: &str,
    operation: &str,
    body: &Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", HOST)
        .header("authorization", authorization)
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    (status, code, parsed)
}

/// The rules each awsJson model's own store holds on the default bus, in name order.
fn stored_rules(engine: &Arc<engine::InMemoryEngine>) -> (Vec<String>, Vec<String>) {
    let current = engine
        .state::<service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState>();
    let current_names: Vec<String> = current
        .lock()
        .bus(DEFAULT_BUS)
        .map(|bus| bus.rules.keys().cloned().collect())
        .unwrap_or_default();
    let legacy = engine
        .state::<service_com_amazonaws_cloudwatchevents_awsevents::state::ComAmazonawsCloudwatcheventsAwseventsState>();
    let legacy_names: Vec<String> = legacy
        .lock()
        .bus(DEFAULT_BUS)
        .map(|bus| bus.rules.keys().cloned().collect())
        .unwrap_or_default();
    (current_names, legacy_names)
}

#[tokio::test]
async fn each_route_key_is_served_by_one_model_out_of_its_own_store() {
    let (router, engine) = router_and_engine();

    // (case, authorization, rule name, whether the LEGACY model must hold it)
    let rows: [(&str, &str, &str, bool); 4] = [
        ("the events key", SIGNED_FOR_EVENTS, "events-key-rule", false),
        (
            "the legacy model's own key",
            SIGNED_FOR_LEGACY,
            "legacy-key-rule",
            true,
        ),
        (
            "the events key again, a second rule",
            SIGNED_FOR_EVENTS,
            "events-key-rule.two",
            false,
        ),
        (
            "the legacy key again, a second rule",
            SIGNED_FOR_LEGACY,
            "legacy-key-rule.two",
            true,
        ),
    ];

    for (case, authorization, rule, legacy_owns) in rows {
        let pattern = json!({ "source": [rule] }).to_string();
        let (status, code, body) = call(
            &router,
            authorization,
            "PutRule",
            &json!({ "Name": rule, "EventPattern": pattern }),
        )
        .await;
        for refusal in ROUTER_REFUSALS {
            assert_ne!(
                code.as_str(),
                refusal,
                "case {case:?}: the router refused this request ({refusal}) instead of one \
                 model serving it, got {status} {body}"
            );
        }
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: exactly one model must serve this request, got {status} {body}"
        );

        let (current_rules, legacy_rules) = stored_rules(&engine);
        let (holder, other) = if legacy_owns {
            (legacy_rules, current_rules)
        } else {
            (current_rules, legacy_rules)
        };
        assert!(
            holder.iter().any(|stored| stored == rule),
            "case {case:?}: the model that answered must hold {rule:?}, holds {holder:?}"
        );
        assert!(
            !other.iter().any(|stored| stored == rule),
            "case {case:?}: the other model must hold nothing of it, holds {other:?}"
        );

        // And the rule reads back under the SAME key, out of the same store.
        let (status, code, described) =
            call(&router, authorization, "DescribeRule", &json!({ "Name": rule })).await;
        assert_eq!(
            (status, code.as_str(), described["Name"].clone()),
            (200, "", json!(rule)),
            "case {case:?}: the rule must read back under the key that wrote it, got \
             {status} {described}"
        );
    }

    // Neither model ever saw the other's writes: each store holds exactly the two rules
    // written under its own key.
    let (current_rules, legacy_rules) = stored_rules(&engine);
    assert_eq!(
        (current_rules, legacy_rules),
        (
            vec![
                "events-key-rule".to_string(),
                "events-key-rule.two".to_string()
            ],
            vec![
                "legacy-key-rule".to_string(),
                "legacy-key-rule.two".to_string()
            ]
        ),
        "each model must hold exactly what was written under its own key"
    );
}

/// An operation only the SUPERSET model binds, and one only it implements, are served
/// under the `events` key and land in ITS store: the key reaches that model and not the
/// legacy one, whose `CreateEventBus` is still a scaffold.
#[tokio::test]
async fn an_operation_only_the_superset_model_serves_lands_in_its_store() {
    let (router, engine) = router_and_engine();

    let (status, code, body) = call(
        &router,
        SIGNED_FOR_EVENTS,
        "CreateEventBus",
        &json!({ "Name": "superset-only-bus" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "the `events` key must reach the model that implements CreateEventBus, got \
         {status} {body}"
    );
    assert_eq!(
        body["EventBusArn"],
        json!(format!(
            "arn:aws:events:{REGION}:{}:event-bus/superset-only-bus",
            engine::emulator_account_id()
        )),
        "the bus ARN must name the signed region, got {body}"
    );

    let current = engine
        .state::<service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState>();
    assert!(
        current.lock().bus("superset-only-bus").is_some(),
        "the model that answered must hold the bus it created"
    );

    // The same operation under the legacy model's own key reaches THAT model, whose
    // handler is still scaffolded — so the two keys are answered by two different models
    // and neither is a router refusal.
    let (status, code, body) = call(
        &router,
        SIGNED_FOR_LEGACY,
        "CreateEventBus",
        &json!({ "Name": "legacy-bus" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (501, "NotImplemented"),
        "the legacy key must reach the legacy model, whose CreateEventBus is a scaffold, \
         got {status} {body}"
    );
}

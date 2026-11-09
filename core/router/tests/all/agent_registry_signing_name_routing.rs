// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `agent-registry` SigV4 signing name.
//!
//! The Agent Registry CONTROL plane (`AgentRegistryControl`, endpointPrefix
//! `agent-registry-control`) and the Agent Registry DATA plane (`AgentRegistry`,
//! endpointPrefix `agent-registry`) SHARE one SigV4 signing name, `agent-registry`, and
//! one `arnNamespace`, while serving two different endpoints. The router resolves a
//! request's routing key from the SigV4 credential scope first, so both services see the
//! key `agent-registry`: a service that claims on the signing name alone claims the
//! other's traffic too, and the router — which refuses rather than guesses — answers
//! `500 AmbiguousService` with an awsJson body, an error code no AWS model declares and a
//! 5xx an SDK retries to exhaustion.
//!
//! The rule these suites pin: a request signed for a signing name several emulated
//! services share is claimed by exactly ONE of them — the one whose own model binds a
//! ROUTE (an HTTP method AND a URI) for the request, with the endpoint the routing key
//! names deciding first. No first path segment can separate this pair: both models publish
//! `registries` (`POST /registries` and `GET /registries/{registryId}` on the control
//! plane, `POST /registries/{registryId}/discoverable-records-list` on the data plane),
//! which is exactly how AWS dispatches it — the endpoint host, then the `restJson1` route.
//!
//! Rows therefore span the dimensions that rule generalises over: which endpoint owns the
//! route, the method (`/registries` is bound by one model for POST and by neither for
//! GET), the depth at which the two models' URIs diverge, the greedy `{resourceArn+}`
//! label, a route neither model binds, and whether the target is named by a credential
//! scope or — on an unsigned request — by the endpoint host.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names the signing name both
/// services share. The signature is never checked: the routers here run with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// How a row names the service that must claim it. `Both` and `Neither` are the two
/// routing failures: `Both` is the `500 AmbiguousService` this issue is about, `Neither`
/// the router's `404 UnknownService`.
const OWNER_CONTROL: &str = "agent-registry-control";
const OWNER_DATA: &str = "agent-registry";
const OWNER_BOTH: &str = "both (the router answers 500 AmbiguousService)";
const OWNER_NEITHER: &str = "neither (the router answers 404 UnknownService)";

/// The host that names the control plane's endpoint, the one that names the data plane's,
/// and a bare emulator endpoint that names neither.
const CONTROL_HOST: &str = "agent-registry-control.us-west-2.amazonaws.com";
const DATA_HOST: &str = "agent-registry.us-west-2.amazonaws.com";
const BARE_HOST: &str = "localhost:9297";

/// A registry id of `RegistryId`'s shape (`^[a-zA-Z0-9]{12,16}$`) and a record id of
/// `RegistryRecordId`'s (`^[a-zA-Z0-9]{12}$`), for the path labels the rows address.
const REGISTRY_ID: &str = "aaaaaaaaaaaa";
const RECORD_ID: &str = "bbbbbbbbbbbb";

type Control =
    service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService<
        engine::InMemoryEngine,
    >;
type Data = service_com_amazonaws_agentregistry_agentregistry::EmulatedService<
    engine::InMemoryEngine,
>;

/// The two services that share the `agent-registry` signing name, over one shared engine,
/// exactly as `router/src/lib.rs` mounts them.
fn both_services() -> (Control, Data) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (Control::new(Arc::clone(&engine)), Data::new(engine))
}

/// A router with both of them mounted, so a request that two services claim really is
/// refused as `AmbiguousService`.
fn router_with_both_services() -> MultiServiceRouter {
    let (control, data) = both_services();
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(control);
    router.register_service(data);
    router
}

/// The request parts the router hands to `Service::can_handle`. `signed` chooses whether
/// the request carries a credential scope at all: an unsigned one is routed by its host.
fn parts(method: &str, host: &str, uri: &str, signed: bool) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json");
    if signed {
        builder = builder.header("authorization", AUTHORIZATION);
    }
    builder
        .body(Bytes::new())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

/// The service that claims a request, as one of the four `OWNER_*` outcomes.
fn claimant(control: &Control, data: &Data, parts: &http::request::Parts) -> &'static str {
    match (control.can_handle(parts), data.can_handle(parts)) {
        (true, false) => OWNER_CONTROL,
        (false, true) => OWNER_DATA,
        (true, true) => OWNER_BOTH,
        (false, false) => OWNER_NEITHER,
    }
}

/// One answer from the router: the HTTP status, the restJson1 error code (the empty
/// string on a response that carries none, read from both the body's `__type` and the
/// `x-amzn-errortype` header the protocol frames an error with) and the content type.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Value) -> (u16, String, String) {
    let payload = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", BARE_HOST)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = if header_code.is_empty() {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    let code = code.rsplit(['#', '/']).next().unwrap_or_default().to_string();
    (status, code, content_type)
}

/// The ARN of a registry, for the greedy `{resourceArn+}` label the tagging routes bind.
fn registry_arn() -> String {
    format!("arn:aws:agent-registry:us-west-2:000000000000:registry/{REGISTRY_ID}")
}

#[test]
fn exactly_one_service_sharing_the_agent_registry_signing_name_claims_each_request() {
    let (control, data) = both_services();

    // Every route below comes from the two models' `smithy.api#http` bindings, as each
    // crate's `OPERATIONS.md` lists them (`smithy_query(query="digest", …)` on each
    // model). The rows are (case, method, host, uri, the service that must claim it).
    let tags_path = format!("/tags/{}", registry_arn());
    let rows: Vec<(String, &str, &str, String, &str)> = vec![
        // The control plane's own routes, on hosts that name it, name its sibling, and
        // name nothing at all: on a SIGNED request the credential scope resolves the key,
        // so the host cannot change the answer.
        ("control CreateRegistry".to_string(), "POST", CONTROL_HOST, "/registries".to_string(), OWNER_CONTROL),
        ("control CreateRegistry on the sibling's host".to_string(), "POST", DATA_HOST, "/registries".to_string(), OWNER_CONTROL),
        ("control CreateRegistry on a bare emulator endpoint".to_string(), "POST", BARE_HOST, "/registries".to_string(), OWNER_CONTROL),
        ("control GetRegistry".to_string(), "GET", BARE_HOST, format!("/registries/{REGISTRY_ID}"), OWNER_CONTROL),
        ("control UpdateRegistry".to_string(), "PATCH", BARE_HOST, format!("/registries/{REGISTRY_ID}"), OWNER_CONTROL),
        ("control DeleteRegistry".to_string(), "DELETE", BARE_HOST, format!("/registries/{REGISTRY_ID}"), OWNER_CONTROL),
        ("control ListRegistries".to_string(), "POST", BARE_HOST, "/registries-list".to_string(), OWNER_CONTROL),
        ("control CreateRegistryRecord".to_string(), "POST", BARE_HOST, format!("/registries/{REGISTRY_ID}/records"), OWNER_CONTROL),
        ("control ListRegistryRecords".to_string(), "POST", BARE_HOST, format!("/registries/{REGISTRY_ID}/records-list"), OWNER_CONTROL),
        ("control GetRegistryRecord".to_string(), "GET", BARE_HOST, format!("/registries/{REGISTRY_ID}/records/{RECORD_ID}"), OWNER_CONTROL),
        ("control UpdateRegistryRecord".to_string(), "PATCH", BARE_HOST, format!("/registries/{REGISTRY_ID}/records/{RECORD_ID}"), OWNER_CONTROL),
        ("control UpdateRegistryRecordStatus".to_string(), "PATCH", BARE_HOST, format!("/registries/{REGISTRY_ID}/records/{RECORD_ID}/status"), OWNER_CONTROL),
        ("control SubmitRegistryRecordForApproval".to_string(), "POST", BARE_HOST, format!("/registries/{REGISTRY_ID}/records/{RECORD_ID}/submit-for-approval"), OWNER_CONTROL),
        // The greedy `{resourceArn+}` label: one path space, three methods, all the
        // control plane's.
        ("control TagResource".to_string(), "POST", BARE_HOST, tags_path.clone(), OWNER_CONTROL),
        ("control ListTagsForResource".to_string(), "GET", BARE_HOST, tags_path.clone(), OWNER_CONTROL),
        ("control UntagResource".to_string(), "DELETE", BARE_HOST, format!("{tags_path}?tagKeys=team"), OWNER_CONTROL),
        // The data plane's own routes, including the one whose FIRST path segment the
        // control plane publishes too — the segment that makes a path-prefix claim
        // ambiguous and a route claim exact.
        ("data BatchGetDiscoverableRegistryRecord".to_string(), "POST", BARE_HOST, "/discoverable-records-batch".to_string(), OWNER_DATA),
        ("data SearchDiscoverableRegistryRecords".to_string(), "POST", BARE_HOST, "/discoverable-records-search".to_string(), OWNER_DATA),
        ("data ListDiscoverableRegistryRecords".to_string(), "POST", BARE_HOST, format!("/registries/{REGISTRY_ID}/discoverable-records-list"), OWNER_DATA),
        ("data ListDiscoverableRegistryRecords on the sibling's host".to_string(), "POST", CONTROL_HOST, format!("/registries/{REGISTRY_ID}/discoverable-records-list"), OWNER_DATA),
        // Routes NEITHER model binds: a method no model binds on a path one of them owns,
        // a path depth neither reaches, and a path space neither publishes. None of these
        // may be claimed by two services, which is what made every one of them a 500.
        ("no model binds GET /registries".to_string(), "GET", BARE_HOST, "/registries".to_string(), OWNER_NEITHER),
        ("no model binds POST on a registry".to_string(), "POST", BARE_HOST, format!("/registries/{REGISTRY_ID}"), OWNER_NEITHER),
        ("no model binds GET on the search route".to_string(), "GET", BARE_HOST, "/discoverable-records-search".to_string(), OWNER_NEITHER),
        ("no model publishes this path space".to_string(), "POST", BARE_HOST, "/nothing-here".to_string(), OWNER_NEITHER),
    ];

    for (case, method, host, uri, expected) in rows {
        let claimed = claimant(&control, &data, &parts(method, host, &uri, true));
        assert_eq!(
            claimed, expected,
            "case {case:?} ({method} {uri} on {host}): exactly one service must claim it, \
             got {claimed:?}"
        );
    }
}

#[test]
fn an_unsigned_request_is_claimed_by_the_endpoint_its_host_names() {
    let (control, data) = both_services();

    // With no credential scope the routing key comes from the `Host` label, which is each
    // service's `endpointPrefix` — the two differ (`agent-registry-control` names only the
    // control plane; `agent-registry` is also the shared signing name, so it names both
    // and the route decides). Rows span both hosts against both planes' routes.
    let rows: Vec<(String, &str, &str, String, &str)> = vec![
        ("control route on the control host".to_string(), "POST", CONTROL_HOST, "/registries".to_string(), OWNER_CONTROL),
        ("control route on the shared host".to_string(), "POST", DATA_HOST, "/registries".to_string(), OWNER_CONTROL),
        ("data route on the shared host".to_string(), "POST", DATA_HOST, "/discoverable-records-search".to_string(), OWNER_DATA),
        ("data route on the control host".to_string(), "POST", CONTROL_HOST, "/discoverable-records-search".to_string(), OWNER_NEITHER),
        ("shared first segment on the control host".to_string(), "POST", CONTROL_HOST, format!("/registries/{REGISTRY_ID}/discoverable-records-list"), OWNER_NEITHER),
        ("shared first segment on the shared host".to_string(), "POST", DATA_HOST, format!("/registries/{REGISTRY_ID}/discoverable-records-list"), OWNER_DATA),
    ];

    for (case, method, host, uri, expected) in rows {
        let claimed = claimant(&control, &data, &parts(method, host, &uri, false));
        assert_eq!(
            claimed, expected,
            "case {case:?} ({method} {uri} on {host}, unsigned): the endpoint the host \
             names and the route its model binds decide, got {claimed:?}"
        );
    }
}

#[tokio::test]
async fn no_agent_registry_request_is_answered_by_the_routers_refusals() {
    let router = router_with_both_services();

    // The two routing failures no row may provoke: `AmbiguousService` (500, several
    // claimants — rendered in an awsJson envelope whatever the claimants' own protocol is)
    // and `UnknownService` (404, none claimed) on a route a model DOES bind. Every row
    // must be answered by one service's own `restJson1` envelope instead.
    let rows: Vec<(String, &str, String, Value)> = vec![
        ("CreateRegistry".to_string(), "POST", "/registries".to_string(), json!({ "name": "routing-probe" })),
        ("ListRegistries".to_string(), "POST", "/registries-list".to_string(), json!({})),
        ("GetRegistry".to_string(), "GET", format!("/registries/{REGISTRY_ID}"), Value::Null),
        ("ListTagsForResource".to_string(), "GET", format!("/tags/{}", registry_arn()), Value::Null),
        ("SearchDiscoverableRegistryRecords".to_string(), "POST", "/discoverable-records-search".to_string(), json!({ "registryId": REGISTRY_ID, "query": "anything" })),
        ("ListDiscoverableRegistryRecords".to_string(), "POST", format!("/registries/{REGISTRY_ID}/discoverable-records-list"), json!({})),
    ];

    for (case, method, uri, body) in rows {
        let (status, code, content_type) = call(&router, method, &uri, body).await;
        assert_ne!(
            code, "AmbiguousService",
            "case {case:?}: a request two services claim is refused 500 AmbiguousService; \
             exactly one must claim it, got {status} {code:?}"
        );
        assert_ne!(
            code, "UnknownService",
            "case {case:?}: a route a model binds must be claimed, got {status} {code:?}"
        );
        assert_ne!(
            status, 500,
            "case {case:?}: no modelled route answers a 5xx, got {status} {code:?}"
        );
        assert!(
            content_type.starts_with("application/json"),
            "case {case:?}: restJson1 frames every answer as application/json, got \
             {content_type:?} with {status} {code:?}"
        );
    }

    // And a route NEITHER model binds is the router's honest `404 UnknownService` — not
    // the 500 two claimants produce.
    let (status, code, _) = call(&router, "POST", "/nothing-here", json!({})).await;
    assert_eq!(
        (status, code.as_str()),
        (404, "UnknownService"),
        "case \"a path no model publishes\": nobody claims it, got {status} {code:?}"
    );
}

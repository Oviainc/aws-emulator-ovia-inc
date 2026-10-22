// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS, now that its signing name is not enough.
//!
//! ConnectCampaignsV2 signs as `connect-campaigns` — and so does ConnectCampaigns v1
//! (same `aws.auth#sigv4` name, same `arnNamespace`, same endpoint host family). A
//! service that claims every request carrying its signing name therefore claims the
//! other API version's traffic too, and a router holding both finds two claimants and
//! refuses the request before dispatch instead of serving either one.
//!
//! The rule these cases pin: a service claims a request only when the resolved routing
//! key is its signing name AND the request's first URI path segment is one its own
//! model declares. The two API versions' modelled path spaces are disjoint — every V2
//! route is `/v2/`-prefixed, every v1 route is unprefixed — so the conjunct separates
//! them, whatever signal the routing key came from and whatever the endpoint host is.
//!
//! The rows differ along what the rule generalises over: the path (every modelled V2
//! route family, the peer's route families, near-miss segments), the HTTP method, and
//! the signal the routing key is resolved from (SigV4 scope, endpoint host, none).

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::{
    EmulatedService, ROUTE_KEY, ROUTE_PATH_SEGMENTS,
};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names `service`. The scope's
/// 4th field is the primary signal the router resolves a routing key from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    )
}

/// How a case's request names the service it is addressed to.
#[derive(Debug, Clone, Copy)]
enum RouteSignal {
    /// A SigV4 credential scope naming this service's signing name.
    SignedForThisService,
    /// A SigV4 credential scope naming some other service.
    SignedFor(&'static str),
    /// No credentials at all, addressed by this service's endpoint host — the
    /// host-style signal an anonymous request routes by.
    HostOnly(&'static str),
    /// Neither: nothing names a service, so no routing key resolves.
    Nothing,
}

/// One claim case: what the request looks like, and whether this service must claim it.
#[derive(Debug)]
struct Case {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    signal: RouteSignal,
    claimed: bool,
}

/// The request parts a case describes, as `can_handle` is handed them.
fn parts(case: &Case) -> http::request::Parts {
    let mut builder = http::Request::builder().method(case.method);
    builder = match case.signal {
        RouteSignal::SignedForThisService => builder
            .uri(format!("https://connect-campaigns.us-east-1.amazonaws.com{}", case.path))
            .header("host", "connect-campaigns.us-east-1.amazonaws.com")
            .header("authorization", authorization(ROUTE_KEY)),
        RouteSignal::SignedFor(service) => builder
            .uri(format!("https://{service}.us-east-1.amazonaws.com{}", case.path))
            .header("host", format!("{service}.us-east-1.amazonaws.com"))
            .header("authorization", authorization(service)),
        RouteSignal::HostOnly(host) => builder
            .uri(format!("https://{host}{}", case.path))
            .header("host", host),
        RouteSignal::Nothing => builder.uri(format!("http://127.0.0.1:9297{}", case.path)),
    };
    let (parts, ()) = builder
        .body(())
        .expect("a well-formed test request")
        .into_parts();
    parts
}

/// Every distinct URI path family this service's model declares, plus the peer's and a
/// set of near misses.
///
/// Source for the V2 routes: `smithy_query(query="digest", model_path=
/// "services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/\
/// smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json")`,
/// whose 37 operations bind `/v2/campaigns`, `/v2/campaigns/{id}` and its
/// sub-resources, `/v2/campaigns-state`, `/v2/campaigns-summary`,
/// `/v2/connect-instance/{connectInstanceId}/…` and `/v2/tags/{arn}`.
///
/// Source for the peer routes: the same query against
/// `services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/\
/// smithy-model/com-amazonaws-connectcampaigns-amazonconnectcampaignservice.json`,
/// whose 22 operations bind the SAME families unprefixed (`/campaigns`,
/// `/campaigns-state`, `/campaigns-summary`, `/connect-instance/{id}/…`, `/tags/{arn}`).
const CASES: &[Case] = &[
    // ---- this service's own modelled route families ----
    Case {
        name: "CreateCampaign",
        method: "PUT",
        path: "/v2/campaigns",
        signal: RouteSignal::SignedForThisService,
        claimed: true,
    },
    Case {
        name: "DescribeCampaign",
        method: "GET",
        path: "/v2/campaigns/000000000000",
        signal: RouteSignal::SignedForThisService,
        claimed: true,
    },
    Case {
        name: "PutOutboundRequestBatch",
        method: "PUT",
        path: "/v2/campaigns/000000000000/outbound-requests",
        signal: RouteSignal::SignedForThisService,
        claimed: true,
    },
    Case {
        name: "GetCampaignStateBatch",
        method: "POST",
        path: "/v2/campaigns-state",
        signal: RouteSignal::SignedForThisService,
        claimed: true,
    },
    Case {
        name: "ListCampaigns",
        method: "POST",
        path: "/v2/campaigns-summary",
        signal: RouteSignal::SignedForThisService,
        claimed: true,
    },
    Case {
        name: "StartInstanceOnboardingJob",
        method: "PUT",
        path: "/v2/connect-instance/instance-1/onboarding",
        signal: RouteSignal::SignedForThisService,
        claimed: true,
    },
    Case {
        name: "ListTagsForResource",
        method: "GET",
        path: "/v2/tags/arn%3Aaws%3Aconnect-campaigns%3Aus-east-1%3A000000000000%3Acampaign%2F1",
        signal: RouteSignal::SignedForThisService,
        claimed: true,
    },
    Case {
        name: "DeleteCampaign",
        method: "DELETE",
        path: "/v2/campaigns/000000000000",
        signal: RouteSignal::SignedForThisService,
        claimed: true,
    },
    Case {
        name: "a V2 route with a query string",
        method: "POST",
        path: "/v2/campaigns-summary?maxResults=1",
        signal: RouteSignal::SignedForThisService,
        claimed: true,
    },
    Case {
        name: "a V2 route reached anonymously on the shared endpoint host",
        method: "PUT",
        path: "/v2/campaigns",
        signal: RouteSignal::HostOnly("connect-campaigns.us-east-1.amazonaws.com"),
        claimed: true,
    },
    // ---- the peer API version's route families: NOT this service's ----
    Case {
        name: "v1 CreateCampaign",
        method: "PUT",
        path: "/campaigns",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    Case {
        name: "v1 DescribeCampaign",
        method: "GET",
        path: "/campaigns/000000000000",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    Case {
        name: "v1 PutDialRequestBatch",
        method: "PUT",
        path: "/campaigns/000000000000/dial-requests",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    Case {
        name: "v1 GetCampaignStateBatch",
        method: "POST",
        path: "/campaigns-state",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    Case {
        name: "v1 ListCampaigns",
        method: "POST",
        path: "/campaigns-summary",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    Case {
        name: "v1 StartInstanceOnboardingJob",
        method: "PUT",
        path: "/connect-instance/instance-1/onboarding",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    Case {
        name: "v1 ListTagsForResource",
        method: "GET",
        path: "/tags/arn%3Aaws%3Aconnect-campaigns%3Aus-east-1%3A000000000000%3Acampaign%2F1",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    // ---- near misses: the segment is matched by EXACT equality, never by prefix ----
    Case {
        name: "a longer segment the V2 prefix starts",
        method: "PUT",
        path: "/v22/campaigns",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    Case {
        name: "a segment that merely contains the V2 prefix",
        method: "PUT",
        path: "/campaigns-v2",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    Case {
        name: "the next API version's prefix",
        method: "PUT",
        path: "/v3/campaigns",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    Case {
        name: "the V2 prefix in the wrong case (HTTP paths are case-sensitive)",
        method: "PUT",
        path: "/V2/campaigns",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    Case {
        name: "the root path",
        method: "POST",
        path: "/",
        signal: RouteSignal::SignedForThisService,
        claimed: false,
    },
    // ---- the key conjunct still holds: a V2 path alone is not a claim ----
    Case {
        name: "a V2 path signed for another service",
        method: "PUT",
        path: "/v2/campaigns",
        signal: RouteSignal::SignedFor("connect"),
        claimed: false,
    },
    Case {
        name: "a V2 path on another service's endpoint host",
        method: "PUT",
        path: "/v2/campaigns",
        signal: RouteSignal::HostOnly("connect.us-east-1.amazonaws.com"),
        claimed: false,
    },
    Case {
        name: "a V2 path naming no service at all",
        method: "PUT",
        path: "/v2/campaigns",
        signal: RouteSignal::Nothing,
        claimed: false,
    },
];

#[test]
fn this_service_claims_its_own_modelled_path_space_and_nothing_else() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    for case in CASES {
        let claimed = service.can_handle(&parts(case));
        assert_eq!(
            claimed, case.claimed,
            "case {:?} ({} {}, {:?}): expected can_handle == {}, got {claimed}",
            case.name, case.method, case.path, case.signal, case.claimed
        );
    }
}

#[test]
fn the_generated_path_discriminator_covers_every_modelled_route() {
    assert!(
        !ROUTE_PATH_SEGMENTS.is_empty(),
        "every modelled route begins with a literal segment, so the generated \
         discriminator must not be empty: {ROUTE_PATH_SEGMENTS:?}"
    );
    assert!(
        !ROUTE_PATH_SEGMENTS.contains(&emulator_server::ROOT_PATH_SEGMENT),
        "this restJson1 service binds no operation to the root path, so it must not \
         claim it: {ROUTE_PATH_SEGMENTS:?}"
    );
    // Every one of the model's 37 operations binds a `/v2/`-prefixed URI (source: the
    // `smithy_query` digest cited on `CASES`), so the derived set is exactly that one
    // segment — and in particular holds no segment of the peer's unprefixed space.
    assert_eq!(
        ROUTE_PATH_SEGMENTS,
        ["v2"],
        "expected the one segment every modelled V2 route begins with, got \
         {ROUTE_PATH_SEGMENTS:?}"
    );
}

#[tokio::test]
async fn a_peer_route_never_reaches_this_service_while_its_own_route_is_served() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the core's concern; the header is a routing signal here.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));

    let create_body = serde_json::json!({
        "name": "campaign-1",
        "connectInstanceId": "instance-1",
        "channelSubtypeConfig": {
            "telephony": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {"connectContactFlowId": "flow-1"}
            }
        }
    });

    // The peer API version's create route, carrying the signing name the two versions
    // share. This service must not claim it, so nothing reaches its store.
    let (status, payload) = call(&router, "PUT", "/campaigns", Some(&create_body)).await;
    assert_ne!(
        status, 200,
        "case {:?}: the peer version's route must not be served by this service, got \
         {status} {payload}",
        "v1 CreateCampaign on the shared signing name"
    );
    let (status, payload) = call(&router, "POST", "/v2/campaigns-summary", Some(&Value::Null)).await;
    assert_eq!(
        (status, summary_len(&payload)),
        (200, 0),
        "case {:?}: a request on the peer's route must store no campaign here, got \
         {status} {payload}",
        "ListCampaigns after the peer-route call"
    );

    // This service's own create route, identical in every other respect, is served.
    let (status, payload) = call(&router, "PUT", "/v2/campaigns", Some(&create_body)).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected this service to serve its own modelled route, got {status} \
         {payload}",
        "V2 CreateCampaign"
    );
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "case {:?}: expected a minted campaign id, got {payload}",
        "V2 CreateCampaign"
    );

    // And the campaign it wrote is read back through this service's own reader.
    let (status, payload) = call(&router, "GET", &format!("/v2/campaigns/{id}"), None).await;
    assert_eq!(
        (
            status,
            payload
                .pointer("/campaign/name")
                .and_then(Value::as_str)
                .unwrap_or("<absent>")
        ),
        (200, "campaign-1"),
        "case {:?}: expected the stored campaign back, got {status} {payload}",
        "DescribeCampaign round trip"
    );
    let (status, payload) = call(&router, "POST", "/v2/campaigns-summary", Some(&Value::Null)).await;
    assert_eq!(
        (status, summary_len(&payload)),
        (200, 1),
        "case {:?}: expected exactly the campaign created through the V2 route, got \
         {status} {payload}",
        "ListCampaigns after the V2 create"
    );
}

/// The number of entries `ListCampaigns` reported, or `usize::MAX` when the response
/// carries no list at all (so a mismatch prints the whole payload).
fn summary_len(payload: &Value) -> usize {
    payload
        .get("campaignSummaryList")
        .and_then(Value::as_array)
        .map_or(usize::MAX, Vec::len)
}

/// Drive one request through the router and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = match body {
        Some(value) if !value.is_null() => {
            bytes::Bytes::from(serde_json::to_vec(value).expect("a test body serializes"))
        }
        Some(_) => bytes::Bytes::from_static(b"{}"),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://127.0.0.1:9297{path}"))
        .header("host", "connect-campaigns.us-east-1.amazonaws.com")
        .header("authorization", authorization(ROUTE_KEY))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

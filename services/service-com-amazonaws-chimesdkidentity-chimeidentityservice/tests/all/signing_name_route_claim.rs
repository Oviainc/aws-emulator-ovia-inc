// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS, driven through the production router.
//!
//! RULE: a SigV4 signing name is not a service identity. `chime` is the signing name of
//! SIX distinct AWS services, each serving its OWN endpoint, so this service claims a
//! request only when the request ADDRESSES it:
//!
//! 1. the endpoint the request names — the `Host`/`:authority` label — is this service's
//!    own (`identity-chime`), in which case this endpoint serves the request whatever its
//!    path, exactly as a client pointed at that host expects;
//! 2. and when the request names no endpoint of the family at all (a bare emulator host,
//!    which is what a local client sends), the URI path space THIS model binds: its own
//!    exclusive first segments, plus its share of a space a sibling also binds — the
//!    `/app-instances` resource space, whose streaming-configuration sub-space belongs to
//!    Chime SDK Messaging, and the `/tags` space five of the six bind IDENTICALLY, which
//!    names no service in a request that carries no endpoint and is therefore served by
//!    this service as the family's declared tagging owner, out of the ARN-keyed tag
//!    directory the family shares.
//!
//! Every case drives `MultiServiceRouter::dispatch`, the same path the production front
//! door uses, so what is asserted is the wire behaviour a client sees. The tables span
//! what the rule generalises over: the method, the depth and form of the URI, the query
//! constraint, whether the claimed route's operation is implemented or still scaffolded,
//! and how the request names the service it is for.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_chimesdkidentity_chimeidentityservice::EmulatedService;

// The ONE percent-encoder in this test binary, shared from the lifecycle suite rather
// than copied: an ARN carries `:` and `/`, and a non-greedy `@httpLabel` is one segment.
use crate::app_instance_lifecycle::uri_encode;

/// The host a local client reaches this emulator on: it names no AWS endpoint at all, so
/// the claim falls to the path half of the rule. (The framework's host reader rejects
/// loopback names and IP literals, which name no service.)
const BARE_HOST: &str = "localhost:9297";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature itself is never checked: the test router runs with request authentication
/// off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=00"
    )
}

/// Just this one service, mounted on a shared engine behind the production router.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// How a request names the service it is for.
#[derive(Clone, Copy)]
enum Addressed {
    /// Signed for a signing name (the 4th credential-scope field), on a host that names
    /// no endpoint — the shape a local SDK client sends.
    SignedFor(&'static str),
    /// Unsigned, addressing an endpoint host — the signal the router falls back to, and
    /// the one that names a single member of this family.
    HostOnly(&'static str),
}

/// Dispatch one request and report `(status, error code, body)`.
///
/// The error code comes from the body, which is what separates a service's own answer
/// from a routing failure: the router answers `UnknownService` (404) when nobody claims
/// a request and `AmbiguousService` (500) when several do, so an expectation naming this
/// service's own code — or a member only its output carries — can only hold when this
/// service claimed the request.
async fn dispatch(
    router: &MultiServiceRouter,
    addressed: Addressed,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> (u16, Option<String>, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
        .unwrap_or_default();
    let host = match addressed {
        Addressed::SignedFor(_) => BARE_HOST,
        Addressed::HostOnly(host) => host,
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path_and_query}"))
        .header("host", host)
        .header("content-type", "application/json");
    if let Addressed::SignedFor(signing_name) = addressed {
        builder = builder.header("authorization", authorization(signing_name));
    }
    let request = builder.body(payload).expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let code = parsed
        .get("__type")
        .or_else(|| parsed.get("Code"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, parsed)
}

/// Create one `AppInstance` through the router and return its ARN.
async fn app_instance(router: &MultiServiceRouter, token: &str) -> String {
    let (status, code, body) = dispatch(
        router,
        Addressed::SignedFor("chime"),
        "POST",
        "/app-instances",
        Some(json!({"Name": "route-claim", "ClientRequestToken": token})),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateAppInstance must be claimed and served: got {status} {code:?} {body}"
    );
    body["AppInstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAppInstance returned no ARN: {body}"))
        .to_string()
}

/// What a row expects of the request.
enum Expect {
    /// This service claimed it, answered `status`, and the body carries this member.
    Member(u16, &'static str),
    /// This service claimed it, answered `status`, and the body carries this error code.
    Code(u16, &'static str),
    /// This service claimed it and answered this bodiless status.
    Status(u16),
    /// This service declined it, so with only this service mounted nobody claimed it.
    NotClaimed,
}

/// One case of a claim table: a request, and what this service must do with it.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    expect: Expect,
}

/// Dispatch every row of a table and assert its expectation.
async fn check(router: &MultiServiceRouter, addressed: Addressed, rows: Vec<Row>) {
    for row in rows {
        let Row {
            case,
            method,
            uri,
            body,
            expect,
        } = row;
        let (status, code, payload) = dispatch(router, addressed, method, &uri, body).await;
        match expect {
            Expect::Member(expected_status, member) => {
                assert_eq!(
                    (status, code.as_deref()),
                    (expected_status, None),
                    "case {case:?}: expected HTTP {expected_status} from this service for \
                     {method} {uri}, got {status} {code:?} {payload}"
                );
                assert!(
                    payload.get(member).is_some(),
                    "case {case:?}: expected this service's {member} in the response, \
                     got {payload}"
                );
            }
            Expect::Code(expected_status, expected_code) => assert_eq!(
                (status, code.as_deref()),
                (expected_status, Some(expected_code)),
                "case {case:?}: expected HTTP {expected_status} {expected_code} from this \
                 service for {method} {uri}, got {status} {code:?} {payload}"
            ),
            Expect::Status(expected_status) => assert_eq!(
                (status, code.as_deref()),
                (expected_status, None),
                "case {case:?}: expected HTTP {expected_status} from this service for \
                 {method} {uri}, got {status} {code:?} {payload}"
            ),
            Expect::NotClaimed => assert_eq!(
                (status, code.as_deref()),
                (404, Some("UnknownService")),
                "case {case:?}: {method} {uri} is outside this model's path space, so this \
                 service must leave it to the sibling that owns it, got \
                 {status} {code:?} {payload}"
            ),
        }
    }
}

#[tokio::test]
async fn a_request_naming_no_endpoint_is_claimed_only_inside_this_model_s_path_space() {
    let router = build_router();
    let arn = app_instance(&router, "tok-route-claim").await;
    let encoded = uri_encode(&arn);

    // Rows differ along what the rule generalises over: the HTTP method, the number of
    // URI segments, a literal query constraint, and whether the claimed route's
    // operation is implemented (it answers from this service's own store) or still
    // scaffolded (it answers the scaffold's 501 `NotImplemented`) — a claim is about the
    // path space, not about what is implemented behind it.
    //
    // The declined rows are the path spaces of the SIBLING services that share the
    // `chime` signing name, including one that lives UNDER a first path segment this
    // service owns (`/app-instances/…/streaming-configurations` is Chime SDK
    // Messaging's) — which is why the first path segment alone cannot be the
    // discriminator. The `/tags` space five of the six bind identically is claimed here
    // instead: this service is the family's declared tagging owner for a request that
    // names no endpoint.
    let rows: Vec<Row> = vec![
        Row {
            case: "list app instances, this service's own collection route",
            method: "GET",
            uri: "/app-instances".to_string(),
            body: None,
            expect: Expect::Member(200, "AppInstances"),
        },
        Row {
            case: "describe the created app instance, a one-label route",
            method: "GET",
            uri: format!("/app-instances/{encoded}"),
            body: None,
            expect: Expect::Member(200, "AppInstance"),
        },
        Row {
            case: "create an app instance user, under a segment only this model binds",
            method: "POST",
            uri: "/app-instance-users".to_string(),
            body: Some(json!({
                "AppInstanceArn": arn,
                "AppInstanceUserId": "routeclaim",
                "Name": "Route Claim",
                "ClientRequestToken": "tok-route-claim-user",
            })),
            expect: Expect::Member(201, "AppInstanceUserArn"),
        },
        Row {
            case: "list app instance bots, a collection route under a segment only this model binds",
            method: "GET",
            uri: format!("/app-instance-bots?app-instance-arn={encoded}"),
            body: None,
            expect: Expect::Member(200, "AppInstanceBots"),
        },
        Row {
            case: "put retention settings, a two-label route under a non-GET method",
            method: "PUT",
            uri: format!("/app-instances/{encoded}/retention-settings"),
            body: Some(json!({"AppInstanceRetentionSettings": {"ChannelRetentionSettings": {"RetentionDays": 30}}})),
            expect: Expect::Member(200, "AppInstanceRetentionSettings"),
        },
        Row {
            case: "retention settings, a two-label route served from this service's store",
            method: "GET",
            uri: format!("/app-instances/{encoded}/retention-settings"),
            body: None,
            expect: Expect::Member(200, "AppInstanceRetentionSettings"),
        },
        Row {
            // `GET /tags?arn=…` IS a route this model binds — and four siblings bind it
            // identically. The resource ARN names the service and a request naming no
            // endpoint does not carry it readably (tag and untag put it in the body), so
            // the space is served by one declared owner — this service — out of the tag
            // directory the whole family shares.
            case: "tag a resource, the shared space under POST and a query constraint",
            method: "POST",
            uri: "/tags?operation=tag-resource".to_string(),
            body: Some(json!({"ResourceARN": arn, "Tags": [{"Key": "k", "Value": "v"}]})),
            expect: Expect::Status(204),
        },
        Row {
            case: "list tags, a route five of the six bind identically",
            method: "GET",
            uri: format!("/tags?arn={encoded}"),
            body: None,
            expect: Expect::Member(200, "Tags"),
        },
        Row {
            case: "delete the app instance, a route bound only under DELETE",
            method: "DELETE",
            uri: format!("/app-instances/{encoded}"),
            body: None,
            expect: Expect::Status(204),
        },
        Row {
            case: "messaging's streaming configurations, under a first segment this model owns",
            method: "GET",
            uri: format!("/app-instances/{encoded}/streaming-configurations"),
            body: None,
            expect: Expect::NotClaimed,
        },
        Row {
            case: "messaging's channels collection",
            method: "POST",
            uri: "/channels".to_string(),
            body: Some(json!({"AppInstanceArn": arn, "Name": "c", "ClientRequestToken": "t"})),
            expect: Expect::NotClaimed,
        },
        Row {
            case: "voice's connectors collection",
            method: "GET",
            uri: "/voice-connectors".to_string(),
            body: None,
            expect: Expect::NotClaimed,
        },
        Row {
            case: "the telephony space the legacy and voice endpoints share",
            method: "GET",
            uri: "/phone-numbers".to_string(),
            body: None,
            expect: Expect::NotClaimed,
        },
        Row {
            case: "media pipelines' collection",
            method: "GET",
            uri: "/sdk-media-pipelines".to_string(),
            body: None,
            expect: Expect::NotClaimed,
        },
        Row {
            case: "meetings' collection",
            method: "POST",
            uri: "/meetings".to_string(),
            body: Some(json!({
                "ClientRequestToken": "tok-meeting",
                "MediaRegion": "us-east-1",
                "ExternalMeetingId": "external-1",
            })),
            expect: Expect::NotClaimed,
        },
        Row {
            case: "amazon chime's accounts collection",
            method: "GET",
            uri: "/accounts".to_string(),
            body: None,
            expect: Expect::NotClaimed,
        },
        Row {
            case: "a first segment no model in the family binds",
            method: "GET",
            uri: "/not-a-chime-route".to_string(),
            body: None,
            expect: Expect::NotClaimed,
        },
    ];

    check(&router, Addressed::SignedFor("chime"), rows).await;
}

#[tokio::test]
async fn a_request_addressing_this_endpoint_is_served_by_this_service_whatever_its_path() {
    let router = build_router();

    // The endpoint host is the ONE signal that names a single member of the family, so a
    // request carrying it is this endpoint's whatever its path: an operation this model
    // does not bind answers from the service the caller ADDRESSED — its own protocol's
    // routing failure — rather than silently from a sibling, and the `/tags` space five
    // models bind identically is served here too.
    let rows: Vec<Row> = vec![
        Row {
            case: "this model's own collection route",
            method: "GET",
            uri: "/app-instances".to_string(),
            body: None,
            expect: Expect::Member(200, "AppInstances"),
        },
        Row {
            // An app instance this emulator does not hold: the tagging routes read this
            // service's own store for a resource of this service, so the answer is the
            // modelled refusal — which only this service can give, and the router cannot.
            case: "the shared tagging space, reachable because the request names this endpoint",
            method: "GET",
            uri: "/tags?arn=arn%3Aaws%3Achime%3Aus-east-1%3A000000000000%3Aapp-instance%2Fai1"
                .to_string(),
            body: None,
            expect: Expect::Code(400, "BadRequestException"),
        },
        Row {
            case: "a sibling's route, answered by the endpoint the caller addressed",
            method: "POST",
            uri: "/channels".to_string(),
            body: Some(json!({"Name": "c", "ClientRequestToken": "t"})),
            expect: Expect::Code(501, "NotImplemented"),
        },
    ];

    check(
        &router,
        Addressed::HostOnly("identity-chime.us-east-1.amazonaws.com"),
        rows,
    )
    .await;
}

#[tokio::test]
async fn only_this_service_s_own_endpoint_or_the_shared_signing_name_reaches_it() {
    let router = build_router();

    // One request — `GET /app-instances`, a route this model binds — carrying each way of
    // naming a service in turn. The rule: the shared signing name on a host that names no
    // endpoint reaches this service through the path half; this service's OWN endpoint
    // reaches it; a sibling's endpoint does not, this family's legacy endpoint (whose
    // prefix IS the shared signing name) does not, and a signing name outside the family
    // does not.
    let rows: [(&str, Addressed, bool); 5] = [
        (
            "signed for the shared chime signing name, naming no endpoint",
            Addressed::SignedFor("chime"),
            true,
        ),
        (
            "unsigned, addressing this service's own endpoint prefix",
            Addressed::HostOnly("identity-chime.us-east-1.amazonaws.com"),
            true,
        ),
        (
            "unsigned, addressing a sibling's endpoint prefix",
            Addressed::HostOnly("messaging-chime.us-east-1.amazonaws.com"),
            false,
        ),
        (
            "unsigned, addressing the legacy Amazon Chime endpoint",
            Addressed::HostOnly("chime.us-east-1.amazonaws.com"),
            false,
        ),
        (
            "signed for an unrelated service",
            Addressed::SignedFor("s3"),
            false,
        ),
    ];

    for (case, addressed, claimed) in rows {
        let (status, code, payload) =
            dispatch(&router, addressed, "GET", "/app-instances", None).await;
        if claimed {
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: expected this service to answer ListAppInstances, got \
                 {status} {code:?} {payload}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("UnknownService")),
                "case {case:?}: expected the request to reach no service, got \
                 {status} {code:?} {payload}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! Routing the UNSIGNED (anonymous) surface: which requests this service claims.
//!
//! `RegisterClient`, `StartDeviceAuthorization` and `CreateToken` are `@optionalAuth`
//! with an empty auth list, so every SDK sends them with no `Authorization` header; being
//! `restJson1` they carry no `X-Amz-Target` either, and a client pointed at the emulator
//! addresses a loopback name or an IP literal, which names no service. Nothing in such a
//! request names a service, so the claim has to come from the modelled route it
//! addresses — and only from that, so a nameless request outside this model's routes, and
//! a request that NAMES another service, are still claimed by nobody.
//!
//! Every case drives the service through the router the production emulator serves, so
//! what is asserted is what a real `aws sso login` client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ssooidc_awsssooidcservice as ssooidc;
use ssooidc::state::GRANT_TYPE_DEVICE_CODE;

use crate::device_authorization_flow::{error_shape, text, Emulator, START_URL};

/// The router's own refusal when NO registered service claims a request: no service was
/// named and no modelled route attributed it either.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The router's own refusal when SEVERAL services claim one request.
const AMBIGUOUS_SERVICE: &str = "AmbiguousService";

/// The port the emulator's data plane is reached on, so a case addresses it the way a
/// client configured with a local endpoint URL does.
const LOCAL_PORT: u16 = 9297;

/// One way a client that names no service addresses the emulator.
///
/// The two places an endpoint can appear are the `Host` header (HTTP/1) and the request
/// URI's authority (the absolute form an SDK sends, and the HTTP/2 `:authority`), and a
/// request can carry neither. Each row below is one of those shapes.
struct Endpoint {
    /// What this endpoint shape is, for the failure messages.
    case: &'static str,
    /// The `Host` header, when the client sends one.
    host: Option<&'static str>,
    /// The authority in the request URI, when the client sends the absolute form.
    authority: Option<&'static str>,
}

/// The endpoint shapes that name no service — plus, as the last row, the host-style
/// endpoint that DOES name one through the model's `endpointPrefix`, which an unsigned
/// request addressed to the real service resolves to. Every row must reach this service.
fn endpoints() -> Vec<Endpoint> {
    vec![
        Endpoint {
            case: "a loopback Host header on an origin-form URI",
            host: Some("localhost:9297"),
            authority: None,
        },
        Endpoint {
            case: "an IPv4-literal Host header",
            host: Some("127.0.0.1:9297"),
            authority: None,
        },
        Endpoint {
            case: "an absolute-form URI with a loopback authority and no Host header",
            host: None,
            authority: Some("localhost:9297"),
        },
        Endpoint {
            case: "an absolute-form URI with an IPv4-literal authority",
            host: None,
            authority: Some("127.0.0.1:9297"),
        },
        Endpoint {
            case: "no endpoint at all: a bare path and no Host header",
            host: None,
            authority: None,
        },
        Endpoint {
            case: "the host-style endpoint the model publishes (endpointPrefix `oidc`)",
            host: Some("oidc.us-west-2.amazonaws.com"),
            authority: None,
        },
    ]
}

/// A request carrying NO credential scope and no target header: `method path` with
/// `body`, addressed the way `endpoint` says.
fn unsigned(
    endpoint: &Endpoint,
    method: &str,
    path: &str,
    body: &Value,
) -> http::Request<bytes::Bytes> {
    let uri = match endpoint.authority {
        Some(authority) => format!("https://{authority}{path}"),
        None => path.to_string(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(host) = endpoint.host {
        builder = builder.header("host", host);
    }
    builder
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the request builds")
}

/// A request whose SigV4 credential scope names `service`, as a signed SDK request
/// carries it. The endpoint is the local one, so the scope is the only thing that names a
/// service.
fn signed_for(service: &str, method: &str, path: &str, body: &Value) -> http::Request<bytes::Bytes> {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=EMULATORAKTEST/20260101/us-west-2/{service}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    );
    http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("localhost:{LOCAL_PORT}"))
        .header("content-type", "application/json")
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the request builds")
}

/// Assert the request reached NOBODY: the router's own `404 UnknownService`.
fn assert_unclaimed(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected the router's 404 {UNKNOWN_SERVICE}, got {status} with {body}"
    );
    assert!(
        error_shape(body).contains(UNKNOWN_SERVICE),
        "case {case:?}: expected the router's {UNKNOWN_SERVICE}, got {body}"
    );
}

/// Assert the request reached THIS service: neither of the router's routing refusals.
fn assert_claimed(case: &str, status: u16, body: &Value) {
    let shape = body["__type"].as_str().unwrap_or_default().to_string();
    assert!(
        !shape.contains(UNKNOWN_SERVICE) && !shape.contains(AMBIGUOUS_SERVICE),
        "case {case:?}: the request did not reach the service: {status} with {body}"
    );
}

/// RULE: a request that names no target service — no SigV4 credential scope, no RPC
/// target header and no service-naming endpoint host — is attributed by the modelled
/// route it addresses, so every operation this model serves unsigned is reachable at its
/// own method and URI. The table varies the shape of the endpoint the client addressed
/// (the two places an authority can appear, and neither), and each row drives the whole
/// device-authorization surface: the two successes and one modelled error per operation.
#[tokio::test]
async fn an_unsigned_request_routes_by_the_modelled_route_it_addresses() {
    for endpoint in endpoints() {
        let case = endpoint.case;
        let emulator = Emulator::new();

        // RegisterClient -> 200 with a minted clientId/clientSecret pair.
        let (status, registered) = emulator
            .send(unsigned(
                &endpoint,
                "POST",
                "/client/register",
                &json!({
                    "clientName": "anonymous-client",
                    "clientType": "public",
                    "grantTypes": [GRANT_TYPE_DEVICE_CODE],
                }),
            ))
            .await;
        assert_claimed(case, status, &registered);
        assert_eq!(
            status, 200,
            "case {case:?}: expected RegisterClient to answer 200, got {status} with {registered}"
        );
        let client_id = text(&registered, "clientId");
        let client_secret = text(&registered, "clientSecret");
        assert!(
            !client_id.is_empty() && !client_secret.is_empty(),
            "case {case:?}: expected a minted pair, got {registered}"
        );

        // StartDeviceAuthorization -> 200 with the device-flow codes.
        let (status, started) = emulator
            .send(unsigned(
                &endpoint,
                "POST",
                "/device_authorization",
                &json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    "startUrl": START_URL,
                }),
            ))
            .await;
        assert_claimed(case, status, &started);
        assert_eq!(
            status, 200,
            "case {case:?}: expected StartDeviceAuthorization to answer 200, got {status} with \
             {started}"
        );
        for member in ["deviceCode", "userCode", "verificationUri"] {
            assert!(
                !text(&started, member).is_empty(),
                "case {case:?}: expected a {member} in {started}"
            );
        }
        for member in ["expiresIn", "interval"] {
            assert!(
                started[member].as_i64().is_some_and(|value| value > 0),
                "case {case:?}: expected a positive {member} in {started}"
            );
        }

        // CreateToken on a device code nothing issued -> the modelled 400
        // InvalidGrantException, NOT a routing refusal.
        let (status, refused) = emulator
            .send(unsigned(
                &endpoint,
                "POST",
                "/token",
                &json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    "grantType": GRANT_TYPE_DEVICE_CODE,
                    "deviceCode": "a-device-code-nothing-issued",
                }),
            ))
            .await;
        assert_claimed(case, status, &refused);
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400, got {status} with {refused}"
        );
        assert!(
            error_shape(&refused).contains("InvalidGrantException"),
            "case {case:?}: expected InvalidGrantException, got {refused}"
        );

        // A registration the service refuses -> the modelled 400
        // InvalidClientMetadataException, so a negative case is answered by the
        // operation rather than by the router.
        let (status, refused) = emulator
            .send(unsigned(
                &endpoint,
                "POST",
                "/client/register",
                &json!({"clientName": "anonymous-client", "clientType": "confidential"}),
            ))
            .await;
        assert_claimed(case, status, &refused);
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400, got {status} with {refused}"
        );
        assert!(
            error_shape(&refused).contains("InvalidClientMetadataException"),
            "case {case:?}: expected InvalidClientMetadataException, got {refused}"
        );
    }
}

/// RULE (the other half): the modelled route is the WHOLE of the claim for a request
/// that names no service, so a nameless request whose method and URI this model binds no
/// route for is claimed by nobody — the router's `404 UnknownService` — rather than
/// answered by a service that does not serve it. The table varies what makes the request
/// miss: an unbound path, a bound path's parent, a bound path with a method it does not
/// bind, the root path, and a path that is a bound one with a suffix.
#[tokio::test]
async fn a_nameless_request_outside_the_modelled_routes_is_claimed_by_nobody() {
    let emulator = Emulator::new();
    // The routes this model binds are POST /client/register, POST /device_authorization,
    // POST /token and POST /token?aws_iam=t (smithy_query(query="digest")); every row
    // below is outside that set.
    let rows: [(&str, &str, &str); 6] = [
        ("a path no operation binds", "POST", "/authorize"),
        ("the register path's parent", "POST", "/client"),
        ("a bound path with a suffix", "POST", "/token/refresh"),
        ("the root path", "POST", "/"),
        ("GET on the token route", "GET", "/token"),
        ("DELETE on the register route", "DELETE", "/client/register"),
    ];

    for (case, method, path) in rows {
        for endpoint in endpoints() {
            // The host-style endpoint NAMES this service, so a miss there is the
            // service's own routing error, not the router's; this half is about the
            // requests that name nothing.
            if endpoint.host == Some("oidc.us-west-2.amazonaws.com") {
                continue;
            }
            let case = format!("{case} ({})", endpoint.case);
            let (status, body) = emulator
                .send(unsigned(&endpoint, method, path, &json!({})))
                .await;
            assert_unclaimed(&case, status, &body);
        }
    }
}

/// RULE (the third half): the nameless claim is a LAST resort, so a request that DOES
/// name a service is left to the service it names — this one claims it only when the name
/// is one of its own (the signing name a signed request carries, or the endpoint prefix a
/// host-addressed one resolves to). The table varies the name and where it comes from.
#[tokio::test]
async fn a_request_that_names_another_service_is_not_claimed_on_this_services_routes() {
    let emulator = Emulator::new();
    let minimal = json!({"clientName": "named-client", "clientType": "public"});

    // (case, the request, whether this service claims it)
    let rows: Vec<(&str, http::Request<bytes::Bytes>, bool)> = vec![
        (
            "a credential scope naming another service on this service's token route",
            signed_for("dynamodb", "POST", "/token", &minimal),
            false,
        ),
        (
            "a credential scope naming the SSO portal, a different endpoint",
            signed_for("awsssoportal", "POST", "/client/register", &minimal),
            false,
        ),
        (
            "an X-Amz-Target naming another service",
            http::Request::builder()
                .method("POST")
                .uri("/client/register")
                .header("host", format!("localhost:{LOCAL_PORT}"))
                .header("content-type", "application/json")
                .header("x-amz-target", "DynamoDB_20120810.GetItem")
                .body(bytes::Bytes::from(minimal.to_string()))
                .expect("the request builds"),
            false,
        ),
        (
            "another service's endpoint host",
            http::Request::builder()
                .method("POST")
                .uri("https://dynamodb.us-west-2.amazonaws.com/client/register")
                .header("content-type", "application/json")
                .body(bytes::Bytes::from(minimal.to_string()))
                .expect("the request builds"),
            false,
        ),
        (
            "a credential scope naming this service's own signing name",
            signed_for("sso-oauth", "POST", "/client/register", &minimal),
            true,
        ),
    ];

    for (case, request, claimed) in rows {
        let (status, body) = emulator.send(request).await;
        if claimed {
            assert_claimed(case, status, &body);
            assert_eq!(
                status, 200,
                "case {case:?}: expected RegisterClient to answer 200, got {status} with {body}"
            );
        } else {
            assert_unclaimed(case, status, &body);
        }
    }
}

/// The AWS error code the router's authenticator refuses an unverifiable request with.
const ACCESS_DENIED: &str = "AccessDenied";

/// One emulator whose router VERIFIES request signatures, as the production router does
/// — the other suites run with verification off, which cannot tell whether an unsigned
/// call is served because the model says so or because nothing was checked.
async fn served_with_authentication_on(
    request: http::Request<bytes::Bytes>,
) -> (u16, Value, MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(true);
    router.register_service(ssooidc::EmulatedService::new(Arc::clone(&engine)));
    let (status, body) = dispatch(&router, request).await;
    (status, body, router, engine)
}

/// Dispatch one request through `router` and read the status and the JSON body.
async fn dispatch(
    router: &MultiServiceRouter,
    request: http::Request<bytes::Bytes>,
) -> (u16, Value) {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "the response body is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, body)
}

/// RULE: the claim decides only WHICH service answers; whether a claimed request needs a
/// signature stays with the router's authenticator, which reads the modelled operation's
/// own auth schemes. So with verification ON, the three operations the model serves
/// anonymously are served unsigned from end to end, and `CreateTokenWithIAM` — the one
/// operation of this service the model leaves signed — is refused by the AUTHENTICATOR on
/// the same nameless endpoint, never by the router's routing refusal.
#[tokio::test]
async fn authentication_decides_the_signature_and_the_claim_decides_the_service() {
    let endpoint = Endpoint {
        case: "a loopback Host header on an origin-form URI",
        host: Some("localhost:9297"),
        authority: None,
    };

    // RegisterClient, unsigned, with verification on: served, because the model gives it
    // an empty auth list.
    let (status, registered, router, _engine) = served_with_authentication_on(unsigned(
        &endpoint,
        "POST",
        "/client/register",
        &json!({
            "clientName": "authenticating-router",
            "clientType": "public",
            "grantTypes": [GRANT_TYPE_DEVICE_CODE],
        }),
    ))
    .await;
    assert_claimed("RegisterClient with verification on", status, &registered);
    assert_eq!(
        status, 200,
        "expected an unsigned RegisterClient to be served with verification on, got {status} \
         with {registered}"
    );
    let client_id = text(&registered, "clientId");
    let client_secret = text(&registered, "clientSecret");

    // StartDeviceAuthorization, unsigned: served too, and from the state the register
    // call persisted in this same emulator.
    let (status, started) = dispatch(
        &router,
        unsigned(
            &endpoint,
            "POST",
            "/device_authorization",
            &json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "startUrl": START_URL,
            }),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected an unsigned StartDeviceAuthorization to be served with verification on, \
         got {status} with {started}"
    );

    // CreateToken, unsigned: served, and answered by the handler — the modelled error for
    // a grant nothing issued, not a refusal from the boundary.
    let (status, refused) = dispatch(
        &router,
        unsigned(
            &endpoint,
            "POST",
            "/token",
            &json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "a-device-code-nothing-issued",
            }),
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected the modelled 400 for an unissued grant, got {status} with {refused}"
    );
    assert!(
        error_shape(&refused).contains("InvalidGrantException"),
        "expected InvalidGrantException, got {refused}"
    );

    // CreateTokenWithIAM on the same nameless endpoint: the route is claimed, so the
    // refusal comes from the authenticator the model's `sigv4` scheme requires — never
    // the router's `UnknownService`.
    let (status, denied) = dispatch(
        &router,
        unsigned(&endpoint, "POST", "/token?aws_iam=t", &json!({"grantType": "refresh_token"})),
    )
    .await;
    assert_claimed("CreateTokenWithIAM unsigned", status, &denied);
    assert!(
        error_shape(&denied).contains(ACCESS_DENIED),
        "expected the authenticator's {ACCESS_DENIED} for an unsigned CreateTokenWithIAM, got \
         {status} with {denied}"
    );
}

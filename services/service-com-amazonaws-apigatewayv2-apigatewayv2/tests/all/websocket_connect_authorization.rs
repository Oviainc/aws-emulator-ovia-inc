// SPDX-License-Identifier: CC0-1.0

//! The AUTHORIZATION of a WebSocket api's connection request, over the wire: what a
//! client's RFC 6455 opening handshake against a deployed stage is answered with when the
//! `$connect` route that serves it is fronted by an `Authorizer` of the api.
//!
//! The rule these cases assert, one level above the recorded case (one WebSocket api, one
//! `REQUEST` authorizer over `route.request.querystring.token`, a handshake expected to be
//! refused before the `$connect` integration runs):
//!
//! > A route's authorization is applied to the request BEFORE anything that route fronts.
//! > When the route's `AuthorizationType` names an `Authorizer` of the api, every entry of
//! > that authorizer's `IdentitySource` must be present in the request, not null and
//! > non-empty; only then is the authorizer's Lambda invoked, and otherwise the request is
//! > refused `401` `{"message":"Unauthorized"}` with the authorizer never invoked. A route
//! > may only name an authorizer its api holds, which its writers enforce, so the
//! > authorization step always has a configuration to decide from; and a route whose
//! > authorization is not an authorizer's (`NONE`, `AWS_IAM`) is not refused by this step.
//!
//! That is `Authorizer$IdentitySource` verbatim: "These parameters are used to perform
//! runtime validation for Lambda-based authorizers by verifying all of the
//! identity-related request parameters are present in the request, not null, and
//! non-empty. Only when this is true does the authorizer invoke the authorizer Lambda
//! function. Otherwise, it returns a 401 Unauthorized response without calling the Lambda
//! function."
//! (`smithy_query(query="doc",shape="CreateAuthorizerRequest",member="IdentitySource")`).
//!
//! What a handshake whose identity sources ARE satisfied gets is the answer the CONNECTION
//! REQUEST's own integration gives: the authorizer's own ALLOW/DENY is the answer of the
//! Lambda its `AuthorizerUri` names, which no deployment package runs here, and the `MOCK`
//! integration these rows' `$connect` route fronts reaches no backend, so it gives the
//! connection request no decision and the gateway answers `500`
//! (`websocket_transport_data_plane` owns that rule, over the api's own socket). So a `500`
//! in these rows means "the authorization step let this handshake through", and is
//! asserted for exactly that.
//!
//! The rows span what the rule generalises over: which request part the identity source
//! names (a header, a query-string parameter, a stage variable, a context parameter), the
//! number of entries (none, one, two), the spelling of the expression (`route.request.…`
//! and the `$request.…` form the same member documents), the case of the name, its
//! percent-encoding, a value that is present but empty or whitespace, and the route's
//! `AuthorizationType`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator};
use crate::websocket_handshake_data_plane::{message, send, Answer, CLIENT_KEY};

/// The status a request whose authorizer was not invoked is refused with; see the module
/// docs' quotation of `Authorizer$IdentitySource`.
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The status a handshake the authorization step LET THROUGH is answered with, and the
/// sentence it carries: past the authorization the connection request is served by the
/// `$connect` route's own INTEGRATION, and a `MOCK` integration — the one these rows' route
/// fronts — reaches no backend, so it gives the connection request no decision and the
/// gateway answers `500`. What these rows assert is that the answer is NOT the `401` the
/// authorization step itself refuses with.
const LET_THROUGH: u16 = 500;
const LET_THROUGH_MESSAGE: &str = "Internal server error";

/// The status a successful upgrade would carry. Asserted ABSENT everywhere: a refusal
/// before the integration is what AWS answers the handshake itself with.
const SWITCHING_PROTOCOLS: u16 = 101;

/// A well-formed Lambda authorizer URI, the form `AuthorizerUri` documents: "For REQUEST
/// authorizers, this must be a well-formed Lambda function URI, for example,
/// arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{function-arn}/invocations".
fn lambda_uri(function: &str) -> String {
    format!(
        "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/\
         arn:aws:lambda:eu-west-1:123456789012:function:{function}/invocations"
    )
}

/// How the `$connect` route names the authorizer that decides its requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteAuthorizer {
    /// The `AuthorizerId` of the authorizer created on the api.
    Created,
    /// No authorizer at all: the route's authorization is its `AuthorizationType` alone.
    None,
}

/// An api created, authorized, routed and deployed to one stage.
struct Deployed {
    api_id: String,
    /// The host of the api's own `ApiEndpoint`, read off the create response, so the
    /// handshake addresses the endpoint the service reported.
    endpoint_host: String,
    stage: String,
    /// The `AuthorizerId` the api minted.
    authorizer_id: String,
}

/// Create a WEBSOCKET api with one `REQUEST` authorizer over `identity_sources`, a
/// `$connect` route whose authorization is `authorization_type`/`authorizer`, a stage
/// carrying `stage_variables`, and a deployment released to that stage.
///
/// Every create is read back through its own `Get…` before the handshake is sent, so what
/// the data plane then answers from is the record a later caller reads and not an echo of
/// the request.
async fn deploy(
    emulator: &MultiServiceRouter,
    case: &str,
    identity_sources: &[&str],
    authorization_type: &str,
    authorizer: RouteAuthorizer,
    stage_variables: &[(&str, &str)],
) -> Deployed {
    let (status, created) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "socket-api",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        })),
    )
    .await;
    assert_eq!(status, 201, "case {case:?}: CreateApi must answer 201, got {created}");
    let api_id = as_str(&created, "apiId");
    // These cases drive the handshake at the api's own `execute-api` host, which this
    // emulator's FRONT DOOR claims; the api's own socket is announced in its `ApiEndpoint`
    // and is driven over a real connection in `websocket_transport_data_plane`.
    let endpoint_host = format!(
        "{api_id}.execute-api.{}.amazonaws.com",
        crate::api_lifecycle::REGION
    );

    let (status, authorizer_body) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "connect-auth",
            "authorizerType": "REQUEST",
            "authorizerUri": lambda_uri("connect-auth"),
            "identitySource": identity_sources
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateAuthorizer must answer 201, got {authorizer_body}"
    );
    let authorizer_id = as_str(&authorizer_body, "authorizerId");
    assert!(
        !authorizer_id.is_empty(),
        "case {case:?}: CreateAuthorizer must answer an AuthorizerId, got {authorizer_body}"
    );
    let (status, fetched) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/authorizers/{authorizer_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: GetAuthorizer must answer 200, got {fetched}");
    assert_eq!(
        fetched.get("identitySource"),
        Some(&json!(identity_sources)),
        "case {case:?}: the stored authorizer must report the identity sources it was \
         created with, got {fetched}"
    );

    let mut route = json!({ "routeKey": "$connect", "authorizationType": authorization_type });
    match authorizer {
        RouteAuthorizer::Created => route["authorizerId"] = json!(authorizer_id),
        RouteAuthorizer::None => {}
    }
    let (status, created_route) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(route),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute must answer 201, got {created_route}"
    );
    let route_id = as_str(&created_route, "routeId");
    let (status, fetched_route) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: GetRoute must answer 200, got {fetched_route}");
    assert_eq!(
        as_str(&fetched_route, "authorizationType"),
        authorization_type,
        "case {case:?}: the stored route must report its AuthorizationType, got \
         {fetched_route}"
    );

    let variables: Value = stage_variables
        .iter()
        .map(|(name, value)| ((*name).to_string(), json!(value)))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let stage = "prod";
    let (status, created_stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": stage, "stageVariables": variables })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {created_stage}"
    );

    let (status, deployment) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(json!({ "stageName": stage })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateDeployment must answer 201, got {deployment}"
    );
    let (status, fetched_stage) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages/{stage}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetStage must answer 200, got {fetched_stage}"
    );
    assert!(
        !as_str(&fetched_stage, "deploymentId").is_empty(),
        "case {case:?}: the stage must carry the released deployment, got {fetched_stage}"
    );

    Deployed {
        api_id,
        endpoint_host,
        stage: stage.to_string(),
        authorizer_id,
    }
}

/// Send the opening handshake to `deployed`'s stage, with `query` appended to the path and
/// `headers` added to the three fields of RFC 6455 §4.1.
async fn handshake(
    emulator: &MultiServiceRouter,
    deployed: &Deployed,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut fields: Vec<(&str, &str)> = vec![
        ("connection", "Upgrade"),
        ("upgrade", "websocket"),
        ("sec-websocket-key", CLIENT_KEY),
        ("sec-websocket-version", "13"),
    ];
    fields.extend_from_slice(headers);
    let path = if query.is_empty() {
        format!("/{}", deployed.stage)
    } else {
        format!("/{}?{query}", deployed.stage)
    };
    send(emulator, &deployed.endpoint_host, "GET", &path, &fields).await
}

/// Assert the handshake was refused rather than upgraded, whatever the refusal is.
fn assert_not_upgraded(case: &str, answer: &Answer) {
    assert_ne!(
        answer.status, SWITCHING_PROTOCOLS,
        "case {case:?}: a 101 would promise a socket this emulator cannot carry, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.accept, None,
        "case {case:?}: no upgrade happened, so no Sec-WebSocket-Accept is sent, got {:?}",
        answer.accept
    );
}

/// One row: the authorizer's identity sources, the stage's variables, what the handshake
/// presents, and the status the authorization step owes it.
struct Row {
    case: &'static str,
    identity_sources: &'static [&'static str],
    stage_variables: &'static [(&'static str, &'static str)],
    /// The raw query string the handshake carries.
    query: &'static str,
    /// The headers the handshake carries beyond the RFC 6455 fields.
    headers: &'static [(&'static str, &'static str)],
    /// [`UNAUTHORIZED`] when an identity source is absent, [`NOT_IMPLEMENTED`] when the
    /// authorization step let the handshake through to the transport refusal.
    expected: u16,
}

const ROWS: &[Row] = &[
    Row {
        case: "the recorded case: one query-string identity source the handshake carries",
        identity_sources: &["route.request.querystring.token"],
        stage_variables: &[],
        query: "token=t0ken",
        headers: &[],
        expected: LET_THROUGH,
    },
    Row {
        case: "the same authorizer, a handshake carrying no query string at all",
        identity_sources: &["route.request.querystring.token"],
        stage_variables: &[],
        query: "",
        headers: &[],
        expected: UNAUTHORIZED,
    },
    Row {
        case: "the parameter present but empty, which the rule counts as absent",
        identity_sources: &["route.request.querystring.token"],
        stage_variables: &[],
        query: "token=",
        headers: &[],
        expected: UNAUTHORIZED,
    },
    Row {
        case: "a query-string parameter name differing only in case, which is a different \
               parameter",
        identity_sources: &["route.request.querystring.token"],
        stage_variables: &[],
        query: "Token=t0ken",
        headers: &[],
        expected: UNAUTHORIZED,
    },
    Row {
        case: "a header identity source the handshake carries, matched case-insensitively \
               as a header name is",
        identity_sources: &["route.request.header.Auth"],
        stage_variables: &[],
        query: "",
        headers: &[("auth", "t0ken")],
        expected: LET_THROUGH,
    },
    Row {
        case: "a header identity source the handshake does not carry",
        identity_sources: &["route.request.header.Auth"],
        stage_variables: &[],
        query: "",
        headers: &[("other", "t0ken")],
        expected: UNAUTHORIZED,
    },
    Row {
        case: "a header carrying only whitespace, which is not a non-empty value",
        identity_sources: &["route.request.header.Auth"],
        stage_variables: &[],
        query: "",
        headers: &[("auth", "   ")],
        expected: UNAUTHORIZED,
    },
    Row {
        case: "two identity sources, both presented",
        identity_sources: &[
            "route.request.header.Auth",
            "route.request.querystring.token",
        ],
        stage_variables: &[],
        query: "token=t0ken&other=x",
        headers: &[("auth", "t0ken")],
        expected: LET_THROUGH,
    },
    Row {
        case: "two identity sources, only one presented: ALL of them must be",
        identity_sources: &[
            "route.request.header.Auth",
            "route.request.querystring.token",
        ],
        stage_variables: &[],
        query: "token=t0ken",
        headers: &[],
        expected: UNAUTHORIZED,
    },
    Row {
        case: "no identity source at all, which a REQUEST authorizer may have: nothing to \
               validate, so nothing is refused",
        identity_sources: &[],
        stage_variables: &[],
        query: "",
        headers: &[],
        expected: LET_THROUGH,
    },
    Row {
        case: "a stage variable the stage holds",
        identity_sources: &["stagevariables.tenant"],
        stage_variables: &[("tenant", "acme")],
        query: "",
        headers: &[],
        expected: LET_THROUGH,
    },
    Row {
        case: "a stage variable the stage does not hold",
        identity_sources: &["stagevariables.tenant"],
        stage_variables: &[("other", "acme")],
        query: "",
        headers: &[],
        expected: UNAUTHORIZED,
    },
    Row {
        case: "the $-prefixed spellings the same member documents, satisfied",
        identity_sources: &["$request.header.Auth", "$request.querystring.token"],
        stage_variables: &[],
        query: "token=t0ken",
        headers: &[("auth", "t0ken")],
        expected: LET_THROUGH,
    },
    Row {
        case: "the $-prefixed spelling of a header the handshake does not carry",
        identity_sources: &["$request.header.Auth"],
        stage_variables: &[],
        query: "",
        headers: &[],
        expected: UNAUTHORIZED,
    },
    Row {
        case: "a query parameter whose name the handshake percent-encoded, which is the \
               same parameter decoded",
        identity_sources: &["route.request.querystring.my token"],
        stage_variables: &[],
        query: "my%20token=t0ken",
        headers: &[],
        expected: LET_THROUGH,
    },
    Row {
        case: "a context parameter, which the service computes for every call and no \
               handshake can be missing",
        identity_sources: &["context.identity.sourceIp"],
        stage_variables: &[],
        query: "",
        headers: &[],
        expected: LET_THROUGH,
    },
    Row {
        case: "an expression naming a request part this data plane cannot read, which is \
               no parameter it can find present",
        identity_sources: &["route.request.body.action"],
        stage_variables: &[],
        query: "",
        headers: &[],
        expected: UNAUTHORIZED,
    },
];

#[tokio::test]
async fn an_absent_identity_source_refuses_the_connection_request_before_its_route_runs() {
    for row in ROWS {
        let emulator = emulator();
        let deployed = deploy(
            &emulator,
            row.case,
            row.identity_sources,
            "CUSTOM",
            RouteAuthorizer::Created,
            row.stage_variables,
        )
        .await;
        let answer = handshake(&emulator, &deployed, row.query, row.headers).await;
        assert_not_upgraded(row.case, &answer);
        assert_eq!(
            answer.status, row.expected,
            "case {:?}: expected {} for identity sources {:?}, got {}",
            row.case,
            row.expected,
            row.identity_sources,
            answer.summary()
        );
        if row.expected == UNAUTHORIZED {
            assert_eq!(
                message(&answer),
                UNAUTHORIZED_MESSAGE,
                "case {:?}: expected the {UNAUTHORIZED_MESSAGE:?} body the refusal carries, \
                 got {}",
                row.case,
                answer.summary()
            );
            assert_ne!(
                answer.status, LET_THROUGH,
                "case {:?}: the authorizer was never invoked, so the connection request's \
                 own integration was never reached, got {}",
                row.case,
                answer.summary()
            );
        } else {
            assert_eq!(
                message(&answer),
                LET_THROUGH_MESSAGE,
                "case {:?}: a handshake the authorization step let through is answered by \
                 the connection request's integration, got {}",
                row.case,
                answer.summary()
            );
        }
    }
}

#[tokio::test]
async fn a_route_whose_authorization_is_not_an_authorizers_is_not_refused_by_this_step() {
    // Each row carries an authorizer ON THE API whose identity source no handshake below
    // presents, so a `401` would mean the step was applied to a route that does not name
    // it. `NONE` is "open access" and `AWS_IAM` is decided by an IAM policy this emulator
    // holds no records of, so neither is this step's to refuse.
    const CASES: &[(&str, &str, RouteAuthorizer)] = &[
        ("open access, with no authorizer named", "NONE", RouteAuthorizer::None),
        (
            "open access, even though the route still names the authorizer",
            "NONE",
            RouteAuthorizer::Created,
        ),
        ("IAM authorization", "AWS_IAM", RouteAuthorizer::None),
    ];

    for (case, authorization_type, authorizer) in CASES {
        let emulator = emulator();
        let deployed = deploy(
            &emulator,
            case,
            &["route.request.querystring.token"],
            authorization_type,
            *authorizer,
            &[],
        )
        .await;
        let answer = handshake(&emulator, &deployed, "", &[]).await;
        assert_not_upgraded(case, &answer);
        assert_eq!(
            answer.status, LET_THROUGH,
            "case {case:?}: expected {LET_THROUGH} for an AuthorizationType of \
             {authorization_type}, got {}",
            answer.summary()
        );
    }
}

#[tokio::test]
async fn a_route_must_name_an_authorizer_its_api_holds() {
    // The 500 the authorization step answers for a route whose `AuthorizerId` resolves to
    // no authorizer is not reachable from the wire, because the WRITERS of a route refuse
    // that association: a `CUSTOM` route may only name an authorizer its api holds. This
    // pins the invariant the data plane relies on, so if a writer ever stops enforcing it
    // the gap shows up here rather than as a 500 a client cannot explain.
    let emulator = emulator();
    let (status, created) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "socket-api",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateApi must answer 201, got {created}");
    let api_id = as_str(&created, "apiId");
    // Lowercase alphanumerics, the alphabet this service mints an id from, that no
    // authorizer of this api carries.
    let (status, refused) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "$connect",
            "authorizationType": "CUSTOM",
            "authorizerId": "zzzz999zzz"
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "a route may not name an authorizer its api does not hold, got {status} {refused}"
    );
    let (status, routes) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
    assert_eq!(status, 200, "GetRoutes must answer 200, got {routes}");
    assert_eq!(
        routes.get("items"),
        Some(&json!([])),
        "the refused create must have stored no route, got {routes}"
    );
}

#[tokio::test]
async fn an_authorizer_holds_a_bounded_number_of_identity_sources() {
    // The authorization step reads the WHOLE identity-source list on every handshake, so
    // the list one create stores is what bounds that per-request work. `IdentitySourceList`
    // carries no modelled length constraint, so the bound is the service's own — far above
    // the one to three sources a real authorizer names, refusing only an absurd list —
    // answered with the modelled 429 ("A limit has been exceeded").
    const MAX_IDENTITY_SOURCES: usize = 100;
    let emulator = emulator();
    let (status, created) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "socket-api",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateApi must answer 201, got {created}");
    let api_id = as_str(&created, "apiId");

    let sources = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| format!("route.request.header.Auth{index}"))
            .collect()
    };
    for (case, count, expected) in [
        ("one identity source", 1usize, 201u16),
        ("the bound itself", MAX_IDENTITY_SOURCES, 201),
        ("one past the bound", MAX_IDENTITY_SOURCES + 1, 429),
    ] {
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/authorizers"),
            Some(json!({
                "name": format!("auth-{count}"),
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("connect-auth"),
                "identitySource": sources(count)
            })),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected} for {count} identity sources, got {status} \
             {body}"
        );
        if expected == 201 {
            assert_eq!(
                body.get("identitySource"),
                Some(&json!(sources(count))),
                "case {case:?}: the stored authorizer must report every identity source, \
                 got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_connection_request_changes_nothing_a_later_caller_sees() {
    let emulator = emulator();
    let deployed = deploy(
        &emulator,
        "a read must not change what a later caller sees",
        &["route.request.querystring.token"],
        "CUSTOM",
        RouteAuthorizer::Created,
        &[("tenant", "acme")],
    )
    .await;
    let authorizer_uri = format!(
        "/v2/apis/{}/authorizers/{}",
        deployed.api_id, deployed.authorizer_id
    );
    let stage_uri = format!("/v2/apis/{}/stages/{}", deployed.api_id, deployed.stage);
    let (status, authorizer_before) = call(&emulator, "GET", &authorizer_uri, None).await;
    assert_eq!(status, 200, "GetAuthorizer must answer 200, got {authorizer_before}");
    let (status, stage_before) = call(&emulator, "GET", &stage_uri, None).await;
    assert_eq!(status, 200, "GetStage must answer 200, got {stage_before}");

    let first = handshake(&emulator, &deployed, "", &[]).await;
    let second = handshake(&emulator, &deployed, "", &[]).await;
    assert_eq!(
        (first.status, message(&first)),
        (UNAUTHORIZED, UNAUTHORIZED_MESSAGE.to_string()),
        "the first handshake presents no identity source, got {}",
        first.summary()
    );
    assert_eq!(
        (first.status, message(&first)),
        (second.status, message(&second)),
        "two identical handshakes must be answered identically, got {} then {}",
        first.summary(),
        second.summary()
    );

    let (status, authorizer_after) = call(&emulator, "GET", &authorizer_uri, None).await;
    assert_eq!(status, 200, "GetAuthorizer must answer 200, got {authorizer_after}");
    assert_eq!(
        authorizer_after, authorizer_before,
        "the refused handshake changed the authorizer record a later caller reads"
    );
    let (status, stage_after) = call(&emulator, "GET", &stage_uri, None).await;
    assert_eq!(status, 200, "GetStage must answer 200, got {stage_after}");
    assert_eq!(
        stage_after, stage_before,
        "the refused handshake changed the stage record a later caller reads"
    );
}

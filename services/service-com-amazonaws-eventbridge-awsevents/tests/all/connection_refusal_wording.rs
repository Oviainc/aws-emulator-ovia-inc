// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal of a connection call, end to end over the
//! wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`)
//! through the real `MultiServiceRouter`.
//!
//! The RULES asserted here:
//!
//! * every refusal of a connection call opens with the clause naming the CALL
//!   that could not be served — `Failed to <verb> the connection(s).` for the
//!   verb of the operation that ran — and only then states the reason, so a
//!   reason is read against the call that failed;
//! * a refusal of a submitted value names the member whose value was rejected —
//!   the LEAF member, not the block that carries it — and an endpoint member
//!   that has to be an HTTPS URL is refused with `Parameter <Member> is not
//!   valid. Reason: Endpoint '<value as submitted>' is invalid, please provide a
//!   valid HTTPS endpoint URL.`, the value quoted exactly as the request sent
//!   it;
//! * a refused create writes nothing: the name it carried still resolves to no
//!   connection, on a read and in a listing.
//!
//! Each table spans the class rather than one case: the endpoint rows differ
//! along the scheme (plain http, another scheme entirely, a scheme-shaped
//! prefix, no scheme at all), along the shape of the value (host and path, bare
//! host, a value carrying its own quote characters) and along the WRITER the
//! endpoint reached the service through (`CreateConnection` and
//! `UpdateConnection`); the wording rows span every operation of the connection
//! resource and the refusal kinds each of them has.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-east-2";

/// An authorization endpoint of the accepted form, used as the positive control
/// and as the endpoint of the connection the update rows start from. This
/// emulator serves nothing at it, which is not what these rows are about: the
/// request is accepted and the connection's later state is
/// `connection_authorization`'s subject.
const HTTPS_ENDPOINT: &str = "https://authorization.example.com/oauth/token";

/// The emulator: one router carrying only this service over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One `awsJson1_1` call to EventBridge over the router.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a
/// success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` an error body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The clause the rule under test says every refusal of a connection call opens
/// with, composed here from the call's own verb rather than imported from the
/// implementation.
fn expected_prefix(verb: &str) -> String {
    format!("Failed to {verb} the connection(s).")
}

/// What the rule says a refusal of a connection call reads in full: the clause
/// naming the call, then the reason.
fn expected_refusal(verb: &str, reason: &str) -> String {
    format!("{} {reason}", expected_prefix(verb))
}

/// What the rule says a rejected HTTPS endpoint member reads: the member whose
/// value was rejected and the value as submitted, in single quotes.
fn expected_endpoint_reason(member: &str, endpoint: &str) -> String {
    format!(
        "Parameter {member} is not valid. Reason: Endpoint '{endpoint}' is invalid, please \
         provide a valid HTTPS endpoint URL."
    )
}

/// The `AuthParameters` of an OAuth connection whose authorization endpoint is
/// `endpoint`.
fn oauth_parameters(endpoint: &str) -> Value {
    json!({
        "OAuthParameters": {
            "ClientParameters": { "ClientID": "the-client", "ClientSecret": "sekrit" },
            "AuthorizationEndpoint": endpoint,
            "HttpMethod": "POST",
        },
    })
}

/// An `API_KEY` connection called `name`, whose credentials are themselves the
/// authorization — the connection the update rows start from.
async fn given_api_key_connection(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = events(
        router,
        "CreateConnection",
        &json!({
            "Name": name,
            "AuthorizationType": "API_KEY",
            "AuthParameters": {
                "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit" },
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConnection answered {status} {body}"
    );
}

/// Every connection name the store holds, in the order a listing reports them.
async fn listed_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, listed) = events(router, "ListConnections", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConnections answered {status} {listed}"
    );
    listed["Connections"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|connection| connection["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A value that is not an HTTPS URL is refused by BOTH writers of a
/// connection's authorization endpoint, naming the member whose value was
/// rejected, quoting the value exactly as submitted, and opening with the
/// clause of the call that failed — and the refused create leaves no connection
/// behind under the name it carried.
#[tokio::test]
async fn a_non_https_authorization_endpoint_is_refused_naming_the_member_and_the_value() {
    // (case, the AuthorizationEndpoint the request carries)
    let rows: [(&str, &str); 7] = [
        ("a plain http URL", "http://authorization.example.com/oauth/token"),
        ("a plain http URL in upper case", "HTTP://AUTHORIZATION.EXAMPLE.COM/TOKEN"),
        ("another scheme entirely", "ftp://authorization.example.com/token"),
        ("a scheme-shaped prefix that is not https", "httpsx://authorization.example.com/token"),
        ("the scheme without its separator", "https:authorization.example.com/token"),
        ("a bare host and path, no scheme at all", "authorization.example.com/token"),
        // The value is echoed back inside single quotes, so one carrying its own
        // quote characters is still echoed exactly as submitted.
        ("a value carrying its own quotes", "'http://quoted.example.com/token'"),
    ];

    for (case, endpoint) in rows {
        let router = emulator();
        let kept = "already-here";
        given_api_key_connection(&router, case, kept).await;

        let refused = "the-connection";
        let (status, body) = events(
            &router,
            "CreateConnection",
            &json!({
                "Name": refused,
                "AuthorizationType": "OAUTH_CLIENT_CREDENTIALS",
                "AuthParameters": oauth_parameters(endpoint),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an endpoint that is not https must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let expected = expected_refusal(
            "create",
            &expected_endpoint_reason("AuthorizationEndpoint", endpoint),
        );
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&body)
        );

        // Nothing was created under that name: the read and the listing agree
        // with the refusal.
        let (status, read) = events(&router, "DescribeConnection", &json!({ "Name": refused })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create leaves no connection to read, got {status} {read}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {read}"
        );
        let expected = expected_refusal(
            "describe",
            &format!("Connection '{refused}' does not exist."),
        );
        assert_eq!(
            message(&read),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&read)
        );
        assert_eq!(
            listed_names(&router, case).await,
            vec![kept.to_string()],
            "case {case:?}: only the accepted connection is listed"
        );

        // The same value through the OTHER writer of a connection's
        // authorization endpoint, refused in the words of ITS own call.
        let (status, updated) = events(
            &router,
            "UpdateConnection",
            &json!({
                "Name": kept,
                "AuthorizationType": "OAUTH_CLIENT_CREDENTIALS",
                "AuthParameters": oauth_parameters(endpoint),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: UpdateConnection must refuse it too, got {status} {updated}"
        );
        assert_eq!(
            error_code(&updated),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {updated}"
        );
        let expected = expected_refusal(
            "update",
            &expected_endpoint_reason("AuthorizationEndpoint", endpoint),
        );
        assert_eq!(
            message(&updated),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&updated)
        );
    }
}

/// An HTTPS authorization endpoint is ACCEPTED by both writers, whatever the
/// case of its scheme: the rule above refuses a value that is not an HTTPS URL,
/// and nothing else.
#[tokio::test]
async fn an_https_authorization_endpoint_is_accepted_by_both_writers() {
    // (case, the AuthorizationEndpoint the request carries)
    let rows: [(&str, &str); 3] = [
        ("an https URL", HTTPS_ENDPOINT),
        ("an https URL whose scheme is upper case", "HTTPS://authorization.example.com/token"),
        ("an https URL with a port and a query", "https://authorization.example.com:8443/t?a=1"),
    ];

    for (case, endpoint) in rows {
        let router = emulator();
        let name = "the-connection";
        let (status, created) = events(
            &router,
            "CreateConnection",
            &json!({
                "Name": name,
                "AuthorizationType": "OAUTH_CLIENT_CREDENTIALS",
                "AuthParameters": oauth_parameters(endpoint),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an https endpoint must be accepted, got {status} {created}"
        );
        // The accepted endpoint is what a read reports back, as submitted.
        let (status, read) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeConnection answered {status} {read}"
        );
        assert_eq!(
            read["AuthParameters"]["OAuthParameters"]["AuthorizationEndpoint"],
            json!(endpoint),
            "case {case:?}: expected the endpoint as submitted, got {read}"
        );

        let (status, updated) = events(
            &router,
            "UpdateConnection",
            &json!({
                "Name": name,
                "AuthorizationType": "OAUTH_CLIENT_CREDENTIALS",
                "AuthParameters": oauth_parameters(endpoint),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateConnection must accept it too, got {status} {updated}"
        );
    }
}

/// One wording row: the case, the operation, its request, the verb its refusal
/// has to name, the modelled wire code and the reason the clause is followed by.
type WordingRow = (&'static str, &'static str, Value, &'static str, &'static str, String);

/// Every refusal of a connection call names the call that failed first, whatever
/// the operation and whatever the refusal is about.
#[tokio::test]
async fn every_connection_refusal_names_the_call_that_failed() {
    let router = emulator();
    let taken = "taken";
    given_api_key_connection(&router, "the connection the rows below reuse", taken).await;
    let absent = "absent";

    let rows: Vec<WordingRow> = vec![
        (
            "a second create under one name",
            "CreateConnection",
            json!({
                "Name": taken,
                "AuthorizationType": "API_KEY",
                "AuthParameters": {
                    "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
                },
            }),
            "create",
            "ResourceAlreadyExistsException",
            format!("A connection with name '{taken}' already exists."),
        ),
        (
            // The framework's own boundary validator refuses a value outside a
            // modelled ENUM before any handler runs, in AWS's own wording for
            // that class, so the row here is a refusal the HANDLER makes: the
            // parameters name no authorization type at all.
            "a create whose parameters name no authorization type",
            "CreateConnection",
            json!({
                "Name": "parameterless",
                "AuthorizationType": "BASIC",
                "AuthParameters": {},
            }),
            "create",
            "ValidationException",
            "Parameter AuthParameters is not valid. Reason: The value for AuthParameters must \
             include exactly one authorization type."
                .to_string(),
        ),
        (
            "a create carrying the parameters of another type instead of its own",
            "CreateConnection",
            json!({
                "Name": "mismatched",
                "AuthorizationType": "API_KEY",
                "AuthParameters": {
                    "BasicAuthParameters": { "Username": "u", "Password": "p" },
                },
            }),
            "create",
            "ValidationException",
            "Parameter AuthParameters is not valid. Reason: ApiKeyAuthParameters is required \
             for AuthorizationType API_KEY."
                .to_string(),
        ),
        (
            "a create carrying the parameters of another type beside its own",
            "CreateConnection",
            json!({
                "Name": "doubled",
                "AuthorizationType": "BASIC",
                "AuthParameters": {
                    "BasicAuthParameters": { "Username": "u", "Password": "p" },
                    "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
                },
            }),
            "create",
            "ValidationException",
            "Parameter AuthParameters is not valid. Reason: The value for AuthParameters must \
             include exactly one authorization type."
                .to_string(),
        ),
        (
            "a read of a connection that does not exist",
            "DescribeConnection",
            json!({ "Name": absent }),
            "describe",
            "ResourceNotFoundException",
            format!("Connection '{absent}' does not exist."),
        ),
        (
            "an update of a connection that does not exist",
            "UpdateConnection",
            json!({ "Name": absent }),
            "update",
            "ResourceNotFoundException",
            format!("Connection '{absent}' does not exist."),
        ),
        (
            "a deauthorize of a connection that does not exist",
            "DeauthorizeConnection",
            json!({ "Name": absent }),
            "deauthorize",
            "ResourceNotFoundException",
            format!("Connection '{absent}' does not exist."),
        ),
        (
            "a delete of a connection that does not exist",
            "DeleteConnection",
            json!({ "Name": absent }),
            "delete",
            "ResourceNotFoundException",
            format!("Connection '{absent}' does not exist."),
        ),
    ];

    for (case, operation, request, verb, expected_code, reason) in &rows {
        let (status, body) = events(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let expected = expected_refusal(verb, reason);
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&body)
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! Every route of this service whose model binds a `@required` member to the QUERY string,
//! driven end to end with that member left out.
//!
//! RULE. A request whose method and path address a modelled route of this service is served
//! by the operation the model binds to that route, so a `@required` member the request
//! carries no value for is this service's client-input refusal (`BadRequestException`,
//! HTTP 400) stating the absent member — never a report that the operation is not
//! implemented. The REST router's required-binding check exists to pick BETWEEN operations
//! overloaded on one method + URI template; where a method + URI template names exactly one
//! operation it cannot un-name it.
//!
//! The rows are DERIVED FROM THIS SERVICE'S OWN MODEL rather than written out: the suite
//! walks the model for every operation whose input binds a `@required` member to the query
//! string, so the class is covered whole and an operation the model adds to it is covered
//! the day it arrives — which is what keeps the service's routing seam
//! (`src/service.rs`, the operations it holds for exactly this case) from drifting behind
//! the model. The rows differ along what the rule generalises over: the HTTP method, the
//! member's type (a string and a list), the route's shape, and the parameter absent, then
//! supplied.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, with the response read back as
//! JSON.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and a region. No authenticator is
/// registered, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-1";

/// This service's Smithy model, so the rows below are the model's own rather than a copy of
/// it that can drift.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-kafkaconnect-kafkaconnect.json"
));

/// The operations this model binds a `@required` member to the query string on, as
/// `smithy_query(query="operation",operation="UpdateConnector,UntagResource")` reports them
/// (`UpdateConnector$currentVersion`, `UntagResource$tagKeys`). Asserted to be among what
/// the walk below finds, so a walk that silently stopped finding routes cannot pass.
const KNOWN_OPERATIONS: &[&str] = &["UntagResource", "UpdateConnector"];

/// The error code this service declares for incorrect input, on all of its operations.
const BAD_REQUEST: &str = "BadRequestException";

/// The lead-in of the message this service words a refused constraint with.
const CONSTRAINT_REFUSAL_PREFIX: &str = "Bad request due to incorrect input: ";

/// What every `@httpLabel` segment of a derived route is filled with: an ARN shaped like the
/// ones this service mints, addressing nothing any store holds. The refusal under test is
/// raised at the request boundary, before any handler runs, so what the label addresses
/// never decides the answer — which the `absent_required_query_parameter` suite pins
/// against a resource that IS there.
const LABEL_VALUE: &str =
    "arn:aws:kafkaconnect:eu-west-1:000000000000:connector/required-query-walk/\
     11111111-2222-3333-4444-555555555555-1";

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and read back `(status, code, body)`.
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::new())
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("a JSON response body: {e}"))
    };
    let code = value
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, value)
}

/// The `message` a failure response carries, or `""`.
fn message_of(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One route of this service whose input binds a `@required` member to the query string, as
/// this model declares it.
#[derive(Debug)]
struct RequiredQueryRoute {
    /// The operation's shape name.
    operation: String,
    /// Its `@http` method.
    method: String,
    /// Its `@http` URI with every `@httpLabel` filled, and no query string.
    target: String,
    /// Each `@required` + `@httpQuery` member of its input: the parameter name on the wire,
    /// and the member path a refusal states it by.
    parameters: Vec<(String, String)>,
}

/// Every route of this model whose input binds a `@required` member to the query string, in
/// operation order.
///
/// Read off the model: an operation shape carrying `@http`, whose input declares a member
/// with BOTH `smithy.api#required` and `smithy.api#httpQuery`. Nothing about a route is
/// written out here, so a model that moves a route, renames a parameter or adds an operation
/// to this class moves the rows with it.
fn required_query_routes() -> Vec<RequiredQueryRoute> {
    let model: Value = serde_json::from_str(MODEL).expect("this service's Smithy model parses");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model declares shapes")
        .clone();

    let mut routes: Vec<RequiredQueryRoute> = Vec::new();
    for (shape_id, shape) in &shapes {
        if shape["type"].as_str() != Some("operation") {
            continue;
        }
        let Some(http) = shape["traits"].get("smithy.api#http") else {
            continue;
        };
        let Some(input) = shape["input"]["target"]
            .as_str()
            .and_then(|target| shapes.get(target))
        else {
            continue;
        };
        let mut parameters: Vec<(String, String)> = Vec::new();
        for (member_name, member) in input["members"].as_object().into_iter().flatten() {
            if member["traits"].get("smithy.api#required").is_none() {
                continue;
            }
            let Some(query) = member["traits"].get("smithy.api#httpQuery") else {
                continue;
            };
            // A `@httpQuery` trait carries the wire name; one with no value names the member
            // itself, which is how the router's own binding table reads it.
            let parameter = query.as_str().unwrap_or(member_name.as_str()).to_string();
            parameters.push((
                parameter,
                // The spelling a refusal states the member by, from the framework's own path
                // renderer rather than a second copy of its casing rule.
                aws_smithy_schema_server::constraints::path::member(member_name),
            ));
        }
        if parameters.is_empty() {
            continue;
        }
        routes.push(RequiredQueryRoute {
            operation: shape_id
                .rsplit('#')
                .next()
                .unwrap_or(shape_id.as_str())
                .to_string(),
            method: http["method"]
                .as_str()
                .expect("an @http trait states its method")
                .to_string(),
            target: with_labels_filled(http["uri"].as_str().expect("an @http trait states its uri")),
            parameters,
        });
    }
    routes.sort_by(|left, right| left.operation.cmp(&right.operation));
    routes
}

/// `uri_pattern` with every `{Label}` / `{Label+}` segment replaced by [`LABEL_VALUE`],
/// percent-encoded into ONE path segment the way an AWS SDK encodes a label, and any static
/// query literal of the pattern dropped — the request target a client sends for that route.
fn with_labels_filled(uri_pattern: &str) -> String {
    let encoded =
        percent_encoding::utf8_percent_encode(LABEL_VALUE, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
    let path = uri_pattern.split('?').next().unwrap_or(uri_pattern);
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') && segment.ends_with('}') {
                encoded.clone()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<String>>()
        .join("/")
}

/// The clause the framework states for a required member the request supplied nothing for,
/// at the member path `at`.
fn missing_member_clause(at: &str) -> String {
    format!("Value null at '{at}' failed to satisfy constraint: Member must not be null")
}

/// `target` with `parameter` carrying `value` appended to its query string.
fn with_parameter(target: &str, parameter: &str, value: &str) -> String {
    match target.contains('?') {
        true => format!("{target}&{parameter}={value}"),
        false => format!("{target}?{parameter}={value}"),
    }
}

#[tokio::test]
async fn every_modelled_required_query_parameter_left_out_is_the_modelled_bad_request() {
    let emulator = emulator();
    let routes = required_query_routes();

    let found: Vec<&str> = routes
        .iter()
        .map(|route| route.operation.as_str())
        .collect();
    for operation in KNOWN_OPERATIONS {
        assert!(
            found.contains(operation),
            "case {operation:?}: the model walk must find every operation that binds a \
             required query parameter, found {found:?}"
        );
    }

    for route in &routes {
        let case = format!("{} {} {}", route.operation, route.method, route.target);

        // (1) Every required query parameter left out — the only shape of "none" a
        // query-bound list has, since an empty list serializes to no parameter at all.
        let (status, code, body) = call(&emulator, &route.method, &route.target).await;
        assert_eq!(
            (status, code.as_str()),
            (400, BAD_REQUEST),
            "case {case:?}: a request leaving its required query parameter out is refused as \
             a client error, got {status} {code:?}: {body}"
        );
        let message = message_of(&body);
        assert!(
            message.starts_with(CONSTRAINT_REFUSAL_PREFIX),
            "case {case:?}: expected a message starting {CONSTRAINT_REFUSAL_PREFIX:?}, got \
             {message:?}"
        );
        for (_, at) in &route.parameters {
            let clause = missing_member_clause(at);
            assert!(
                message.contains(&clause),
                "case {case:?}: expected the message to state {clause:?}, got {message:?}"
            );
        }

        // (2) The SAME route with every required parameter supplied: the request routes and
        // reaches the operation, so whatever it is answered with, it is neither a refusal for
        // an absent member nor a report that the operation is unimplemented.
        let mut supplied = route.target.clone();
        for (parameter, _) in &route.parameters {
            supplied = with_parameter(&supplied, parameter, "1");
        }
        let (status, code, body) = call(&emulator, &route.method, &supplied).await;
        assert_ne!(
            status, 501,
            "case {case:?}: the route is implemented, so a request carrying its required \
             parameter reaches it, got {status} {code:?}: {body}"
        );
        let message = message_of(&body);
        for (_, at) in &route.parameters {
            let clause = missing_member_clause(at);
            assert!(
                !message.contains(&clause),
                "case {case:?}: a request that supplies {at:?} is not refused for its \
                 absence, got {status} {code:?}: {body}"
            );
        }
    }
}

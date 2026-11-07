// SPDX-License-Identifier: CC0-1.0

//! A request that leaves a `@required` query-bound member out, end to end over HTTP, on
//! EVERY route of this service whose model binds one.
//!
//! The rule under test: a request whose method and path address a modelled route of this
//! service, and which the REST router matches to no operation only because a `@required`
//! request-observable binding of that route is absent, is a refusal of the REQUEST — this
//! service's `BadRequestException` (400) in its `Validation failed:` envelope, naming the
//! absent member — and never a report that the operation is not implemented. The router's
//! required-binding check exists to pick BETWEEN operations overloaded on one method + URI
//! template; it cannot un-name the only operation a route has.
//!
//! The rows are DERIVED FROM THE MODEL rather than written out here: the suite walks this
//! service's own Smithy model for every operation whose input binds a `@required` member to
//! the query string, so the class is covered whole — the three batch reads (`Arns`) and the
//! two tag removals (`TagKeys`) — and an operation the model adds to that class is covered
//! the day it arrives. They span what the rule generalises over: the HTTP method (GET,
//! DELETE), a path with no label and a path with one, a two-segment literal prefix and a
//! three-segment one, and the parameter absent, present, and present carrying no value.
//!
//! The negative rows matter as much: a request that addresses NO route of this service —
//! an unknown path, a longer path, a method the route does not have — keeps the protocol's
//! own routing answer, so the fix cannot turn every unrouted request into a 400.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;

/// This service's Smithy model, so the rows below are the model's own rather than a copy of
/// it that can drift.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json"
));

/// The operations this model binds a `@required` member to the query string on, as
/// `smithy_query(query="operation",operation="BatchGetRouterInput,\
/// BatchGetRouterNetworkInterface,BatchGetRouterOutput,UntagResource,UntagGlobalResource")`
/// reports them. Asserted to be a subset of what the walk below finds, so a walk that
/// silently stopped finding routes cannot pass for a fix.
const KNOWN_OPERATIONS: &[&str] = &[
    "BatchGetRouterInput",
    "BatchGetRouterNetworkInterface",
    "BatchGetRouterOutput",
    "UntagGlobalResource",
    "UntagResource",
];

/// What every `@httpLabel` segment of a derived route is filled with: an ARN shaped like the
/// ones this service mints, addressing nothing any store holds. The refusal under test is
/// raised before any handler runs, so what the label addresses never decides the answer.
const LABEL_VALUE: &str = "arn:aws:mediaconnect:us-east-1:123456789012:routerInput:aaaaaaaaaaaa";

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
    /// and the member path a refusal names it by.
    parameters: Vec<(String, String)>,
}

/// Every route of this model whose input binds a `@required` member to the query string, in
/// operation order.
///
/// Read off the model: an operation shape carrying `@http`, whose input declares a member
/// with BOTH `smithy.api#required` and `smithy.api#httpQuery`. Nothing about the route is
/// written out here, so a model that moves a route, renames a parameter or adds an
/// operation to this class moves the rows with it.
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
            // A `@httpQuery` trait carries the wire name; a trait with no value names the
            // member itself, which is how the router's own binding table reads it.
            let parameter = query.as_str().unwrap_or(member_name.as_str()).to_string();
            parameters.push((
                parameter,
                // The spelling a refusal states the member by, from the framework's own
                // path renderer rather than a second copy of its casing rule.
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
            target: with_labels_filled(
                http["uri"].as_str().expect("an @http trait states its uri"),
            ),
            parameters,
        });
    }
    routes.sort_by(|left, right| left.operation.cmp(&right.operation));
    routes
}

/// `uri_pattern` with every `{Label}` / `{Label+}` segment replaced by [`LABEL_VALUE`], and
/// any static query literal of the pattern dropped — the request target a client sends for
/// that route.
fn with_labels_filled(uri_pattern: &str) -> String {
    let path = uri_pattern.split('?').next().unwrap_or(uri_pattern);
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') && segment.ends_with('}') {
                LABEL_VALUE
            } else {
                segment
            }
        })
        .collect::<Vec<&str>>()
        .join("/")
}

/// The clause this service states for a required member the request supplied nothing for:
/// the framework's own sentence, which names the member and the rule it broke (see
/// `src/service.rs`, where every boundary refusal of this service is worded).
///
/// `pub(crate)` because the tag-removal suite asserts the same sentence for the routes it
/// drives (`tests/all/tag_removal_without_tag_keys.rs`), so there is one spelling of it in
/// this test binary.
pub(crate) fn missing_member_clause(member_path: &str) -> String {
    format!("Value null at '{member_path}' failed to satisfy constraint: Member must not be null")
}

/// `target` with `parameter` carrying `value` appended to its query string.
fn with_parameter(target: &str, parameter: &str, value: &str) -> String {
    match target.contains('?') {
        true => format!("{target}&{parameter}={value}"),
        false => format!("{target}?{parameter}={value}"),
    }
}

/// The message of a reply, whichever casing the envelope carries: a framework refusal
/// renders `message`, a modelled error the model's own `Message` member.
///
/// `pub(crate)` because the tag-removal suite reads the message off a reply the same way.
pub(crate) fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every route this model binds a `@required` query parameter on refuses a request that
/// leaves it out with this service's client-input error, naming the absent member — and
/// serves the same route once the parameter is supplied.
#[tokio::test]
async fn a_required_query_parameter_left_out_is_refused_as_a_client_error() {
    let emulator = Emulator::new();
    let routes = required_query_routes();

    let found: Vec<&str> = routes
        .iter()
        .map(|route| route.operation.as_str())
        .collect();
    for operation in KNOWN_OPERATIONS {
        assert!(
            found.contains(operation),
            "case {operation:?}: the model walk finds every operation that binds a required \
             query parameter, found {found:?}"
        );
    }

    for route in &routes {
        let case = format!("{} {} {}", route.operation, route.method, route.target);

        // (1) Every required parameter left out: the only shape of "none" a query-bound
        // list has, since an empty list serializes to no parameter at all.
        let (status, code, body) = emulator.send(&route.method, &route.target, None).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a request leaving its required query parameter out is refused as \
             a client error, got {status} {code:?}: {body}"
        );
        let message = message_of(&body);
        let named = route.parameters.iter().any(|(_, member_path)| {
            message == mc::validation_failed_message(&missing_member_clause(member_path))
        });
        assert!(
            named,
            "case {case:?}: the refusal states one of {:?} as absent in this service's \
             envelope, got {message:?}",
            route
                .parameters
                .iter()
                .map(|(_, member_path)| member_path.as_str())
                .collect::<Vec<&str>>()
        );

        // (2) The SAME route with every required parameter supplied: the request routes and
        // reaches the operation, so whatever it is answered with, it is not a refusal for an
        // absent member and not a report that the operation is unimplemented.
        let mut supplied = route.target.clone();
        for (parameter, _) in &route.parameters {
            supplied = with_parameter(&supplied, parameter, LABEL_VALUE);
        }
        let (status, code, body) = emulator.send(&route.method, &supplied, None).await;
        let message = message_of(&body);
        assert_ne!(
            status, 501,
            "case {case:?}: the route is implemented, so a request carrying its required \
             parameter reaches it, got {status} {code:?}: {body}"
        );
        for (_, member_path) in &route.parameters {
            assert_ne!(
                message,
                mc::validation_failed_message(&missing_member_clause(member_path)),
                "case {case:?}: a request that supplies {member_path:?} is not refused for \
                 its absence, got {status} {code:?}: {body}"
            );
        }
    }
}

/// A request that addresses no route of this service keeps the protocol's own routing
/// answer, and a required parameter that IS on the query string routes whatever value it
/// carries — so only an ABSENT required parameter is read as a refusal.
#[tokio::test]
async fn only_an_absent_required_parameter_turns_a_routing_failure_into_a_refusal() {
    let emulator = Emulator::new();

    // (case, method, target): every row addresses no modelled route of this service, one way
    // each — an unknown path, a path one segment longer than a required-query route's, a
    // path one segment shorter than one, and a method no route of that path has.
    let unrouted = [
        ("a path no route of this service matches", "GET", "/v1/nope"),
        (
            "a segment past a required-query route's path",
            "GET",
            "/v1/routerInputs/extra",
        ),
        ("a bare tag path with no resource", "DELETE", "/tags"),
        (
            "a method the required-query route does not have",
            "PUT",
            "/v1/routerInputs",
        ),
    ];
    for (case, method, target) in unrouted {
        let (status, code, body) = emulator.send(method, target, None).await;
        assert_eq!(
            (status, code.as_deref()),
            (501, Some("NotImplemented")),
            "case {case:?} ({method} {target}): a request that names no modelled operation \
             keeps the protocol's routing answer, got {status} {code:?}: {body}"
        );
    }

    // The parameter's NAME is what routes the request: present with no value at all, the
    // route matches and the VALUE is what the boundary then refuses — a different refusal
    // from the absent-member one, in the same envelope.
    let (status, code, body) = emulator.send("GET", "/v1/routerInputs?arns=", None).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "a required parameter carrying no value routes and is refused for its value, got \
         {status} {code:?}: {body}"
    );
    let message = message_of(&body);
    assert_ne!(
        message,
        mc::validation_failed_message(&missing_member_clause("arns")),
        "a parameter present with no value is not refused as absent, got {message:?}"
    );
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX),
        "every refusal of a request's content carries this service's validation envelope, \
         got {message:?}"
    );
}

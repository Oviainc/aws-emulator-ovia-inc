// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what this service answers a request that names none of its
//! routes.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a request this endpoint claims that resolves to NO modelled route
//! is refused by this service's front door — HTTP 401, the wire error code
//! `UnauthorizedException`, and the one fixed body member `message` = `Unauthorized`,
//! which is what that layer states for everything it refuses — rather than with any
//! modelled error of an operation it never reached, and rather than with a report about
//! the emulator. Two shapes of request resolve to no route, and the rows below span both:
//!
//! * a method and path matching no route of this endpoint: an unknown path, a known path
//!   under a method no operation binds, and a path carrying too few or too many segments
//!   for every pattern;
//! * a method and path matching one, where the request carries no value for a `@required`
//!   query member that route binds — the operation cannot be invoked without it.
//!   `UntagResource` is the one route of this service with such a member
//!   (`smithy_query(query="operation", operation="UntagResource")`: `TagKeys` is
//!   `@required` with `httpQuery: tagKeys`), so an SDK asked to remove an EMPTY set of
//!   keys sends `DELETE /tags/{ResourceArn}` with no query at all and names no route.
//!
//! Each unroutable row is paired with the positive controls: the same routes, driven as
//! the model binds them, are served normally — including `UntagResource` with one real
//! key, read back through `ListTagsForResource` — so what the rows above show is the
//! absence of a route and not a service that stopped serving the tag plane.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::QUICK_SETUP_TYPE_CATALOG;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The status this service's front door refuses on.
const UNAUTHORIZED_STATUS: u16 = 401;

/// The wire error code it carries — a shape this service's model does not declare, which
/// is what tells a client the refusal came from in front of the service.
const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// The whole sentence it states, for every request it refuses: fixed text, naming no
/// member, value or resource, so there is nothing in it taken from the request.
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on
/// and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a refusal carried, read through either casing restJson1 may render an
/// error structure's single message member under.
fn error_message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .filter_map(|key| body.get(*key))
        .filter_map(Value::as_str)
        .next()
        .unwrap_or("<no message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an SDK
/// does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Create one configuration manager carrying `tags`, answering its ARN.
async fn create_manager(
    router: &MultiServiceRouter,
    case: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let mut tag_map = Map::new();
    for (key, value) in tags {
        tag_map.insert(key.clone(), Value::String(value.clone()));
    }
    // `Type` must name a configuration type this service publishes; no case here is about
    // the type, so every create uses the catalog's first entry. Its parameters name
    // somewhere to deploy to and the Region to deploy into, and provide every parameter
    // the table documented for that type marks `(Required)`, which every accepted
    // definition must.
    let type_ = QUICK_SETUP_TYPE_CATALOG[0].type_;
    let body = json!({
        "Name": "routing-rule-manager",
        "ConfigurationDefinitions": [{
            "Type": type_,
            "Parameters": super::type_parameter_fixtures::with_required_parameters(
                type_,
                json!({
                    "TargetOrganizationalUnits": "ou-1",
                    "TargetRegions": "us-east-1",
                }),
            ),
        }],
        "Tags": Value::Object(tag_map),
    });
    let (status, answer) = call(router, "POST", "/configurationManager", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// The tags `ListTagsForResource` reports for `resource_arn`.
///
/// Its output member is a LIST of `{Key, Value}` pairs
/// (`smithy_query(query="operation", operation="ListTagsForResource")`), read here into a
/// map so a row's expectation is independent of the order they are reported in.
async fn reported_tags(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/tags/{}", label(resource_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource must succeed, got {status} / {answer:?}"
    );
    let tags = answer
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags member, got {answer:?}"));
    tags.as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Tags must be a list, got {tags:?}"))
        .iter()
        .map(|entry| {
            let key = entry
                .get("Key")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag must carry a Key, got {entry:?}"));
            let value = entry
                .get("Value")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag must carry a Value, got {entry:?}"));
            (key.to_string(), value.to_string())
        })
        .collect()
}

/// A tag map, from pairs.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Assert one front-door refusal: the status, the wire error code, and the whole fixed
/// sentence under the lowercase `message` member.
fn assert_unauthorized(case: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, UNAUTHORIZED_STATUS,
        "case {case:?}: expected {UNAUTHORIZED_STATUS}, got {status} / {answer:?}"
    );
    let code = error_code(answer);
    assert_eq!(
        code, UNAUTHORIZED_CODE,
        "case {case:?}: expected {UNAUTHORIZED_CODE}, got {code:?} in {answer:?}"
    );
    let message = error_message(answer);
    assert_eq!(
        message, UNAUTHORIZED_MESSAGE,
        "case {case:?}: expected {UNAUTHORIZED_MESSAGE:?}, got {message:?} in {answer:?}"
    );
    // The sentence travels under the lowercase member the front door names, not under the
    // `Message` this service's own modelled errors carry.
    assert_eq!(
        answer.get("message").and_then(Value::as_str),
        Some(UNAUTHORIZED_MESSAGE),
        "case {case:?}: expected the sentence under the lowercase `message`, got {answer:?}"
    );
}

/// One request that names no route of this endpoint: its method, its URI (built around
/// the ARN of a manager this endpoint DOES serve, so nothing about the row is about an
/// unknown target), and its body.
struct Row {
    name: &'static str,
    method: &'static str,
    uri: Box<dyn Fn(&str) -> String>,
    body: Option<Value>,
}

/// Every unroutable request shape, spanning both halves of the rule: a `@required` query
/// member the route binds and the request omits, and a method/path pair no route pattern
/// matches at all (`smithy_query(query="digest")` is where the 14 routes come from).
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The empty-key removal: an SDK sends `TagKeys=[]` as no `tagKeys` parameter.
            name: "UntagResource with no tagKeys parameter",
            method: "DELETE",
            uri: Box::new(|arn| format!("/tags/{}", label(arn))),
            body: None,
        },
        Row {
            name: "UntagResource carrying some other query parameter instead",
            method: "DELETE",
            uri: Box::new(|arn| format!("/tags/{}?maxResults=1", label(arn))),
            body: None,
        },
        Row {
            // A query parameter's name is case-sensitive, so `TagKeys` is not `tagKeys`.
            name: "UntagResource naming the query parameter in the wrong case",
            method: "DELETE",
            uri: Box::new(|arn| format!("/tags/{}?TagKeys=Env", label(arn))),
            body: None,
        },
        Row {
            name: "a path no route of this service names",
            method: "GET",
            uri: Box::new(|_| "/thereIsNoSuchRoute".to_string()),
            body: None,
        },
        Row {
            name: "the tag plane under a method no operation binds",
            method: "POST",
            uri: Box::new(|arn| format!("/tags/{}", label(arn))),
            body: Some(json!({ "Tags": { "Env": "prod" } })),
        },
        Row {
            name: "the manager route under a method no operation binds",
            method: "PATCH",
            uri: Box::new(|arn| format!("/configurationManager/{}", label(arn))),
            body: Some(json!({ "Name": "renamed" })),
        },
        Row {
            name: "a list route under the method its sibling binds",
            method: "GET",
            uri: Box::new(|_| "/listConfigurationManagers".to_string()),
            body: None,
        },
        Row {
            name: "the tag plane with its label left off",
            method: "GET",
            uri: Box::new(|_| "/tags".to_string()),
            body: None,
        },
        Row {
            name: "the manager route with its label left off",
            method: "GET",
            uri: Box::new(|_| "/configurationManager/".to_string()),
            body: None,
        },
        Row {
            name: "the tag plane with one segment too many",
            method: "GET",
            uri: Box::new(|arn| format!("/tags/{}/extra", label(arn))),
            body: None,
        },
        Row {
            name: "the configuration-definition route with its id left off",
            method: "PUT",
            uri: Box::new(|arn| format!("/configurationDefinition/{}", label(arn))),
            body: Some(json!({ "TypeVersion": "LATEST" })),
        },
    ]
}

/// Each row earns the front door's refusal, and the state the row's request would have
/// touched is untouched by it.
#[tokio::test]
async fn a_request_naming_no_route_is_refused_by_the_front_door() {
    let stored = tags(&[("keep", "me"), ("Env", "prod")]);
    for row in rows() {
        let router = emulator();
        let arn = create_manager(&router, row.name, &stored).await;

        let uri = (row.uri)(&arn);
        let (status, answer) = call(&router, row.method, &uri, row.body.as_ref()).await;
        assert_unauthorized(row.name, status, &answer);

        let left = reported_tags(&router, row.name, &arn).await;
        assert_eq!(
            left, stored,
            "case {:?}: a refused request must change nothing, expected {stored:?}, got {left:?}",
            row.name
        );
    }
}

/// The positive control for the rule's second half: the same route, carrying the
/// `@required` query member, is served — the named key is removed and a later
/// `ListTagsForResource` reports what is left — so the rows above show a request that
/// names no route rather than an operation that stopped working.
#[tokio::test]
async fn untag_resource_carrying_the_query_member_is_served() {
    let case = "one real key";
    let router = emulator();
    let arn = create_manager(&router, case, &tags(&[("keep", "me"), ("Env", "prod")])).await;

    let (status, answer) = call(
        &router,
        "DELETE",
        &format!("/tags/{}?tagKeys=keep", label(&arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UntagResource must answer 200, got {status} / {answer:?}"
    );

    let left = reported_tags(&router, case, &arn).await;
    assert_eq!(
        left,
        tags(&[("Env", "prod")]),
        "case {case:?}: expected only the untouched tag to remain, got {left:?}"
    );
}

/// The positive control for the rule's first half: every route the rows above malformed
/// is served when it is named as the model binds it, so the refusal is about the request's
/// own method and path and not about this service refusing a route family.
#[tokio::test]
async fn every_route_named_as_the_model_binds_it_is_served() {
    let router = emulator();
    let arn = create_manager(&router, "routed", &tags(&[("keep", "me")])).await;

    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "ListTagsForResource",
            "GET",
            format!("/tags/{}", label(&arn)),
            None,
        ),
        (
            "TagResource",
            "PUT",
            format!("/tags/{}", label(&arn)),
            Some(json!({ "Tags": { "Env": "prod" } })),
        ),
        (
            "GetConfigurationManager",
            "GET",
            format!("/configurationManager/{}", label(&arn)),
            None,
        ),
        (
            "ListConfigurationManagers",
            "POST",
            "/listConfigurationManagers".to_string(),
            Some(json!({})),
        ),
        (
            "ListQuickSetupTypes",
            "GET",
            "/listQuickSetupTypes".to_string(),
            None,
        ),
        (
            "GetServiceSettings",
            "GET",
            "/serviceSettings".to_string(),
            None,
        ),
    ];

    for (case, method, uri, body) in cases {
        let (status, answer) = call(&router, method, &uri, body.as_ref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {method} {uri} must be served, got {status} / {answer:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! A WAFv2 WEB ACL in front of a deployed REST API stage, end to end over the wire.
//!
//! The rule this suite pins: a request to a deployed stage that has a web ACL
//! associated is DECIDED BY THAT WEB ACL before the stage serves it, and the stage
//! always produces an HTTP response — the web ACL's rules run in ascending priority,
//! the first terminating action settles the request, and the `DefaultAction` decides
//! one no rule matched. A block is answered at the edge (through the api's own
//! `WAF_FILTERED` gateway response, or with the `BlockAction`'s own `CustomResponse`
//! when it configures one), an allow reaches the origin carrying the headers a
//! `CustomRequestHandling` inserted, and a counted rule's labels are visible to the
//! rules that run after it.
//!
//! Each test asserts that rule over a TABLE rather than one recorded request: the rows
//! differ along what the rule generalises over — which statement decides, which
//! component it inspects, how large the inspected component is, where in a forwarded
//! header an address sits, how many requests a caller has already sent — and each row
//! names its case and the actual answer.
//!
//! Every call goes through the real wire path: the api and its stages are built through
//! API Gateway's own control plane, the web ACLs through WAFv2's, and the requests are
//! unsigned GETs and POSTs to the api's own invoke host — both services mounted on ONE
//! shared engine behind the multi-service router, which is what lets one request reach
//! the apigateway data plane and the web ACL the wafv2 control plane stored.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the invoke host and both control planes are addressed in. One region is
/// enough: the stage's ARN carries the region the invoke host names, and the
/// association is keyed by that ARN, so a second region would exercise the ARN
/// builder rather than the web ACL.
const REGION: &str = "us-east-1";

/// The `Scope` an API Gateway stage's web ACL lives in: a stage is a REGIONAL resource
/// (`Scope`'s `CLOUDFRONT` value is for "a global resource type, such as a Amazon
/// CloudFront distribution").
const SCOPE: &str = "REGIONAL";

/// One emulator with API Gateway and WAFv2 mounted on a single shared engine.
struct Edge {
    router: MultiServiceRouter,
}

/// One answered request: its status, its headers and its body verbatim.
struct Answered {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Answered {
    /// The `message` member of a JSON body — what every API Gateway gateway response
    /// carries — or the body itself when it carries none.
    fn message(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|body| body.get("message")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| self.body.clone())
    }

    /// The first value of the header `name`, or `None` when the response carries none.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

impl Edge {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests exercise the services, not SigV4: the control-plane requests carry
        // a credential scope so the router can name the target service, and the invoke
        // requests carry no credential at all, as a client of a published invoke URL
        // sends none.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    /// One API Gateway control-plane call, answering its status and parsed body.
    async fn api_call(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("apigateway.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/apigateway/aws4_request, \
                     SignedHeaders=host, Signature=0"
                ),
            )
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// An API Gateway control-plane call that must be accepted with `expected`.
    async fn api_ok(&self, method: &str, uri: &str, body: Option<Value>, expected: u16) -> Value {
        let (status, parsed) = self.api_call(method, uri, body).await;
        assert_eq!(
            status, expected,
            "case {method} {uri}: expected {expected}, got {status} with {parsed}"
        );
        parsed
    }

    /// One WAFv2 call, answering its status and parsed body.
    async fn waf_call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://wafv2.example.com/")
            .header("host", "wafv2.example.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AWSWAF_20190729.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLEWAFV2/20260101/{REGION}/wafv2/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=00"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status, parsed)
    }

    /// A WAFv2 call that must succeed.
    async fn waf_ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.waf_call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One unsigned request to a deployed stage's invoke URL — the api's own host, no
    /// credential, the headers and body the case sends.
    async fn invoke(
        &self,
        api: &str,
        verb: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Answered {
        let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(verb)
            .uri(format!("https://{host}{path}"))
            .header("host", &host);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::copy_from_slice(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = String::from_utf8_lossy(&response.into_body()).into_owned();
        Answered {
            status,
            headers,
            body,
        }
    }

    /// [`Self::invoke`]: a plain GET with no headers and no body.
    async fn get(&self, api: &str, path: &str) -> Answered {
        self.invoke(api, "GET", path, &[], &[]).await
    }
}

/// The ARN `AssociateWebACL` binds a web ACL to a REST API stage by:
/// `arn:aws:apigateway:{region}::/restapis/{api-id}/stages/{stage}`, the one form
/// `AssociateWebACLRequest$ResourceArn` documents for an API Gateway stage.
fn stage_arn(api: &str, stage: &str) -> String {
    format!("arn:aws:apigateway:{REGION}::/restapis/{api}/stages/{stage}")
}

/// A `VisibilityConfig` that stores samples, so `GetSampledRequests` has a population
/// to draw from.
fn visibility(metric: &str) -> Value {
    json!({
        "SampledRequestsEnabled": true,
        "CloudWatchMetricsEnabled": true,
        "MetricName": metric,
    })
}

/// A web ACL rule: a name, a priority, a statement and an action.
fn rule(name: &str, priority: i64, statement: Value, action: Value) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": statement,
        "Action": action,
        "VisibilityConfig": visibility(name),
    })
}

/// A `ByteMatchStatement` over `field` with no transformation.
fn byte_match(field: Value, search: &str, constraint: &str) -> Value {
    json!({
        "ByteMatchStatement": {
            "SearchString": base64_of(search),
            "FieldToMatch": field,
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            "PositionalConstraint": constraint,
        }
    })
}

/// A `Blob` member travels base64-encoded in an awsJson body, which is what
/// `ByteMatchStatement$SearchString` is.
fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// Create a web ACL with `default_action` and `rules`, answering `(id, arn, lock
/// token)`.
async fn create_web_acl(
    edge: &Edge,
    name: &str,
    default_action: Value,
    rules: Value,
    extra: Value,
) -> (String, String, String) {
    let mut body = json!({
        "Name": name,
        "Scope": SCOPE,
        "DefaultAction": default_action,
        "VisibilityConfig": visibility(name),
        "Rules": rules,
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        body[key] = value;
    }
    let created = edge.waf_ok("CreateWebACL", body).await;
    let summary = &created["Summary"];
    (
        summary["Id"].as_str().expect("Summary.Id").to_string(),
        summary["ARN"].as_str().expect("Summary.ARN").to_string(),
        summary["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string(),
    )
}

/// Put the web ACL `web_acl_arn` in front of `api`'s `stage`.
async fn associate(edge: &Edge, web_acl_arn: &str, api: &str, stage: &str) {
    edge.waf_ok(
        "AssociateWebACL",
        json!({ "WebACLArn": web_acl_arn, "ResourceArn": stage_arn(api, stage) }),
    )
    .await;
}

/// An api carrying the resources every case below drives, each on a `MOCK` integration
/// that answers `200` with a body naming the resource — so the answer a request gets
/// says whether it reached the origin.
struct Api {
    id: String,
}

impl Api {
    /// Build the api and its resources; the caller deploys the stages it needs.
    async fn build(edge: &Edge, name: &str) -> Self {
        let created = edge
            .api_ok("POST", "/restapis", Some(json!({ "name": name })), 201)
            .await;
        let id = created["id"].as_str().expect("the api id").to_string();
        let root = created["rootResourceId"]
            .as_str()
            .expect("the root resource id")
            .to_string();
        let api = Self { id };

        // A request validator that checks the method's request PARAMETERS, so a method
        // can declare a header required: that is what makes a header WAF inserted
        // observable at the origin, since a request missing it is refused by the api
        // itself.
        let validator = edge
            .api_ok(
                "POST",
                &format!("/restapis/{}/requestvalidators", api.id),
                Some(json!({ "name": "parameters", "validateRequestParameters": true })),
                201,
            )
            .await;
        let validator_id = validator["id"]
            .as_str()
            .expect("the validator id")
            .to_string();

        for (path_part, verb, extra) in [
            ("orders", "GET", json!({})),
            ("admin", "GET", json!({})),
            ("upload", "POST", json!({})),
            (
                "tagged",
                "GET",
                json!({
                    "requestParameters": { "method.request.header.x-amzn-waf-tag": true },
                    "requestValidatorId": validator_id,
                }),
            ),
        ] {
            api.resource(edge, &root, path_part, verb, extra).await;
        }
        api
    }

    /// One resource under `root` with a method on `verb` and a `MOCK` integration that
    /// answers `200 {"at":"<path_part>"}`.
    async fn resource(&self, edge: &Edge, root: &str, path_part: &str, verb: &str, extra: Value) {
        let created = edge
            .api_ok(
                "POST",
                &format!("/restapis/{}/resources/{root}", self.id),
                Some(json!({ "pathPart": path_part })),
                201,
            )
            .await;
        let resource = created["id"].as_str().expect("the resource id").to_string();
        let mut method = json!({ "authorizationType": "NONE" });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            method[key] = value;
        }
        edge.api_ok(
            "PUT",
            &format!(
                "/restapis/{}/resources/{resource}/methods/{verb}",
                self.id
            ),
            Some(method),
            201,
        )
        .await;
        edge.api_ok(
            "PUT",
            &format!(
                "/restapis/{}/resources/{resource}/methods/{verb}/integration",
                self.id
            ),
            Some(json!({ "type": "MOCK" })),
            201,
        )
        .await;
        edge.api_ok(
            "PUT",
            &format!(
                "/restapis/{}/resources/{resource}/methods/{verb}/integration/responses/200",
                self.id
            ),
            Some(json!({
                "responseTemplates": {
                    "application/json": format!("{{\"at\":\"{path_part}\"}}")
                }
            })),
            201,
        )
        .await;
    }

    /// Deploy the api into `stage`.
    async fn deploy(&self, edge: &Edge, stage: &str) {
        edge.api_ok(
            "POST",
            &format!("/restapis/{}/deployments", self.id),
            Some(json!({ "stageName": stage })),
            201,
        )
        .await;
    }
}

/// The body a resource's `MOCK` integration answers with.
fn origin_body(path_part: &str) -> String {
    format!("{{\"at\":\"{path_part}\"}}")
}

/// The rule over the STATEMENTS that decide: which action a web ACL settles on, and
/// what the caller is answered, for each kind of rule a web ACL can carry.
///
/// One row per deciding path — the default action with no rule at all, the default
/// action BLOCK, a byte-match rule that blocks with the api's own gateway response, a
/// byte-match rule that blocks with the web ACL's own custom response, an allow rule
/// whose custom request handling the origin sees, and a label a counted rule added
/// that a later rule blocks on — each on its own stage of the same api, so the
/// decision is the web ACL's and nothing else.
#[tokio::test]
async fn a_web_acl_in_front_of_a_stage_decides_every_request_the_stage_answers() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-decisions").await;
    for stage in [
        "none", "allowed", "blocked", "filtered", "custom", "tagging", "labelled",
    ] {
        api.deploy(&edge, stage).await;
    }

    // A stage with NO web ACL: the baseline every other row is read against.
    // `tagged` declares its header required, so the stage itself refuses a request
    // that does not carry one — which is how the inserted-header row below is proved.

    // A web ACL with no rules at all and the default `Allow`.
    let (_, allow_arn, _) =
        create_web_acl(&edge, "allow-all", json!({ "Allow": {} }), json!([]), json!({})).await;
    associate(&edge, &allow_arn, &api.id, "allowed").await;

    // A web ACL whose DEFAULT action blocks: nothing matches, so the default decides.
    let (_, block_arn, _) =
        create_web_acl(&edge, "block-all", json!({ "Block": {} }), json!([]), json!({})).await;
    associate(&edge, &block_arn, &api.id, "blocked").await;

    // A blocking rule with no custom response: answered through the api's own
    // `WAF_FILTERED` gateway response.
    let (_, filtered_arn, _) = create_web_acl(
        &edge,
        "block-orders",
        json!({ "Allow": {} }),
        json!([rule(
            "orders",
            0,
            byte_match(json!({ "UriPath": {} }), "/orders", "ENDS_WITH"),
            json!({ "Block": {} }),
        )]),
        json!({}),
    )
    .await;
    associate(&edge, &filtered_arn, &api.id, "filtered").await;

    // The same rule with the web ACL's OWN custom response: the status, the body and
    // the header are the web ACL's, served at the edge.
    let (_, custom_arn, _) = create_web_acl(
        &edge,
        "block-with-body",
        json!({ "Allow": {} }),
        json!([rule(
            "orders",
            0,
            byte_match(json!({ "UriPath": {} }), "/orders", "ENDS_WITH"),
            json!({
                "Block": {
                    "CustomResponse": {
                        "ResponseCode": 429,
                        "CustomResponseBodyKey": "slow-down",
                        "ResponseHeaders": [{ "Name": "x-waf-case", "Value": "custom" }],
                    }
                }
            }),
        )]),
        json!({
            "CustomResponseBodies": {
                "slow-down": { "ContentType": "APPLICATION_JSON", "Content": "{\"waf\":\"too fast\"}" }
            }
        }),
    )
    .await;
    associate(&edge, &custom_arn, &api.id, "custom").await;

    // An allow rule whose `CustomRequestHandling` inserts a header: the origin sees it,
    // which the method's own required-parameter validator is what proves.
    let (_, tagging_arn, _) = create_web_acl(
        &edge,
        "tag-requests",
        json!({ "Allow": {} }),
        json!([rule(
            "tag",
            0,
            byte_match(json!({ "UriPath": {} }), "/tagged", "ENDS_WITH"),
            json!({
                "Allow": {
                    "CustomRequestHandling": {
                        "InsertHeaders": [{ "Name": "tag", "Value": "edge" }]
                    }
                }
            }),
        )]),
        json!({}),
    )
    .await;
    associate(&edge, &tagging_arn, &api.id, "tagging").await;

    // A COUNTED rule that adds a label, and a later rule that blocks on it: the label
    // a rule adds is visible to the rules that run after it.
    let (_, labelled_arn, _) = create_web_acl(
        &edge,
        "label-then-block",
        json!({ "Allow": {} }),
        json!([
            json!({
                "Name": "mark",
                "Priority": 0,
                "Statement": byte_match(json!({ "SingleHeader": { "Name": "x-tenant" } }), "beta", "EXACTLY"),
                "Action": { "Count": {} },
                "RuleLabels": [{ "Name": "beta" }],
                "VisibilityConfig": visibility("mark"),
            }),
            rule(
                "on-label",
                1,
                json!({ "LabelMatchStatement": { "Scope": "LABEL", "Key": "beta" } }),
                json!({ "Block": {} }),
            ),
        ]),
        json!({}),
    )
    .await;
    associate(&edge, &labelled_arn, &api.id, "labelled").await;

    /// One request and the answer the web ACL in front of its stage earns it.
    struct Case {
        case: &'static str,
        path: String,
        headers: Vec<(&'static str, &'static str)>,
        status: u16,
        body: String,
        response_header: Option<(&'static str, &'static str)>,
    }

    let cases = vec![
        Case {
            case: "no web acl at all: the stage serves the request as it arrived",
            path: "/none/orders".to_string(),
            headers: Vec::new(),
            status: 200,
            body: origin_body("orders"),
            response_header: None,
        },
        Case {
            case: "a web acl with no rules and the default Allow",
            path: "/allowed/orders".to_string(),
            headers: Vec::new(),
            status: 200,
            body: origin_body("orders"),
            response_header: None,
        },
        Case {
            case: "a web acl whose default action blocks",
            path: "/blocked/orders".to_string(),
            headers: Vec::new(),
            status: 403,
            body: "Forbidden".to_string(),
            response_header: None,
        },
        Case {
            case: "a blocking rule, answered through the api's WAF_FILTERED response",
            path: "/filtered/orders".to_string(),
            headers: Vec::new(),
            status: 403,
            body: "Forbidden".to_string(),
            response_header: None,
        },
        Case {
            case: "the same web acl on a path the rule does not match",
            path: "/filtered/admin".to_string(),
            headers: Vec::new(),
            status: 200,
            body: origin_body("admin"),
            response_header: None,
        },
        Case {
            case: "a blocking rule with the web acl's own custom response",
            path: "/custom/orders".to_string(),
            headers: Vec::new(),
            status: 429,
            body: "{\"waf\":\"too fast\"}".to_string(),
            response_header: Some(("x-waf-case", "custom")),
        },
        Case {
            case: "a required header the origin only sees because WAF inserted it",
            path: "/tagging/tagged".to_string(),
            headers: Vec::new(),
            status: 200,
            body: origin_body("tagged"),
            response_header: None,
        },
        Case {
            case: "the same method with no web acl inserting that header",
            path: "/none/tagged".to_string(),
            headers: Vec::new(),
            status: 400,
            body: "Missing required request parameters: [x-amzn-waf-tag]".to_string(),
            response_header: None,
        },
        Case {
            case: "a label a counted rule added, blocked on by a later rule",
            path: "/labelled/orders".to_string(),
            headers: vec![("x-tenant", "beta")],
            status: 403,
            body: "Forbidden".to_string(),
            response_header: None,
        },
        Case {
            case: "the same web acl for a request the counted rule does not mark",
            path: "/labelled/orders".to_string(),
            headers: vec![("x-tenant", "alpha")],
            status: 200,
            body: origin_body("orders"),
            response_header: None,
        },
    ];

    for case in &cases {
        let answered = edge
            .invoke(&api.id, "GET", &case.path, &case.headers, &[])
            .await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected status {}, got {} with body {:?}",
            case.case, case.status, answered.status, answered.body
        );
        assert_eq!(
            answered.message(),
            case.body,
            "case {:?}: expected body {:?}, got {:?}",
            case.case,
            case.body,
            answered.body
        );
        if let Some((name, value)) = case.response_header {
            assert_eq!(
                answered.header(name),
                Some(value),
                "case {:?}: expected the custom response header {name}={value:?}, got {:?}",
                case.case,
                answered.headers
            );
        }
    }
}

/// The rule over how much of a BODY is inspected: inspection stops at the limit the
/// resource type documents, and the field's `OversizeHandling` decides a body that
/// exceeds it.
///
/// One row per handling — `CONTINUE`, `MATCH`, `NO_MATCH` — crossed with a body within
/// the limit and a body past it, with the searched-for fragment placed inside and
/// outside the inspected prefix.
#[tokio::test]
async fn body_inspection_stops_at_the_limit_and_oversize_handling_decides_past_it() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-body").await;

    /// The documented inspection limit for an API Gateway stage: `Body$OversizeHandling`
    /// gives "for CloudFront, API Gateway, Amazon Cognito, App Runner, and Verified
    /// Access, the default limit is 16 KB (16,384 bytes)".
    const LIMIT: usize = 16 * 1024;
    const NEEDLE: &str = "needle";

    for handling in ["CONTINUE", "MATCH", "NO_MATCH"] {
        let stage = handling.to_ascii_lowercase();
        api.deploy(&edge, &stage).await;
        let (_, arn, _) = create_web_acl(
            &edge,
            &format!("body-{stage}"),
            json!({ "Allow": {} }),
            json!([rule(
                "body",
                0,
                json!({
                    "ByteMatchStatement": {
                        "SearchString": base64_of(NEEDLE),
                        "FieldToMatch": { "Body": { "OversizeHandling": handling } },
                        "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                        "PositionalConstraint": "CONTAINS",
                    }
                }),
                json!({ "Block": {} }),
            )]),
            json!({}),
        )
        .await;
        associate(&edge, &arn, &api.id, &stage).await;
    }

    /// One body and the status the web ACL of `stage` answers it with.
    struct Case {
        case: &'static str,
        stage: &'static str,
        body: Vec<u8>,
        status: u16,
    }

    /// A body of `len` bytes of filler with `NEEDLE` at `at`, or no needle at all when
    /// `at` is past the end.
    fn body_with_needle(len: usize, at: Option<usize>) -> Vec<u8> {
        let mut body = vec![b'.'; len];
        if let Some(at) = at {
            body[at..at + NEEDLE.len()].copy_from_slice(NEEDLE.as_bytes());
        }
        body
    }

    let cases = [
        Case {
            case: "CONTINUE: the fragment sits inside the inspected prefix",
            stage: "continue",
            body: body_with_needle(64, Some(8)),
            status: 403,
        },
        Case {
            case: "CONTINUE: a body within the limit that carries no fragment",
            stage: "continue",
            body: body_with_needle(64, None),
            status: 200,
        },
        Case {
            case: "CONTINUE: an oversize body whose fragment sits PAST the limit, so \
                   inspection never reaches it",
            stage: "continue",
            body: body_with_needle(LIMIT * 2, Some(LIMIT + 32)),
            status: 200,
        },
        Case {
            case: "CONTINUE: an oversize body whose fragment sits inside the prefix",
            stage: "continue",
            body: body_with_needle(LIMIT * 2, Some(16)),
            status: 403,
        },
        Case {
            case: "MATCH: an oversize body with no fragment at all still matches",
            stage: "match",
            body: body_with_needle(LIMIT * 2, None),
            status: 403,
        },
        Case {
            case: "MATCH: a body within the limit is inspected normally",
            stage: "match",
            body: body_with_needle(64, None),
            status: 200,
        },
        Case {
            case: "NO_MATCH: an oversize body whose fragment sits inside the prefix \
                   does not match",
            stage: "no_match",
            body: body_with_needle(LIMIT * 2, Some(16)),
            status: 200,
        },
        Case {
            case: "NO_MATCH: a body within the limit is inspected normally",
            stage: "no_match",
            body: body_with_needle(64, Some(8)),
            status: 403,
        },
    ];

    for case in &cases {
        let answered = edge
            .invoke(
                &api.id,
                "POST",
                &format!("/{}/upload", case.stage),
                &[("content-type", "application/json")],
                &case.body,
            )
            .await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected status {} for a {}-byte body, got {} with {:?}",
            case.case,
            case.status,
            case.body.len(),
            answered.status,
            answered.body
        );
    }
}

/// The rule over an IP-set reference: with an `IPSetForwardedIPConfig` the address
/// judged is the one at the configured POSITION of the named header, and the
/// `FallbackBehavior` decides a request that carries no address there.
///
/// One row per position (`FIRST`, `LAST`, `ANY`) crossed with where in the header the
/// listed address sits, plus a request carrying no such header at all.
#[tokio::test]
async fn an_ip_set_reference_judges_the_address_at_the_configured_header_position() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-forwarded").await;

    /// The one address the IP set holds; every row differs in WHERE it sits.
    const LISTED: &str = "198.51.100.9";
    const OTHER: &str = "203.0.113.7";

    let ip_set = edge
        .waf_ok(
            "CreateIPSet",
            json!({
                "Name": "listed",
                "Scope": SCOPE,
                "IPAddressVersion": "IPV4",
                "Addresses": [format!("{LISTED}/32")],
            }),
        )
        .await;
    let ip_set_arn = ip_set["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();

    for position in ["FIRST", "LAST", "ANY"] {
        let stage = position.to_ascii_lowercase();
        api.deploy(&edge, &stage).await;
        let (_, arn, _) = create_web_acl(
            &edge,
            &format!("forwarded-{stage}"),
            json!({ "Allow": {} }),
            json!([rule(
                "forwarded",
                0,
                json!({
                    "IPSetReferenceStatement": {
                        "ARN": ip_set_arn,
                        "IPSetForwardedIPConfig": {
                            "HeaderName": "X-Forwarded-For",
                            "FallbackBehavior": "NO_MATCH",
                            "Position": position,
                        }
                    }
                }),
                json!({ "Block": {} }),
            )]),
            json!({}),
        )
        .await;
        associate(&edge, &arn, &api.id, &stage).await;
    }

    /// One forwarded-for header and the status each position answers it with.
    struct Case {
        case: &'static str,
        stage: &'static str,
        forwarded: Option<String>,
        status: u16,
    }

    let cases = [
        Case {
            case: "FIRST: the listed address is first in the header",
            stage: "first",
            forwarded: Some(format!("{LISTED}, {OTHER}")),
            status: 403,
        },
        Case {
            case: "FIRST: the listed address is last in the header",
            stage: "first",
            forwarded: Some(format!("{OTHER}, {LISTED}")),
            status: 200,
        },
        Case {
            case: "LAST: the listed address is last in the header",
            stage: "last",
            forwarded: Some(format!("{OTHER}, {LISTED}")),
            status: 403,
        },
        Case {
            case: "LAST: the listed address is first in the header",
            stage: "last",
            forwarded: Some(format!("{LISTED}, {OTHER}")),
            status: 200,
        },
        Case {
            case: "ANY: the listed address sits in the middle of the header",
            stage: "any",
            forwarded: Some(format!("{OTHER}, {LISTED}, {OTHER}")),
            status: 403,
        },
        Case {
            case: "ANY: the header lists no address of the set",
            stage: "any",
            forwarded: Some(format!("{OTHER}, {OTHER}")),
            status: 200,
        },
        Case {
            case: "NO_MATCH fallback: the request carries no forwarded-for header",
            stage: "last",
            forwarded: None,
            status: 200,
        },
    ];

    for case in &cases {
        let headers: Vec<(&str, &str)> = case
            .forwarded
            .as_deref()
            .map(|value| vec![("x-forwarded-for", value)])
            .unwrap_or_default();
        let answered = edge
            .invoke(
                &api.id,
                "GET",
                &format!("/{}/orders", case.stage),
                &headers,
                &[],
            )
            .await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected status {}, got {} with {:?}",
            case.case, case.status, answered.status, answered.body
        );
    }
}

/// The rule over a MANAGED rule group: its rules run against the request, a
/// `ScopeDownStatement` limits WHICH requests reach them, and the sample WAF stores
/// names the rule within the group that decided.
///
/// One row per payload — an injected SQL fragment in a query argument, in the path, and
/// a benign value — crossed with a web ACL that scopes the group down to a URI prefix
/// and one that does not.
#[tokio::test]
async fn a_managed_rule_group_decides_and_the_sample_names_the_rule_that_did() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-managed").await;
    api.deploy(&edge, "sqli").await;
    api.deploy(&edge, "scoped").await;

    let managed = |scope_down: Option<Value>| {
        let mut statement = json!({
            "VendorName": "AWS",
            "Name": "AWSManagedRulesSQLiRuleSet",
        });
        if let Some(scope_down) = scope_down {
            statement["ScopeDownStatement"] = scope_down;
        }
        json!({ "ManagedRuleGroupStatement": statement })
    };

    // A rule-group rule carries no `Action` of its own: each of the group's rules does,
    // and the web ACL rule says only whether to override them all.
    let group_rule = |name: &str, statement: Value| {
        json!({
            "Name": name,
            "Priority": 0,
            "Statement": statement,
            "OverrideAction": { "None": {} },
            "VisibilityConfig": visibility(name),
        })
    };

    let (sqli_id, sqli_arn, _) = create_web_acl(
        &edge,
        "sqli-everywhere",
        json!({ "Allow": {} }),
        json!([group_rule("sql-database", managed(None))]),
        json!({}),
    )
    .await;
    associate(&edge, &sqli_arn, &api.id, "sqli").await;

    let (_, scoped_arn, _) = create_web_acl(
        &edge,
        "sqli-under-admin",
        json!({ "Allow": {} }),
        json!([group_rule(
            "sql-database",
            managed(Some(byte_match(
                json!({ "UriPath": {} }),
                "/scoped/admin",
                "STARTS_WITH",
            ))),
        )]),
        json!({}),
    )
    .await;
    associate(&edge, &scoped_arn, &api.id, "scoped").await;

    /// One request and the status the managed group earns it.
    struct Case {
        case: &'static str,
        path: &'static str,
        status: u16,
    }

    let cases = [
        Case {
            case: "an injected SQL fragment in a query argument",
            path: "/sqli/orders?q=%27%20OR%201%3D1%20--",
            status: 403,
        },
        Case {
            case: "a benign query argument",
            path: "/sqli/orders?q=alice",
            status: 200,
        },
        Case {
            case: "scoped down to /admin: the fragment on a path inside the scope",
            path: "/scoped/admin?q=%27%20OR%201%3D1%20--",
            status: 403,
        },
        Case {
            case: "scoped down to /admin: the same fragment OUTSIDE the scope, so the \
                   group never runs",
            path: "/scoped/orders?q=%27%20OR%201%3D1%20--",
            status: 200,
        },
    ];

    for case in &cases {
        let answered = edge.get(&api.id, case.path).await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected status {}, got {} with {:?}",
            case.case, case.status, answered.status, answered.body
        );
    }

    // The sample WAF stored for the web ACL rule that ran the group names the rule
    // WITHIN the group that decided, which is what tells an operator which managed
    // rule refused the request.
    let sampled = edge
        .waf_ok(
            "GetSampledRequests",
            json!({
                "WebAclArn": sqli_arn,
                "RuleMetricName": "sql-database",
                "Scope": SCOPE,
                "MaxItems": 50,
                "TimeWindow": { "StartTime": 0, "EndTime": 99_999_999_999_i64 },
            }),
        )
        .await;
    let samples = sampled["SampledRequests"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let blocked: Vec<&Value> = samples
        .iter()
        .filter(|sample| sample["Action"].as_str() == Some("BLOCK"))
        .collect();
    assert!(
        !blocked.is_empty(),
        "case the sample of the rule that blocked: expected at least one BLOCK sample \
         for web acl {sqli_id}, got {sampled}"
    );
    for sample in &blocked {
        let named = sample["RuleNameWithinRuleGroup"].as_str().unwrap_or_default();
        // `SampledHTTPRequest$RuleNameWithinRuleGroup`: "For managed rule groups, the
        // format for this name is <vendor name>#<managed rule group name>#<rule
        // name>" — so the terminating rule is named by the group that holds it, and
        // the group-local SQLi rule is the last segment.
        let segments: Vec<&str> = named.split('#').collect();
        assert!(
            segments.len() == 3
                && segments[0] == "AWS"
                && segments[1] == "AWSManagedRulesSQLiRuleSet"
                && segments[2].starts_with("SQLi"),
            "case the sample of the rule that blocked: expected the group's own SQLi \
             rule named AWS#AWSManagedRulesSQLiRuleSet#<rule>, got {named:?} in {sample}"
        );
    }
}

/// The rule over a RATE-BASED rule: requests are counted per aggregation instance over
/// the rule's evaluation window, and the rule applies its action once an instance is
/// over the `Limit` — and the keys that are currently over it are what
/// `GetRateBasedStatementManagedKeys` reports.
#[tokio::test]
async fn a_rate_based_rule_refuses_the_caller_past_its_limit_and_lists_its_key() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-rate").await;
    api.deploy(&edge, "rated").await;

    /// `RateBasedStatement$Limit` is modelled `range(min 10)`, so the smallest limit a
    /// rule can carry is the one this case drives.
    const LIMIT: usize = 10;

    let (web_acl_id, arn, _) = create_web_acl(
        &edge,
        "rate-limited",
        json!({ "Allow": {} }),
        json!([rule(
            "by-ip",
            0,
            json!({
                "RateBasedStatement": {
                    "Limit": LIMIT,
                    "AggregateKeyType": "IP",
                }
            }),
            json!({ "Block": {} }),
        )]),
        json!({}),
    )
    .await;
    associate(&edge, &arn, &api.id, "rated").await;

    // Every request up to the limit is admitted; the one that takes the instance over
    // it is refused, and so is every later one in the same window.
    for sent in 1..=LIMIT + 2 {
        let answered = edge.get(&api.id, "/rated/orders").await;
        let expected = if sent <= LIMIT { 200 } else { 403 };
        assert_eq!(
            answered.status, expected,
            "case request {sent} of a limit of {LIMIT}: expected status {expected}, got \
             {} with {:?}",
            answered.status, answered.body
        );
    }

    let keys = edge
        .waf_ok(
            "GetRateBasedStatementManagedKeys",
            json!({
                "Scope": SCOPE,
                "WebACLName": "rate-limited",
                "WebACLId": web_acl_id,
                "RuleName": "by-ip",
            }),
        )
        .await;
    let listed = keys["ManagedKeysIPV4"]["Addresses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "case the keys currently over the limit: expected exactly the one caller's \
         address, got {keys}"
    );
    let address = listed[0].as_str().unwrap_or_default();
    assert!(
        address.ends_with("/32"),
        "case the keys currently over the limit: an IPv4 managed key is reported in CIDR \
         notation, got {address:?}"
    );
}

/// The rule over the control-plane REFUSALS a live edge is asserted against: a web ACL
/// that is still associated, a pattern set a rule still references and an update
/// carrying a stale lock token are each refused, and the edge keeps deciding exactly
/// as it did before the refused call.
#[tokio::test]
async fn refused_control_plane_calls_leave_the_edge_deciding_as_it_did() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-control-plane").await;
    api.deploy(&edge, "live").await;

    // A pattern set the web ACL's rule references, so deleting it has to be refused
    // while the rule is live.
    let pattern_set = edge
        .waf_ok(
            "CreateRegexPatternSet",
            json!({
                "Name": "blocked-paths",
                "Scope": SCOPE,
                "RegularExpressionList": [{ "RegexString": "/orders$" }],
            }),
        )
        .await;
    let pattern_arn = pattern_set["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();
    let pattern_id = pattern_set["Summary"]["Id"]
        .as_str()
        .expect("Summary.Id")
        .to_string();
    let pattern_lock = pattern_set["Summary"]["LockToken"]
        .as_str()
        .expect("Summary.LockToken")
        .to_string();

    let (web_acl_id, web_acl_arn, lock_token) = create_web_acl(
        &edge,
        "referencing",
        json!({ "Allow": {} }),
        json!([rule(
            "by-pattern",
            0,
            json!({
                "RegexPatternSetReferenceStatement": {
                    "ARN": pattern_arn,
                    "FieldToMatch": { "UriPath": {} },
                    "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                }
            }),
            json!({ "Block": {} }),
        )]),
        json!({}),
    )
    .await;
    associate(&edge, &web_acl_arn, &api.id, "live").await;

    /// What the edge decides, before and after each refused call: the blocked path and
    /// the allowed one.
    async fn edge_decides(edge: &Edge, api: &str, case: &str) {
        for (path, status) in [("/live/orders", 403), ("/live/admin", 200)] {
            let answered = edge.get(api, path).await;
            assert_eq!(
                answered.status, status,
                "case {case:?}: expected the edge to answer {path} with {status}, got {} \
                 with {:?}",
                answered.status, answered.body
            );
        }
    }

    edge_decides(&edge, &api.id, "before any refused call").await;

    /// One refused control-plane call: the operation, its body and the error code.
    struct Refusal {
        case: &'static str,
        operation: &'static str,
        body: Value,
        code: &'static str,
    }

    let refusals = [
        Refusal {
            case: "deleting a web acl that is still associated with a stage",
            operation: "DeleteWebACL",
            body: json!({
                "Name": "referencing",
                "Scope": SCOPE,
                "Id": web_acl_id,
                "LockToken": lock_token,
            }),
            code: "WAFAssociatedItemException",
        },
        Refusal {
            case: "deleting a pattern set a live rule still references",
            operation: "DeleteRegexPatternSet",
            body: json!({
                "Name": "blocked-paths",
                "Scope": SCOPE,
                "Id": pattern_id,
                "LockToken": pattern_lock,
            }),
            code: "WAFAssociatedItemException",
        },
        Refusal {
            case: "updating the web acl with a stale lock token",
            operation: "UpdateWebACL",
            body: json!({
                "Name": "referencing",
                "Scope": SCOPE,
                "Id": web_acl_id,
                "LockToken": "00000000-0000-4000-8000-000000000000",
                "DefaultAction": { "Block": {} },
                "VisibilityConfig": visibility("referencing"),
                "Rules": [],
            }),
            code: "WAFOptimisticLockException",
        },
    ];

    for refusal in &refusals {
        let (status, parsed) = edge.waf_call(refusal.operation, refusal.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400 {}, got {status} with {parsed}",
            refusal.case, refusal.code
        );
        let reported = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            reported.ends_with(refusal.code),
            "case {:?}: expected the error code {}, got {reported:?} in {parsed}",
            refusal.case,
            refusal.code
        );
        edge_decides(&edge, &api.id, refusal.case).await;
    }
}

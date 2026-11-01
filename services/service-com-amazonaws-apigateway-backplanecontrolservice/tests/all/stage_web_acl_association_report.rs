// SPDX-License-Identifier: CC0-1.0

//! What a STAGE reports about the WAFv2 web ACL in front of it, end to end over the wire.
//!
//! The rule this suite pins: a stage's `webAclArn` is "The ARN of the WebAcl associated
//! with the Stage" (`Stage$webAclArn`) — the web ACL WAF currently has in front of that
//! stage's own ARN. The binding is WAFv2's record (`AssociateWebACL` puts a web ACL in
//! front of the stage's ARN, `DisassociateWebACL` takes it away), so every reader of a
//! stage reports what WAF holds AT THE TIME OF THE READ: the member is absent while no
//! web ACL is associated, names the associated one once there is, names the REPLACEMENT
//! after a second association, and is absent again after a disassociation — and it is
//! keyed by the stage's ARN, so two apis carrying a stage of the same name each report
//! their own.
//!
//! Each test asserts that rule over a TABLE of cases rather than one recorded call: the
//! rows differ along what the rule generalises over — whether a web ACL is associated,
//! which one, which api's stage of that name, and whether the stage existed before the
//! binding was made — and every reader of the `Stage` shape (`GetStage`, `GetStages`,
//! `CreateStage` and `UpdateStage`) is read on each row, because one record cannot be
//! reported two ways. The same cases assert WAF's own reader (`GetWebACLForResource`)
//! agrees, and that the stage the report names is the stage the web ACL DECIDES: the
//! guarded stage answers a blocked request at the edge while its unguarded sibling is
//! served by the origin.
//!
//! Every call goes through the real wire path: the api and its stages are built through
//! API Gateway's own control plane, the web ACLs through WAFv2's, and the invokes are
//! unsigned GETs to the api's own invoke host — both services mounted on ONE shared
//! engine behind the multi-service router, which is what lets one stage read the web ACL
//! the wafv2 control plane stored.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region both control planes are addressed in, and the one every stage ARN below
/// names: a stage's ARN carries the Region its api was created in.
const REGION: &str = "us-east-1";

/// The `Scope` an API Gateway stage's web ACL lives in: a stage is a REGIONAL resource.
const SCOPE: &str = "REGIONAL";

/// The resource path every api below serves, on a `MOCK` integration.
const RESOURCE: &str = "orders";

/// One emulator with API Gateway and WAFv2 mounted on a single shared engine.
struct Edge {
    router: MultiServiceRouter,
}

/// One answered invoke: its status and its body verbatim.
struct Answered {
    status: u16,
    body: String,
}

impl Edge {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests exercise the services, not SigV4: the control-plane requests carry a
        // credential scope so the router can name the target service, and the invokes
        // carry no credential at all, as a client of a published invoke URL sends none.
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

    /// One unsigned GET to a deployed stage's invoke URL, on the api's own host.
    async fn get(&self, api: &str, path: &str) -> Answered {
        let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("https://{host}{path}"))
            .header("host", &host)
            .body(Bytes::new())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(&response.into_body()).into_owned();
        Answered { status, body }
    }
}

/// The ARN `AssociateWebACL` binds a web ACL to a REST API stage by:
/// `arn:aws:apigateway:{region}::/restapis/{api-id}/stages/{stage}`, the one form
/// `AssociateWebACLRequest$ResourceArn` documents for an API Gateway stage.
fn stage_arn(api: &str, stage: &str) -> String {
    format!("arn:aws:apigateway:{REGION}::/restapis/{api}/stages/{stage}")
}

/// A `VisibilityConfig`, which every web ACL and rule declares.
fn visibility(metric: &str) -> Value {
    json!({
        "SampledRequestsEnabled": true,
        "CloudWatchMetricsEnabled": true,
        "MetricName": metric,
    })
}

/// Create a web ACL with `default_action` and no rules, answering its ARN.
async fn create_web_acl(edge: &Edge, name: &str, default_action: Value) -> String {
    let created = edge
        .waf_ok(
            "CreateWebACL",
            json!({
                "Name": name,
                "Scope": SCOPE,
                "DefaultAction": default_action,
                "VisibilityConfig": visibility(name),
                "Rules": [],
            }),
        )
        .await;
    created["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string()
}

/// Put the web ACL `web_acl_arn` in front of `api`'s `stage`.
async fn associate(edge: &Edge, web_acl_arn: &str, api: &str, stage: &str) {
    edge.waf_ok(
        "AssociateWebACL",
        json!({ "WebACLArn": web_acl_arn, "ResourceArn": stage_arn(api, stage) }),
    )
    .await;
}

/// Take the web ACL away from `api`'s `stage`.
async fn disassociate(edge: &Edge, api: &str, stage: &str) {
    edge.waf_ok(
        "DisassociateWebACL",
        json!({ "ResourceArn": stage_arn(api, stage) }),
    )
    .await;
}

/// An api with one `MOCK`-integration resource answering `200 {"at":"<api name>"}`,
/// deployed into `stages`. Answers the api id and the deployment id, so a case can
/// create a further stage of the same deployment.
async fn deployed_api(edge: &Edge, name: &str, stages: &[&str]) -> (String, String) {
    let created = edge
        .api_ok("POST", "/restapis", Some(json!({ "name": name })), 201)
        .await;
    let api = created["id"].as_str().expect("the api id").to_string();
    let root = created["rootResourceId"]
        .as_str()
        .expect("the root resource id")
        .to_string();

    let resource = edge
        .api_ok(
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": RESOURCE })),
            201,
        )
        .await;
    let resource = resource["id"]
        .as_str()
        .expect("the resource id")
        .to_string();
    edge.api_ok(
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    edge.api_ok(
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
        Some(json!({ "type": "MOCK" })),
        201,
    )
    .await;
    edge.api_ok(
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET/integration/responses/200"),
        Some(json!({
            "responseTemplates": { "application/json": origin_body(name) }
        })),
        201,
    )
    .await;

    let mut deployment = String::new();
    for (index, stage) in stages.iter().enumerate() {
        if index == 0 {
            let created = edge
                .api_ok(
                    "POST",
                    &format!("/restapis/{api}/deployments"),
                    Some(json!({ "stageName": stage })),
                    201,
                )
                .await;
            deployment = created["id"]
                .as_str()
                .expect("the deployment id")
                .to_string();
        } else {
            edge.api_ok(
                "POST",
                &format!("/restapis/{api}/stages"),
                Some(json!({ "stageName": stage, "deploymentId": deployment })),
                201,
            )
            .await;
        }
    }
    (api, deployment)
}

/// The body a resource's `MOCK` integration answers with.
fn origin_body(api_name: &str) -> String {
    format!("{{\"at\":\"{api_name}\"}}")
}

/// The `webAclArn` of the member named `member` of `body`, or `None` when the body
/// carries none — which is what a stage with no web ACL in front of it reports.
fn reported(body: &Value) -> Option<String> {
    body["webAclArn"].as_str().map(str::to_string)
}

/// What each reader of one stage reports under `webAclArn`.
#[derive(Debug, PartialEq, Eq)]
struct Reports {
    /// `GetStage`'s own answer.
    get_stage: Option<String>,
    /// The stage's entry in `GetStages`.
    listed: Option<String>,
    /// `UpdateStage`'s answer, for a patch that changes the stage's description.
    updated: Option<String>,
    /// WAFv2's own reader of the same binding, by the stage's ARN.
    web_acl_for_resource: Option<String>,
}

impl Reports {
    /// The four readings every row is asserted over, for `api`'s `stage`.
    async fn of(edge: &Edge, api: &str, stage: &str) -> Self {
        let get_stage = reported(
            &edge
                .api_ok(
                    "GET",
                    &format!("/restapis/{api}/stages/{stage}"),
                    None,
                    200,
                )
                .await,
        );
        let listing = edge
            .api_ok("GET", &format!("/restapis/{api}/stages"), None, 200)
            .await;
        let listed = listing["item"]
            .as_array()
            .expect("GetStages reports its collection")
            .iter()
            .find(|entry| entry["stageName"].as_str() == Some(stage))
            .map(reported)
            .expect("the listing carries the stage");
        let updated = reported(
            &edge
                .api_ok(
                    "PATCH",
                    &format!("/restapis/{api}/stages/{stage}"),
                    Some(json!({
                        "patchOperations": [
                            { "op": "replace", "path": "/description", "value": stage }
                        ]
                    })),
                    200,
                )
                .await,
        );
        let (status, resolved) = edge
            .waf_call(
                "GetWebACLForResource",
                json!({ "ResourceArn": stage_arn(api, stage) }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case GetWebACLForResource {api}/{stage}: expected 200, got {status} with {resolved}"
        );
        let web_acl_for_resource = resolved["WebACL"]["ARN"].as_str().map(str::to_string);
        Self {
            get_stage,
            listed,
            updated,
            web_acl_for_resource,
        }
    }

    /// The reading every reader must agree on: one record, one answer.
    fn of_all(expected: Option<&str>) -> Self {
        let expected = expected.map(str::to_string);
        Self {
            get_stage: expected.clone(),
            listed: expected.clone(),
            updated: expected.clone(),
            web_acl_for_resource: expected,
        }
    }
}

/// The rule over the ASSOCIATION: every reader of a stage reports the web ACL WAF has in
/// front of that stage's ARN at the time of the read — absent while there is none, the
/// associated one once there is, the replacement after a second association, absent again
/// after a disassociation, and never another api's binding of the same stage name.
#[tokio::test]
async fn every_reader_of_a_stage_reports_the_web_acl_waf_has_in_front_of_it() {
    let edge = Edge::new();
    let (api, deployment) = deployed_api(&edge, "reported-association", &["guarded"]).await;
    let (other_api, _) = deployed_api(&edge, "other-association", &["guarded"]).await;
    let first = create_web_acl(&edge, "first-acl", json!({ "Allow": {} })).await;
    let second = create_web_acl(&edge, "second-acl", json!({ "Allow": {} })).await;

    // Row 1: a stage no web ACL is in front of reports none.
    let case = "no web ACL associated";
    let reports = Reports::of(&edge, &api, "guarded").await;
    assert_eq!(
        reports,
        Reports::of_all(None),
        "case {case}: expected every reader to report no web ACL, got {reports:?}"
    );

    // Row 2: the web ACL just associated.
    let case = "associated with the first web ACL";
    associate(&edge, &first, &api, "guarded").await;
    let reports = Reports::of(&edge, &api, "guarded").await;
    assert_eq!(
        reports,
        Reports::of_all(Some(&first)),
        "case {case}: expected every reader to report {first}, got {reports:?}"
    );

    // Row 3: a second association REPLACES the first — a resource has at most one web
    // ACL in front of it.
    let case = "re-associated with a second web ACL";
    associate(&edge, &second, &api, "guarded").await;
    let reports = Reports::of(&edge, &api, "guarded").await;
    assert_eq!(
        reports,
        Reports::of_all(Some(&second)),
        "case {case}: expected every reader to report {second}, got {reports:?}"
    );

    // Row 4: another api's stage of the SAME name carries its own binding, because the
    // binding is keyed by the stage's ARN and that ARN names the api.
    let case = "another api's stage of the same name";
    associate(&edge, &first, &other_api, "guarded").await;
    let reports = Reports::of(&edge, &other_api, "guarded").await;
    assert_eq!(
        reports,
        Reports::of_all(Some(&first)),
        "case {case}: expected every reader to report {first}, got {reports:?}"
    );
    let reports = Reports::of(&edge, &api, "guarded").await;
    assert_eq!(
        reports,
        Reports::of_all(Some(&second)),
        "case {case}: expected the first api's stage to keep reporting {second}, got {reports:?}"
    );

    // Row 5: disassociated — the member is absent again, with nothing else changed.
    let case = "disassociated";
    disassociate(&edge, &api, "guarded").await;
    let reports = Reports::of(&edge, &api, "guarded").await;
    assert_eq!(
        reports,
        Reports::of_all(None),
        "case {case}: expected every reader to report no web ACL, got {reports:?}"
    );

    // Row 6: a stage whose ARN was bound BEFORE the stage was created reports the
    // binding in the answer that creates it: the report is read from WAF's record, not
    // written onto the stage when one is stored.
    let case = "created after its ARN was bound";
    associate(&edge, &first, &api, "fresh").await;
    let created = edge
        .api_ok(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({ "stageName": "fresh", "deploymentId": deployment })),
            201,
        )
        .await;
    let reported_at_create = reported(&created);
    assert_eq!(
        reported_at_create.as_deref(),
        Some(first.as_str()),
        "case {case}: expected CreateStage to report {first}, got {reported_at_create:?}"
    );
    let reports = Reports::of(&edge, &api, "fresh").await;
    assert_eq!(
        reports,
        Reports::of_all(Some(&first)),
        "case {case}: expected every reader to report {first}, got {reports:?}"
    );
}

/// The report and the EDGE are the same record: the stage that reports a web ACL is the
/// stage that web ACL decides, and a stage that reports none is served by its origin.
/// Both stages answer every request — a blocked one at the edge, an allowed one from the
/// integration.
#[tokio::test]
async fn the_stage_that_reports_a_web_acl_is_the_stage_that_web_acl_decides() {
    let edge = Edge::new();
    let (api, _) = deployed_api(&edge, "decided-association", &["guarded", "open"]).await;
    let blocking = create_web_acl(&edge, "blocking-acl", json!({ "Block": {} })).await;
    associate(&edge, &blocking, &api, "guarded").await;

    for (case, stage, expected_report, expected_status) in [
        (
            "the stage the web ACL is in front of",
            "guarded",
            Some(blocking.as_str()),
            403_u16,
        ),
        (
            "a sibling stage of the same api with no web ACL",
            "open",
            None,
            200,
        ),
    ] {
        let stage_body = edge
            .api_ok(
                "GET",
                &format!("/restapis/{api}/stages/{stage}"),
                None,
                200,
            )
            .await;
        let report = reported(&stage_body);
        assert_eq!(
            report.as_deref(),
            expected_report,
            "case {case}: expected webAclArn {expected_report:?}, got {report:?}"
        );
        let answered = edge.get(&api, &format!("/{stage}/{RESOURCE}")).await;
        assert_eq!(
            answered.status, expected_status,
            "case {case}: expected HTTP {expected_status} from the stage, got {} with {:?}",
            answered.status, answered.body
        );
        let origin_reached = answered.body.contains("decided-association");
        assert_eq!(
            origin_reached,
            expected_status == 200,
            "case {case}: expected the origin to be reached only when the web ACL lets \
             the request through, got {:?}",
            answered.body
        );
    }
}

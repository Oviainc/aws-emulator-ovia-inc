// SPDX-License-Identifier: CC0-1.0

//! The ARN a deployed REST stage is protected BY: the stage's ARN in the Region its api
//! was created in, whichever endpoint the invoke arrived at.
//!
//! `AssociateWebACL` binds a web ACL to a stage by that stage's ARN —
//! `arn:aws:apigateway:{region}::/restapis/{api-id}/stages/{stage}`, the one form
//! `AssociateWebACLRequest$ResourceArn` documents for an API Gateway stage — so the data
//! plane finds the binding only if it builds the SAME string. The Region in that string is
//! the api's own, the Region of the request that created the api, and not one read off
//! whichever endpoint a later invoke arrived at: a client on the machine this emulator runs
//! on resolves no `execute-api` endpoint at all, so it addresses the stage on a loopback
//! endpoint whose host spells no Region, and an ARN built from that host would name a stage
//! nothing ever bound — leaving the web ACL in front of the stage unconsulted and the
//! request answered by the origin.
//!
//! Each test asserts that rule over a TABLE rather than one request: the rows differ along
//! the Region the api is created in (including the one a Region-less host was previously
//! defaulted to) and along the four addressings a client can reach a deployed stage by.
//! Every assertion names its case and the actual answer.
//!
//! Every call goes over the real wire path, with API Gateway and WAFv2 mounted on ONE
//! shared engine behind the multi-service router — which is what lets one invoke reach the
//! apigateway data plane and the binding the wafv2 control plane stored.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `Scope` an API Gateway stage's web ACL lives in: a stage is a REGIONAL resource
/// (`Scope`'s `CLOUDFRONT` value is for "a global resource type, such as a Amazon
/// CloudFront distribution").
const SCOPE: &str = "REGIONAL";

/// The endpoint a client on the machine this emulator runs on reaches it at: its host names
/// neither a service nor a Region, which is what every Region-less row below exercises.
const LOOPBACK_HOST: &str = "127.0.0.1:9297";

/// The reserved path prefix the data plane is addressed behind when the host cannot name
/// the api: `/_aws/execute-api/{api-id}/{stage}/{resource-path}`.
const DATA_PLANE_PREFIX: &str = "/_aws/execute-api";

/// The resource whose path the blocking rule searches for, and the one it does not: both
/// are served by the same stage, so an answer says which of the two the web ACL decided.
const BLOCKED_PART: &str = "marker";
const OPEN_PART: &str = "open";

/// One emulator with API Gateway and WAFv2 mounted on a single shared engine.
struct Edge {
    router: MultiServiceRouter,
}

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
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
}

impl Edge {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite exercises the services, not SigV4: the control-plane requests carry a
        // credential scope so the router can name the target service and the Region, and
        // the invokes carry no credential, as a client of a published invoke URL sends
        // none.
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

    /// One API Gateway control-plane call in `region`, answering its status and parsed body.
    async fn api_call(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("apigateway.{region}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/apigateway/aws4_request, \
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
    async fn api_ok(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
        expected: u16,
    ) -> Value {
        let (status, parsed) = self.api_call(region, method, uri, body).await;
        assert_eq!(
            status, expected,
            "case {method} {uri} in {region}: expected {expected}, got {status} with {parsed}"
        );
        parsed
    }

    /// One WAFv2 call in `region`, answering its status and parsed body.
    async fn waf_call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
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
                     Credential=AKIDEXAMPLEWAFV2/20260101/{region}/wafv2/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=00"
                ),
            )
            .body(Bytes::from(body.to_string()))
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

    /// A WAFv2 call that must succeed.
    async fn waf_ok(&self, region: &str, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.waf_call(region, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation} in {region}: expected 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One unsigned GET of a deployed stage, addressed the way `addressing` spells it.
    async fn invoke(
        &self,
        addressing: Addressing,
        api: &str,
        region: &str,
        stage: &str,
        resource: &str,
        headers: &[(&str, &str)],
    ) -> Answered {
        let host = addressing.host(api, region);
        let path = addressing.path(api, stage, resource);
        let mut builder = http::Request::builder()
            .method("GET")
            .uri(format!("https://{host}{path}"))
            .header("host", &host);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::new())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(&response.into_body()).into_owned();
        Answered { status, body }
    }
}

/// How a client addresses a deployed stage. All four spellings reach the same stage; only
/// the first carries a Region in its host.
#[derive(Debug, Clone, Copy)]
enum Addressing {
    /// The published invoke URL `{api-id}.execute-api.{region}.amazonaws.com/{stage}/…`,
    /// whose host names the api and the Region.
    InvokeHost,
    /// The reserved data-plane prefix on an endpoint that names neither:
    /// `/_aws/execute-api/{api-id}/{stage}/…`.
    PathPrefix,
    /// The api id as the first path segment: `/{api-id}/{stage}/…`.
    ApiIdFirst,
    /// The stage segment alone: `/{stage}/…`.
    StageOnly,
}

impl Addressing {
    /// The `Host` the request carries.
    fn host(self, api: &str, region: &str) -> String {
        match self {
            Self::InvokeHost => format!("{api}.execute-api.{region}.amazonaws.com"),
            Self::PathPrefix | Self::ApiIdFirst | Self::StageOnly => LOOPBACK_HOST.to_string(),
        }
    }

    /// The path the request carries, for the resource `/{resource}` of `stage`.
    fn path(self, api: &str, stage: &str, resource: &str) -> String {
        match self {
            Self::InvokeHost | Self::StageOnly => format!("/{stage}/{resource}"),
            Self::PathPrefix => format!("{DATA_PLANE_PREFIX}/{api}/{stage}/{resource}"),
            Self::ApiIdFirst => format!("/{api}/{stage}/{resource}"),
        }
    }
}

/// The ARN `AssociateWebACL` binds a web ACL to a REST API stage by.
fn stage_arn(region: &str, api: &str, stage: &str) -> String {
    format!("arn:aws:apigateway:{region}::/restapis/{api}/stages/{stage}")
}

/// The body a resource's `MOCK` integration answers with.
fn origin_body(path_part: &str) -> String {
    format!("{{\"at\":\"{path_part}\"}}")
}

/// A `Blob` member travels base64-encoded in an awsJson body, which is what
/// `ByteMatchStatement$SearchString` is.
fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// Create an api in `region` with two `MOCK`-backed resources and deploy it to `stage`,
/// answering the api's id.
async fn deploy(edge: &Edge, region: &str, name: &str, stage: &str) -> String {
    let created = edge
        .api_ok(
            region,
            "POST",
            "/restapis",
            Some(json!({ "name": name, "endpointConfiguration": { "types": ["REGIONAL"] } })),
            201,
        )
        .await;
    let id = created["id"].as_str().expect("the api id").to_string();
    let root = created["rootResourceId"]
        .as_str()
        .expect("the root resource id")
        .to_string();

    for path_part in [BLOCKED_PART, OPEN_PART] {
        let resource = edge
            .api_ok(
                region,
                "POST",
                &format!("/restapis/{id}/resources/{root}"),
                Some(json!({ "pathPart": path_part })),
                201,
            )
            .await;
        let resource_id = resource["id"]
            .as_str()
            .expect("the resource id")
            .to_string();
        edge.api_ok(
            region,
            "PUT",
            &format!("/restapis/{id}/resources/{resource_id}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
            201,
        )
        .await;
        edge.api_ok(
            region,
            "PUT",
            &format!("/restapis/{id}/resources/{resource_id}/methods/GET/integration"),
            Some(json!({ "type": "MOCK" })),
            201,
        )
        .await;
        edge.api_ok(
            region,
            "PUT",
            &format!(
                "/restapis/{id}/resources/{resource_id}/methods/GET/integration/responses/200"
            ),
            Some(json!({
                "responseTemplates": { "application/json": origin_body(path_part) }
            })),
            201,
        )
        .await;
    }

    edge.api_ok(
        region,
        "POST",
        &format!("/restapis/{id}/deployments"),
        Some(json!({ "stageName": stage })),
        201,
    )
    .await;
    id
}

/// Create a REGIONAL web ACL in `region` that blocks a request whose URI path contains
/// [`BLOCKED_PART`] and allows every other one, answering its ARN.
async fn blocking_web_acl(edge: &Edge, region: &str, name: &str) -> String {
    let created = edge
        .waf_ok(
            region,
            "CreateWebACL",
            json!({
                "Name": name,
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": {
                    "SampledRequestsEnabled": true,
                    "CloudWatchMetricsEnabled": true,
                    "MetricName": name,
                },
                "Rules": [{
                    "Name": "logblocker",
                    "Priority": 0,
                    "Action": { "Block": {} },
                    "VisibilityConfig": {
                        "SampledRequestsEnabled": true,
                        "CloudWatchMetricsEnabled": true,
                        "MetricName": "logblocker",
                    },
                    "Statement": { "ByteMatchStatement": {
                        "SearchString": base64_of(BLOCKED_PART),
                        "FieldToMatch": { "UriPath": {} },
                        "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                        "PositionalConstraint": "CONTAINS",
                    }},
                }],
            }),
        )
        .await;
    created["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string()
}

/// The rows: the Region the api is created in, paired with the addressing the invoke uses.
///
/// Two of the Regions are not the one a Region-less host used to be defaulted to and one
/// IS, so the row that always passed is kept beside the rows that did not; the addressings
/// are the four a client can reach a deployed stage by, three of which carry no Region at
/// all in their host.
const ROWS: &[(&str, Addressing)] = &[
    ("us-west-2", Addressing::PathPrefix),
    ("us-west-2", Addressing::InvokeHost),
    ("eu-central-1", Addressing::ApiIdFirst),
    ("ap-southeast-2", Addressing::StageOnly),
    ("us-east-1", Addressing::PathPrefix),
];

/// RULE: the web ACL bound to a stage by the stage's ARN decides every request the stage
/// answers, whatever endpoint the request arrived at — so the blocked path is answered
/// `403` and the path the rule does not match still reaches the origin.
#[tokio::test]
async fn a_stage_is_protected_by_its_web_acl_through_every_addressing() {
    let edge = Edge::new();
    for (index, (region, addressing)) in ROWS.iter().enumerate() {
        let case = format!("{region} via {addressing:?}");
        let stage = format!("s{index}");
        let api = deploy(&edge, region, &format!("region-rows-{index}"), &stage).await;

        // Before any web ACL: the deployed stage serves, which is the pre-flight every
        // client does before it puts WAF in front of the stage.
        let served = edge
            .invoke(*addressing, &api, region, &stage, BLOCKED_PART, &[])
            .await;
        assert_eq!(
            served.status, 200,
            "case {case}: expected the deployed stage to answer 200 before any web ACL, \
             got {} with {:?}",
            served.status, served.body
        );
        assert_eq!(
            served.body,
            origin_body(BLOCKED_PART),
            "case {case}: expected the integration's own body, got {:?}",
            served.body
        );

        let web_acl = blocking_web_acl(&edge, region, &format!("rows-{index}")).await;
        edge.waf_ok(
            region,
            "AssociateWebACL",
            json!({
                "WebACLArn": web_acl,
                "ResourceArn": stage_arn(region, &api, &stage),
            }),
        )
        .await;

        // The blocked path, carrying the opaque `authorization` header a client sends
        // when it is checking what WAF does with a sensitive header: the web ACL's
        // terminating `Block` answers it at the edge.
        let blocked = edge
            .invoke(
                *addressing,
                &api,
                region,
                &stage,
                BLOCKED_PART,
                &[("authorization", "opaque-token")],
            )
            .await;
        assert_eq!(
            blocked.status, 403,
            "case {case}: expected the web ACL's Block to answer 403, got {} with {:?}",
            blocked.status, blocked.body
        );
        assert_eq!(
            blocked.message(),
            "Forbidden",
            "case {case}: expected the WAF_FILTERED gateway response's own message, got {:?}",
            blocked.message()
        );

        // The same stage, a path the rule does not match: the web ACL WAS consulted and
        // its `DefaultAction` let this one through, so the origin answers.
        let allowed = edge
            .invoke(*addressing, &api, region, &stage, OPEN_PART, &[])
            .await;
        assert_eq!(
            allowed.status, 200,
            "case {case}: expected the default Allow to reach the origin, got {} with {:?}",
            allowed.status, allowed.body
        );
        assert_eq!(
            allowed.body,
            origin_body(OPEN_PART),
            "case {case}: expected the integration's own body, got {:?}",
            allowed.body
        );
    }
}

/// RULE, from the other side: the ARN names the Region the api was CREATED in, so a web
/// ACL bound by the same stage's ARN written in ANOTHER Region is in front of nothing and
/// the stage answers as it did with no WAF at all.
#[tokio::test]
async fn a_binding_written_in_another_region_protects_nothing() {
    let edge = Edge::new();
    // Rows differ along the Region the api is created in and the Region the binding is
    // mis-written in, including the pair the Region-less default made collide.
    let rows: &[(&str, &str)] = &[
        ("us-west-2", "us-east-1"),
        ("us-east-1", "us-west-2"),
        ("eu-central-1", "eu-west-1"),
    ];
    for (index, (created_in, bound_in)) in rows.iter().enumerate() {
        let case = format!("created in {created_in}, bound in {bound_in}");
        let stage = format!("w{index}");
        let api = deploy(&edge, created_in, &format!("wrong-region-{index}"), &stage).await;
        let web_acl = blocking_web_acl(&edge, created_in, &format!("wrong-{index}")).await;
        edge.waf_ok(
            created_in,
            "AssociateWebACL",
            json!({
                "WebACLArn": web_acl,
                "ResourceArn": stage_arn(bound_in, &api, &stage),
            }),
        )
        .await;

        let served = edge
            .invoke(
                Addressing::PathPrefix,
                &api,
                created_in,
                &stage,
                BLOCKED_PART,
                &[],
            )
            .await;
        assert_eq!(
            served.status, 200,
            "case {case}: a binding on another Region's ARN is in front of nothing, so the \
             stage must still answer 200, got {} with {:?}",
            served.status, served.body
        );
    }
}

/// The surface the issue this suite was written for measures, once the binding is found:
/// WAF logging in front of a web ACL that protects a deployed stage reached through the
/// Region-less data-plane addressing.
///
/// Rows differ along the destination's log-group NAME — the prefix `PutLoggingConfiguration`
/// requires and the same suffix without it — and the assertion is the whole leg: the
/// refusal, the absence of a stored configuration after it, and the round trip of an
/// accepted one.
#[tokio::test]
async fn waf_logging_behind_a_blocked_stage_judges_its_destination() {
    let edge = Edge::new();
    let region = "us-west-2";
    let stage = "slogs";
    let api = deploy(&edge, region, "logging-destination", stage).await;
    let web_acl = blocking_web_acl(&edge, region, "logging-destination").await;
    edge.waf_ok(
        region,
        "AssociateWebACL",
        json!({
            "WebACLArn": web_acl,
            "ResourceArn": stage_arn(region, &api, stage),
        }),
    )
    .await;

    // The stage is behind the web ACL, reached the only way a client on this machine can
    // reach it.
    let blocked = edge
        .invoke(
            Addressing::PathPrefix,
            &api,
            region,
            stage,
            BLOCKED_PART,
            &[("authorization", "opaque-token")],
        )
        .await;
    assert_eq!(
        blocked.status, 403,
        "case blocked marker path: expected 403 from the web ACL, got {} with {:?}",
        blocked.status, blocked.body
    );

    let account = ::engine::emulator_account_id();
    let group = |name: &str| format!("arn:aws:logs:{region}:{account}:log-group:{name}:*");
    let redacted = json!([{ "SingleHeader": { "Name": "authorization" } }]);

    // The destination whose group NAME lacks the prefix WAF delivers under is refused, and
    // nothing is stored: the web ACL still has no logging configuration afterwards.
    let (status, parsed) = edge
        .waf_call(
            region,
            "PutLoggingConfiguration",
            json!({ "LoggingConfiguration": {
                "ResourceArn": web_acl,
                "LogDestinationConfigs": [group("destination-suffix")],
                "RedactedFields": redacted,
            }}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case unprefixed destination: expected 400, got {status} with {parsed}"
    );
    assert_eq!(
        parsed["__type"].as_str().unwrap_or_default(),
        "WAFInvalidParameterException",
        "case unprefixed destination: expected WAFInvalidParameterException, got {parsed}"
    );

    let (status, parsed) = edge
        .waf_call(
            region,
            "GetLoggingConfiguration",
            json!({ "ResourceArn": web_acl }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case no stored configuration: expected 400, got {status} with {parsed}"
    );
    assert_eq!(
        parsed["__type"].as_str().unwrap_or_default(),
        "WAFNonexistentItemException",
        "case no stored configuration: expected WAFNonexistentItemException, got {parsed}"
    );

    // The same suffix UNDER the prefix is accepted and round-trips both members.
    let accepted = group("aws-waf-logs-destination-suffix");
    let stored = edge
        .waf_ok(
            region,
            "PutLoggingConfiguration",
            json!({ "LoggingConfiguration": {
                "ResourceArn": web_acl,
                "LogDestinationConfigs": [accepted.clone()],
                "RedactedFields": redacted,
            }}),
        )
        .await;
    assert_eq!(
        stored["LoggingConfiguration"]["LogDestinationConfigs"],
        json!([accepted]),
        "case prefixed destination: expected the destination to round-trip, got {stored}"
    );
    let read = edge
        .waf_ok(
            region,
            "GetLoggingConfiguration",
            json!({ "ResourceArn": web_acl }),
        )
        .await;
    assert_eq!(
        read["LoggingConfiguration"]["LogDestinationConfigs"],
        json!([accepted]),
        "case prefixed destination: expected the stored destination on the read, got {read}"
    );
    assert_eq!(
        read["LoggingConfiguration"]["RedactedFields"], redacted,
        "case prefixed destination: expected the redacted fields on the read, got {read}"
    );
}

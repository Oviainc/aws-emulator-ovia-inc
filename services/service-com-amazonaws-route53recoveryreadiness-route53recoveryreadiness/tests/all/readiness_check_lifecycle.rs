// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Route 53 Application Recovery Controller readiness core
//! loop: resource sets, and the readiness checks that watch them.
//!
//! Every assertion goes over the wire: a request is built, dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts, and only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected. Nothing reaches into the
//! service's state struct, so a response that is not computed from persisted state
//! cannot pass.
//!
//! The tests are tables. Each row is one input along an axis the rule under test
//! generalises over — resource count, which `Resource` members are present and how deep
//! the `DnsTargetResource` subtree goes, resource-set type, name form and length, tag
//! count, stored record count and page size, and which of the two resource kinds is
//! addressed. The `my_set` / `my_check` case the bootstrap issue recorded is one row.
//!
//! Every emulator here is built for an account that MAY create readiness resources
//! ([`Emulator::new`]): the feature is closed to accounts that do not already use it, so
//! a create is denied account-wide unless the store says this account is eligible. The
//! gate itself is tested in `account_gate_and_not_found.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::{
    service_state, DEFAULT_MAX_RESULTS, MAX_READINESS_SCOPES_PER_RESOURCE, MAX_RECORDS_PER_STORE,
    MAX_RESOURCES_PER_SET, MAX_TAGS_PER_RESOURCE,
};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `MaxResults` is `range 1..1000` on both list operations (model:
/// `smithy_query(query="shape", shape="MaxResults")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// `__stringMax64PatternAAZAZ09Z`, the shape every name this service reports targets,
/// is `length 0..64 pattern ^\A[a-zA-Z0-9_]+\z$` (model:
/// `smithy_query(query="shape", shape="__stringMax64PatternAAZAZ09Z")`).
const NAME_MAX_LENGTH: usize = 64;

/// Resource types the `ResourceSetType` member's own documentation enumerates (model:
/// `smithy_query(query="doc", shape="CreateResourceSetRequest", member="ResourceSetType")`),
/// all of which satisfy its `^AWS::[A-Za-z0-9]+::[A-Za-z0-9]+$` pattern. The issue cited
/// only `AWS::AutoScaling::AutoScalingGroup`; these are entries it did not cite.
const DOCUMENTED_RESOURCE_SET_TYPES: &[&str] = &[
    "AWS::ApiGateway::Stage",
    "AWS::CloudWatch::Alarm",
    "AWS::DynamoDB::Table",
    "AWS::ElasticLoadBalancingV2::LoadBalancer",
    "AWS::Lambda::Function",
    "AWS::MSK::Cluster",
    "AWS::Route53::HealthCheck",
    "AWS::Route53RecoveryReadiness::DNSTargetResource",
    "AWS::SNS::Subscription",
    "AWS::EC2::VPNGateway",
];

/// Which of the two resource kinds this slice stores a request addresses. Every rule
/// below holds for both, so the tables run over this axis rather than over one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    ResourceSet,
    ReadinessCheck,
}

impl Kind {
    const ALL: [Kind; 2] = [Kind::ResourceSet, Kind::ReadinessCheck];

    /// The collection URI, from the model's `@http` bindings.
    fn path(self) -> &'static str {
        match self {
            Kind::ResourceSet => "/resourcesets",
            Kind::ReadinessCheck => "/readinesschecks",
        }
    }

    /// The ARN's resource-type segment.
    fn arn_resource(self) -> &'static str {
        match self {
            Kind::ResourceSet => "resource-set",
            Kind::ReadinessCheck => "readiness-check",
        }
    }

    /// The list response's items member (`@paginated items`).
    fn items_field(self) -> &'static str {
        match self {
            Kind::ResourceSet => "resourceSets",
            Kind::ReadinessCheck => "readinessChecks",
        }
    }

    fn name_field(self) -> &'static str {
        match self {
            Kind::ResourceSet => "resourceSetName",
            Kind::ReadinessCheck => "readinessCheckName",
        }
    }

    fn arn_field(self) -> &'static str {
        match self {
            Kind::ResourceSet => "resourceSetArn",
            Kind::ReadinessCheck => "readinessCheckArn",
        }
    }

    /// The smallest create request that stores a record under `name`.
    fn create_body(self, name: &str) -> Value {
        match self {
            Kind::ResourceSet => json!({
                "resourceSetName": name,
                "resourceSetType": "AWS::DynamoDB::Table",
                "resources": [],
            }),
            Kind::ReadinessCheck => json!({
                "readinessCheckName": name,
                "resourceSetName": "watched_set",
            }),
        }
    }
}

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_type: String,
    pub(crate) body: Value,
}

impl Reply {
    pub(crate) fn field(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|v| !v.is_null())
    }

    pub(crate) fn string(&self, name: &str) -> Option<String> {
        self.field(name).and_then(Value::as_str).map(str::to_string)
    }

    /// The error shape the answer names: the `x-amzn-errortype` header restJson1 sets,
    /// falling back to the body's `__type` (which carries the shape even when the
    /// header had to be omitted).
    pub(crate) fn error_code(&self) -> String {
        let raw = if self.error_type.is_empty() {
            self.string("__type").unwrap_or_default()
        } else {
            self.error_type.clone()
        };
        raw.rsplit(['#', '/']).next().unwrap_or_default().to_string()
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// An emulator whose account MAY create readiness resources — what an account that
    /// already used the feature before it was closed to new accounts has. Everything
    /// about a stored record is only reachable through such an account.
    pub(crate) fn new() -> Self {
        Self::with_account_eligibility(true)
    }

    /// An emulator whose account is eligible to create readiness resources or not.
    ///
    /// The eligibility is a field of this service's store ([`service_state`]), set here
    /// the way `state::ACCOUNT_ELIGIBLE_ENV` sets it at start-up.
    pub(crate) fn with_account_eligibility(eligible: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        service_state(&*engine)
            .expect("this service's state store")
            .lock()
            .account_readiness_eligible = eligible;
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    pub(crate) async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/route53-recovery-readiness/aws4_request, \
             SignedHeaders=host, Signature=deadbeef";
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "route53-recovery-readiness.us-east-1.amazonaws.com")
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create(&self, kind: Kind, body: Value) -> Reply {
        self.send("POST", kind.path(), Some(body)).await
    }

    async fn get(&self, kind: Kind, name: &str) -> Reply {
        self.send("GET", &format!("{}/{name}", kind.path()), None)
            .await
    }

    async fn delete(&self, kind: Kind, name: &str) -> Reply {
        self.send("DELETE", &format!("{}/{name}", kind.path()), None)
            .await
    }

    async fn list(&self, kind: Kind, query: &str) -> Reply {
        let uri = if query.is_empty() {
            kind.path().to_string()
        } else {
            format!("{}?{query}", kind.path())
        };
        self.send("GET", &uri, None).await
    }

    /// Walk every page the list operation offers and return the items it reported, in
    /// the order it reported them, asserting each page honours `maxResults` and that
    /// only the last page omits `nextToken`.
    async fn drain_pages(&self, kind: Kind, max_results: Option<i64>, label: &str) -> Vec<Value> {
        let mut items = Vec::new();
        let mut token: Option<String> = None;
        // More rounds than there can be pages, so a non-terminating cursor fails
        // loudly instead of looping forever.
        for _ in 0..64 {
            let mut query = Vec::new();
            if let Some(max) = max_results {
                query.push(format!("maxResults={max}"));
            }
            if let Some(cursor) = &token {
                query.push(format!("nextToken={cursor}"));
            }
            let page = self.list(kind, &query.join("&")).await;
            assert_eq!(
                page.status, 200,
                "case {label:?}: expected a page to answer HTTP 200, got {} / {}",
                page.status, page.body
            );
            let reported = page
                .field(kind.items_field())
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: expected a {} array, got {}",
                        kind.items_field(),
                        page.body
                    )
                });
            if let Some(max) = max_results {
                assert!(
                    reported.len() as i64 <= max,
                    "case {label:?}: expected at most {max} items on a page, got {}",
                    reported.len()
                );
            }
            items.extend(reported.clone());
            match page.string("nextToken") {
                Some(next) => {
                    assert_eq!(
                        reported.len() as i64,
                        max_results.unwrap_or(DEFAULT_MAX_RESULTS as i64),
                        "case {label:?}: a page that carries nextToken must be full, got {} items",
                        reported.len()
                    );
                    token = Some(next);
                }
                None => return items,
            }
        }
        panic!("case {label:?}: the cursor never terminated after 64 pages");
    }

    /// The list entry reporting `name`, or `None` when no page carries it.
    async fn find_in_list(&self, kind: Kind, name: &str, label: &str) -> Option<Value> {
        self.drain_pages(kind, None, label)
            .await
            .into_iter()
            .find(|item| item.get(kind.name_field()).and_then(Value::as_str) == Some(name))
    }
}

/// The ARN AWS documents for these resources. Route 53 Application Recovery Controller
/// readiness is a GLOBAL service, so the region segment is empty:
/// `arn:aws:route53-recovery-readiness::<account>:<resource-type>/<name>`. Written out
/// here rather than taken from the handler's helper, so the test pins the format.
fn expected_arn(kind: Kind, name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:{resource}/{name}",
        account = engine::EMULATOR_ACCOUNT_ID,
        resource = kind.arn_resource()
    )
}

fn tags_json(tags: &BTreeMap<String, String>) -> Option<Value> {
    if tags.is_empty() {
        return None;
    }
    Some(Value::Object(
        tags.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    ))
}

fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// One resource-set row: the request, and therefore exactly what every read of it must
/// report back.
struct ResourceSetCase {
    label: &'static str,
    name: String,
    resource_set_type: &'static str,
    resources: Value,
    tags: BTreeMap<String, String>,
}

impl ResourceSetCase {
    fn create_body(&self) -> Value {
        let mut body = json!({
            "resourceSetName": self.name,
            "resourceSetType": self.resource_set_type,
            "resources": self.resources,
        });
        if let Some(value) = tags_json(&self.tags) {
            body["tags"] = value;
        }
        body
    }
}

/// Rows differing along what `CreateResourceSet` ranges over: how many resources the
/// set holds, which `Resource` members each carries and how deep the
/// `DnsTargetResource` subtree goes, the resource-set type, the name's form and length
/// (including the published 64-character bound) and the tag count.
fn resource_set_cases() -> Vec<ResourceSetCase> {
    vec![
        ResourceSetCase {
            // The case the bootstrap issue recorded: one row of many.
            label: "the issue's recorded case",
            name: "my_set".to_string(),
            resource_set_type: "AWS::AutoScaling::AutoScalingGroup",
            resources: json!([{
                "resourceArn": "arn:aws:autoscaling:us-east-1:111122223333:autoScalingGroup:uuid:autoScalingGroupName/asg-1",
                "readinessScopes": [],
            }]),
            tags: tags(&[("env", "test")]),
        },
        ResourceSetCase {
            label: "an empty set with no tags",
            name: "s".to_string(),
            resource_set_type: "AWS::DynamoDB::Table",
            resources: json!([]),
            tags: BTreeMap::new(),
        },
        ResourceSetCase {
            label: "three resources with component ids and readiness scopes",
            name: "three_tables".to_string(),
            resource_set_type: "AWS::DynamoDB::Table",
            resources: json!([
                {
                    "resourceArn": "arn:aws:dynamodb:us-east-1:111122223333:table/one",
                    "componentId": "component-one",
                    "readinessScopes": [
                        "arn:aws:route53-recovery-readiness::111122223333:cell/cell-a",
                        "arn:aws:route53-recovery-readiness::111122223333:recovery-group/rg-a",
                    ],
                },
                {
                    "resourceArn": "arn:aws:dynamodb:us-east-1:111122223333:table/two",
                    "readinessScopes": ["arn:aws:route53-recovery-readiness::111122223333:cell/cell-b"],
                },
                {
                    "resourceArn": "arn:aws:dynamodb:us-east-1:111122223333:table/three",
                },
            ]),
            tags: tags(&[("env", "prod"), ("owner", "team")]),
        },
        ResourceSetCase {
            label: "a dns target resource pointing at a network load balancer",
            name: "dns_nlb".to_string(),
            resource_set_type: "AWS::Route53RecoveryReadiness::DNSTargetResource",
            resources: json!([{
                "dnsTargetResource": {
                    "domainName": "www.example.com",
                    "hostedZoneArn": "arn:aws:route53:::hostedzone/Z1111111111111",
                    "recordSetId": "record-set-one",
                    "recordType": "A",
                    "targetResource": {
                        "nLBResource": {
                            "arn": "arn:aws:elasticloadbalancing:us-east-1:111122223333:loadbalancer/net/nlb/1111111111111111",
                        },
                    },
                },
                "readinessScopes": ["arn:aws:route53-recovery-readiness::111122223333:cell/cell-a"],
            }]),
            tags: BTreeMap::new(),
        },
        ResourceSetCase {
            label: "a dns target resource pointing at a route 53 record",
            name: "dns_r53".to_string(),
            resource_set_type: "AWS::Route53RecoveryReadiness::DNSTargetResource",
            resources: json!([{
                "componentId": "component-dns",
                "dnsTargetResource": {
                    "domainName": "api.example.com",
                    "targetResource": {
                        "r53Resource": {
                            "domainName": "api.example.com",
                            "recordSetId": "record-set-two",
                        },
                    },
                },
            }]),
            tags: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
        },
        ResourceSetCase {
            label: "a name at the published 64-character bound",
            name: "n".repeat(NAME_MAX_LENGTH),
            resource_set_type: "AWS::Lambda::Function",
            resources: json!([{
                "resourceArn": "arn:aws:lambda:us-east-1:111122223333:function:failover",
            }]),
            tags: tags(&[("env", "test")]),
        },
    ]
}

fn assert_resource_set_reported(label: &str, reply: &Reply, expected: &ResourceSetCase) {
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("resourceSetArn"),
        Some(expected_arn(Kind::ResourceSet, &expected.name)),
        "case {label:?}: expected the documented ARN, got {:?}",
        reply.string("resourceSetArn")
    );
    assert_eq!(
        reply.string("resourceSetName").as_deref(),
        Some(expected.name.as_str()),
        "case {label:?}: expected the stored name, got {:?}",
        reply.string("resourceSetName")
    );
    assert_eq!(
        reply.string("resourceSetType").as_deref(),
        Some(expected.resource_set_type),
        "case {label:?}: expected the stored type, got {:?}",
        reply.string("resourceSetType")
    );
    assert_eq!(
        reply.field("resources").cloned().unwrap_or(json!([])),
        expected.resources,
        "case {label:?}: expected every submitted resource member preserved, got {:?}",
        reply.field("resources")
    );
    assert_eq!(
        reply.field("tags").cloned(),
        tags_json(&expected.tags),
        "case {label:?}: expected the stored tags, got {:?}",
        reply.field("tags")
    );
}

#[tokio::test]
async fn a_created_resource_set_is_reported_by_get_and_list_exactly_as_stored() {
    let emulator = Emulator::new();

    for expected in resource_set_cases() {
        let label = expected.label;
        let created = emulator
            .create(Kind::ResourceSet, expected.create_body())
            .await;
        assert_resource_set_reported(&format!("{label} (create)"), &created, &expected);

        let fetched = emulator.get(Kind::ResourceSet, &expected.name).await;
        assert_resource_set_reported(&format!("{label} (get)"), &fetched, &expected);

        let listed = emulator
            .find_in_list(Kind::ResourceSet, &expected.name, label)
            .await
            .unwrap_or_else(|| panic!("case {label:?}: expected the set to appear in the list"));
        assert_eq!(
            listed.get("resourceSetArn").and_then(Value::as_str),
            Some(expected_arn(Kind::ResourceSet, &expected.name).as_str()),
            "case {label:?}: expected the list entry to carry the stored ARN, got {listed}"
        );
        assert_eq!(
            listed.get("resourceSetType").and_then(Value::as_str),
            Some(expected.resource_set_type),
            "case {label:?}: expected the list entry to carry the stored type, got {listed}"
        );
        assert_eq!(
            listed.get("resources").cloned().unwrap_or(json!([])),
            expected.resources,
            "case {label:?}: expected the list entry to carry the stored resources, got {listed}"
        );
        assert_eq!(
            listed.get("tags").cloned(),
            tags_json(&expected.tags),
            "case {label:?}: expected the list entry to carry the stored tags, got {listed}"
        );
    }
}

/// One readiness-check row.
struct ReadinessCheckCase {
    label: &'static str,
    name: String,
    resource_set_name: &'static str,
    tags: BTreeMap<String, String>,
}

impl ReadinessCheckCase {
    fn create_body(&self) -> Value {
        let mut body = json!({
            "readinessCheckName": self.name,
            "resourceSetName": self.resource_set_name,
        });
        if let Some(value) = tags_json(&self.tags) {
            body["tags"] = value;
        }
        body
    }
}

/// Rows differing along what `CreateReadinessCheck` ranges over: the name's form and
/// length (including the published 64-character bound), the tag count, and whether the
/// resource set it watches exists — the model declares no `ResourceNotFoundException`
/// for this operation, so a check may name a set that was never created.
fn readiness_check_cases() -> Vec<ReadinessCheckCase> {
    vec![
        ReadinessCheckCase {
            // The case the bootstrap issue recorded.
            label: "the issue's recorded case",
            name: "my_check".to_string(),
            resource_set_name: "my_set",
            tags: tags(&[("env", "test")]),
        },
        ReadinessCheckCase {
            label: "a single-character name with no tags",
            name: "c".to_string(),
            resource_set_name: "my_set",
            tags: BTreeMap::new(),
        },
        ReadinessCheckCase {
            label: "digits, underscores and mixed case",
            name: "Check_42_B".to_string(),
            resource_set_name: "three_tables",
            tags: tags(&[("a", "1"), ("b", "2")]),
        },
        ReadinessCheckCase {
            label: "a name at the published 64-character bound",
            name: "c".repeat(NAME_MAX_LENGTH),
            resource_set_name: "dns_nlb",
            tags: tags(&[("env", "test")]),
        },
        ReadinessCheckCase {
            label: "a check naming a resource set that does not exist",
            name: "orphan_check".to_string(),
            resource_set_name: "never_created_set",
            tags: BTreeMap::new(),
        },
    ]
}

fn assert_readiness_check_reported(label: &str, reply: &Reply, expected: &ReadinessCheckCase) {
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("readinessCheckArn"),
        Some(expected_arn(Kind::ReadinessCheck, &expected.name)),
        "case {label:?}: expected the documented ARN, got {:?}",
        reply.string("readinessCheckArn")
    );
    assert_eq!(
        reply.string("readinessCheckName").as_deref(),
        Some(expected.name.as_str()),
        "case {label:?}: expected the stored name, got {:?}",
        reply.string("readinessCheckName")
    );
    assert_eq!(
        reply.string("resourceSet").as_deref(),
        Some(expected.resource_set_name),
        "case {label:?}: expected the stored resource-set name, got {:?}",
        reply.string("resourceSet")
    );
    assert_eq!(
        reply.field("tags").cloned(),
        tags_json(&expected.tags),
        "case {label:?}: expected the stored tags, got {:?}",
        reply.field("tags")
    );
}

#[tokio::test]
async fn a_created_readiness_check_is_reported_by_get_and_list_exactly_as_stored() {
    let emulator = Emulator::new();

    for expected in readiness_check_cases() {
        let label = expected.label;
        let created = emulator
            .create(Kind::ReadinessCheck, expected.create_body())
            .await;
        assert_readiness_check_reported(&format!("{label} (create)"), &created, &expected);

        let fetched = emulator.get(Kind::ReadinessCheck, &expected.name).await;
        assert_readiness_check_reported(&format!("{label} (get)"), &fetched, &expected);

        let listed = emulator
            .find_in_list(Kind::ReadinessCheck, &expected.name, label)
            .await
            .unwrap_or_else(|| panic!("case {label:?}: expected the check to appear in the list"));
        assert_eq!(
            listed.get("readinessCheckArn").and_then(Value::as_str),
            Some(expected_arn(Kind::ReadinessCheck, &expected.name).as_str()),
            "case {label:?}: expected the list entry to carry the stored ARN, got {listed}"
        );
        assert_eq!(
            listed.get("resourceSet").and_then(Value::as_str),
            Some(expected.resource_set_name),
            "case {label:?}: expected the list entry to carry the watched set, got {listed}"
        );
        assert_eq!(
            listed.get("tags").cloned(),
            tags_json(&expected.tags),
            "case {label:?}: expected the list entry to carry the stored tags, got {listed}"
        );
    }
}

#[tokio::test]
async fn every_documented_resource_set_type_is_accepted_and_reported_back() {
    let emulator = Emulator::new();

    for (index, resource_set_type) in DOCUMENTED_RESOURCE_SET_TYPES.iter().enumerate() {
        let name = format!("typed_set_{index}");
        let created = emulator
            .create(
                Kind::ResourceSet,
                json!({
                    "resourceSetName": name,
                    "resourceSetType": resource_set_type,
                    "resources": [],
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {resource_set_type:?}: expected HTTP 200, got {} / {}",
            created.status, created.body
        );
        let fetched = emulator.get(Kind::ResourceSet, &name).await;
        assert_eq!(
            fetched.string("resourceSetType").as_deref(),
            Some(*resource_set_type),
            "case {resource_set_type:?}: expected the stored type read back, got {:?}",
            fetched.string("resourceSetType")
        );
    }
}

#[tokio::test]
async fn a_resource_set_type_outside_the_modelled_pattern_is_refused() {
    let emulator = Emulator::new();
    // Rows differ in how they break `^AWS::[A-Za-z0-9]+::[A-Za-z0-9]+$`: wrong prefix
    // case, missing prefix, too few segments, and a character outside the class.
    for (index, resource_set_type) in [
        "aws::dynamodb::table",
        "DynamoDB::Table",
        "AWS::DynamoDB",
        "AWS::Dynamo_DB::Table",
        "",
    ]
    .iter()
    .enumerate()
    {
        let reply = emulator
            .create(
                Kind::ResourceSet,
                json!({
                    "resourceSetName": format!("bad_type_{index}"),
                    "resourceSetType": resource_set_type,
                    "resources": [],
                }),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {resource_set_type:?}: expected HTTP 400, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {resource_set_type:?}: expected ValidationException, got {:?} / {}",
            reply.error_code(),
            reply.body
        );
        let fetched = emulator
            .get(Kind::ResourceSet, &format!("bad_type_{index}"))
            .await;
        assert_eq!(
            fetched.status, 404,
            "case {resource_set_type:?}: a refused create must store nothing, got {} / {}",
            fetched.status, fetched.body
        );
    }
}

#[tokio::test]
async fn a_name_outside_the_modelled_pattern_is_refused_and_stores_nothing() {
    // Rows differ in how they break `^\A[a-zA-Z0-9_]+\z$` / `length 0..64`: a space, a
    // dash, a dot, a slash, emptiness, non-ASCII, and one character past the published
    // bound. Each is tried as every name member of each create operation.
    let rejected: Vec<String> = vec![
        "bad name!".to_string(),
        "with-dash".to_string(),
        "with.dot".to_string(),
        "with/slash".to_string(),
        String::new(),
        "ünïcode".to_string(),
        "n".repeat(NAME_MAX_LENGTH + 1),
    ];

    for bad in &rejected {
        for (label, kind, body) in [
            (
                "CreateResourceSet.ResourceSetName",
                Kind::ResourceSet,
                json!({
                    "resourceSetName": bad,
                    "resourceSetType": "AWS::DynamoDB::Table",
                    "resources": [],
                }),
            ),
            (
                "CreateReadinessCheck.ReadinessCheckName",
                Kind::ReadinessCheck,
                json!({
                    "readinessCheckName": bad,
                    "resourceSetName": "watched_set",
                }),
            ),
            (
                "CreateReadinessCheck.ResourceSetName",
                Kind::ReadinessCheck,
                json!({
                    "readinessCheckName": "well_formed_check",
                    "resourceSetName": bad,
                }),
            ),
        ] {
            let emulator = Emulator::new();
            let reply = emulator.create(kind, body).await;
            let case = format!("{label} = {bad:?}");
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?} / {}",
                reply.error_code(),
                reply.body
            );
            let message = reply.string("message").unwrap_or_default();
            assert!(
                message.contains("failed to satisfy constraint"),
                "case {case:?}: expected a constraint message, got {message:?}"
            );
            // A refused request stores nothing at all.
            let listed = emulator.drain_pages(kind, None, &case).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: a refused create must store nothing, got {listed:?}"
            );
        }
    }

    // And the forms the rule admits are accepted, including the published bound.
    for good in [
        "a".to_string(),
        "0".to_string(),
        "_".to_string(),
        "My_Set_42".to_string(),
        "n".repeat(NAME_MAX_LENGTH),
    ] {
        let emulator = Emulator::new();
        for kind in Kind::ALL {
            let reply = emulator.create(kind, kind.create_body(&good)).await;
            assert_eq!(
                reply.status, 200,
                "case {good:?} on {kind:?}: expected HTTP 200, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.string(kind.name_field()).as_deref(),
                Some(good.as_str()),
                "case {good:?} on {kind:?}: expected the name read back, got {:?}",
                reply.string(kind.name_field())
            );
        }
    }
}

#[tokio::test]
async fn a_second_create_of_a_live_name_is_refused_as_a_conflict() {
    for kind in Kind::ALL {
        let emulator = Emulator::new();
        let name = "duplicated_name";
        let first = emulator.create(kind, kind.create_body(name)).await;
        assert_eq!(
            first.status, 200,
            "case {kind:?}: expected the first create to answer HTTP 200, got {} / {}",
            first.status, first.body
        );

        let second = emulator.create(kind, kind.create_body(name)).await;
        assert_eq!(
            second.status, 409,
            "case {kind:?}: expected HTTP 409 for a duplicate name, got {} / {}",
            second.status, second.body
        );
        assert_eq!(
            second.error_code(),
            "ConflictException",
            "case {kind:?}: expected ConflictException, got {:?} / {}",
            second.error_code(),
            second.body
        );

        // The conflict left the first record untouched, and did not store a second.
        let listed = emulator.drain_pages(kind, None, "after conflict").await;
        assert_eq!(
            listed.len(),
            1,
            "case {kind:?}: expected exactly one stored record, got {listed:?}"
        );
        assert_eq!(
            emulator.get(kind, name).await.string(kind.arn_field()),
            Some(expected_arn(kind, name)),
            "case {kind:?}: expected the first record's ARN to survive the conflict"
        );

        // Deleting it frees the name for a fresh create.
        assert_eq!(
            emulator.delete(kind, name).await.status,
            204,
            "case {kind:?}: expected the delete to answer HTTP 204"
        );
        assert_eq!(
            emulator.create(kind, kind.create_body(name)).await.status,
            200,
            "case {kind:?}: expected the freed name to be creatable again"
        );
    }
}

#[tokio::test]
async fn pagination_reports_every_stored_record_exactly_once() {
    // An exhaustive sweep of the bounded domain this rule generalises over: how many
    // records are stored crossed with how large a page is asked for, including both
    // published `MaxResults` bounds and no `MaxResults` at all.
    for kind in Kind::ALL {
        for stored in 0usize..=8 {
            let emulator = Emulator::new();
            let mut expected: Vec<String> = Vec::new();
            for index in 0..stored {
                let name = format!("record_{index}");
                let created = emulator.create(kind, kind.create_body(&name)).await;
                assert_eq!(
                    created.status, 200,
                    "case {kind:?} {stored} stored: expected HTTP 200 creating {name}, got {} / {}",
                    created.status, created.body
                );
                expected.push(name);
            }
            // The store is a BTreeMap, so a list walks names in sorted order.
            expected.sort();

            for max_results in [
                Some(MAX_RESULTS_MIN),
                Some(2),
                Some(3),
                Some(5),
                Some(MAX_RESULTS_MAX),
                None,
            ] {
                let label = format!("{kind:?}, {stored} stored, maxResults={max_results:?}");
                let reported: Vec<String> = emulator
                    .drain_pages(kind, max_results, &label)
                    .await
                    .into_iter()
                    .map(|item| {
                        item.get(kind.name_field())
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                assert_eq!(
                    reported, expected,
                    "case {label:?}: expected every stored record once, in key order, got {reported:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    for kind in Kind::ALL {
        let emulator = Emulator::new();
        emulator.create(kind, kind.create_body("one")).await;

        // One past each published bound of `MaxResults` (range 1..1000).
        for max in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
            let reply = emulator.list(kind, &format!("maxResults={max}")).await;
            assert_eq!(
                reply.status, 400,
                "case {kind:?} maxResults={max}: expected HTTP 400, got {} / {}",
                reply.status, reply.body
            );
        }
        // And each published bound itself is accepted.
        for max in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
            let reply = emulator.list(kind, &format!("maxResults={max}")).await;
            assert_eq!(
                reply.status, 200,
                "case {kind:?} maxResults={max}: expected HTTP 200, got {} / {}",
                reply.status, reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    for kind in Kind::ALL {
        let emulator = Emulator::new();
        for name in ["one", "two"] {
            emulator.create(kind, kind.create_body(name)).await;
        }

        // Tokens differing in why they are not cursors: not hex at all, odd-length hex,
        // hex that decodes to bytes that are not text, and hex of a stored key that the
        // service never minted a cursor for (so it carries no cursor tag).
        for token in [
            "not-a-cursor".to_string(),
            "abc".to_string(),
            "fffe".to_string(),
            hex::encode("one"),
            hex::encode("v2:one"),
        ] {
            let reply = emulator.list(kind, &format!("nextToken={token}")).await;
            assert_eq!(
                reply.status, 400,
                "case {kind:?} {token:?}: expected HTTP 400 for a foreign nextToken, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {kind:?} {token:?}: expected ValidationException, got {:?} / {}",
                reply.error_code(),
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn delete_removes_the_named_record_and_only_that_record() {
    for kind in Kind::ALL {
        let emulator = Emulator::new();
        let names = ["alpha", "beta", "gamma"];
        for name in names {
            emulator.create(kind, kind.create_body(name)).await;
        }

        for (index, name) in names.iter().enumerate() {
            let deleted = emulator.delete(kind, name).await;
            assert_eq!(
                deleted.status, 204,
                "case {kind:?} {name:?}: expected HTTP 204, got {} / {}",
                deleted.status, deleted.body
            );
            assert_eq!(
                deleted.body,
                Value::Null,
                "case {kind:?} {name:?}: expected an empty 204 body, got {}",
                deleted.body
            );

            let fetched = emulator.get(kind, name).await;
            assert_eq!(
                fetched.status, 404,
                "case {kind:?} {name:?}: expected HTTP 404 after the delete, got {} / {}",
                fetched.status, fetched.body
            );

            let remaining: Vec<String> = emulator
                .drain_pages(kind, None, name)
                .await
                .into_iter()
                .map(|item| {
                    item.get(kind.name_field())
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            let expected: Vec<String> = names[index + 1..].iter().map(|n| n.to_string()).collect();
            assert_eq!(
                remaining, expected,
                "case {kind:?} {name:?}: expected only the undeleted records to remain, got {remaining:?}"
            );
        }
    }
}

#[tokio::test]
async fn reading_or_deleting_an_absent_name_answers_resource_not_found() {
    for kind in Kind::ALL {
        let emulator = Emulator::new();
        // A stored neighbour, so the 404 is about the named record and not an empty store.
        emulator.create(kind, kind.create_body("a_neighbour")).await;

        for absent in ["never_created", "A_NEIGHBOUR", "a_neighbour_2"] {
            for method in ["GET", "DELETE"] {
                let reply = match method {
                    "GET" => emulator.get(kind, absent).await,
                    _ => emulator.delete(kind, absent).await,
                };
                let case = format!("{kind:?} {method} {absent:?}");
                assert_eq!(
                    reply.status, 404,
                    "case {case:?}: expected HTTP 404, got {} / {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {:?} / {}",
                    reply.error_code(),
                    reply.body
                );
            }
        }

        // The neighbour is still there: a 404 mutated nothing.
        assert_eq!(
            emulator.get(kind, "a_neighbour").await.status,
            200,
            "case {kind:?}: expected the stored neighbour to survive every 404"
        );
    }
}

#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    for kind in Kind::ALL {
        let emulator = Emulator::new();
        for name in ["one", "two", "three"] {
            emulator.create(kind, kind.create_body(name)).await;
        }

        let first = emulator.drain_pages(kind, None, "first read").await;
        for _ in 0..3 {
            emulator.get(kind, "one").await;
            emulator.list(kind, "maxResults=1").await;
        }
        let last = emulator.drain_pages(kind, None, "last read").await;
        assert_eq!(
            last, first,
            "case {kind:?}: repeated reads must not change what the store reports"
        );
    }
}

#[tokio::test]
async fn a_store_at_its_ceiling_refuses_another_record() {
    // The emulator keeps at most MAX_RECORDS_PER_STORE records per store, so a caller
    // varying the name cannot grow the process without bound. Both stores are checked,
    // since the rule is about either store rather than one of them.
    for kind in Kind::ALL {
        let emulator = Emulator::new();
        for index in 0..MAX_RECORDS_PER_STORE {
            let created = emulator
                .create(kind, kind.create_body(&format!("record_{index:06}")))
                .await;
            assert_eq!(
                created.status, 200,
                "case {kind:?} record {index}: expected HTTP 200 below the ceiling, got {} / {}",
                created.status, created.body
            );
        }

        let refused = emulator
            .create(kind, kind.create_body("one_too_many"))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {kind:?}: a store at {MAX_RECORDS_PER_STORE} records must refuse another, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {kind:?}: expected ValidationException, got {:?} / {}",
            refused.error_code(),
            refused.body
        );
        let message = refused.string("message").unwrap_or_default();
        assert!(
            message.contains(&MAX_RECORDS_PER_STORE.to_string()),
            "case {kind:?}: expected the refusal to name the ceiling, got {message:?}"
        );
        assert_eq!(
            emulator.get(kind, "one_too_many").await.status,
            404,
            "case {kind:?}: the refused create must have stored nothing"
        );

        // Deleting a record makes room for exactly one more.
        assert_eq!(
            emulator.delete(kind, "record_000000").await.status,
            204,
            "case {kind:?}: expected the delete to answer HTTP 204"
        );
        assert_eq!(
            emulator
                .create(kind, kind.create_body("one_too_many"))
                .await
                .status,
            200,
            "case {kind:?}: expected room for one more record after a delete"
        );
    }
}

#[tokio::test]
async fn a_list_with_no_page_size_answers_the_default_page_and_a_cursor() {
    // A caller who names no `maxResults` gets a bounded page, not every record stored.
    for kind in Kind::ALL {
        let emulator = Emulator::new();
        for index in 0..DEFAULT_MAX_RESULTS + 5 {
            emulator
                .create(kind, kind.create_body(&format!("record_{index:06}")))
                .await;
        }

        let page = emulator.list(kind, "").await;
        let reported = page
            .field(kind.items_field())
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            reported, DEFAULT_MAX_RESULTS,
            "case {kind:?}: expected the default page size, got {reported}"
        );
        assert!(
            page.string("nextToken").is_some(),
            "case {kind:?}: expected a cursor for the records the page left out"
        );
    }
}

#[tokio::test]
async fn a_record_larger_than_the_emulator_stores_is_refused() {
    fn resources(count: usize, scopes_each: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|index| {
                    json!({
                        "resourceArn": format!("arn:aws:dynamodb:us-east-1:111122223333:table/t{index}"),
                        "readinessScopes": (0..scopes_each)
                            .map(|scope| format!("arn:aws:route53-recovery-readiness::111122223333:cell/c{scope}"))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect(),
        )
    }
    fn tag_map(count: usize) -> Value {
        Value::Object(
            (0..count)
                .map(|index| (format!("k{index}"), Value::String(format!("v{index}"))))
                .collect(),
        )
    }

    // Rows: each published ceiling, and one past it. The ceilings are the emulator's
    // own (the model constrains none of these collections), so they are read from the
    // constants the handlers enforce rather than copied.
    let rows: Vec<(String, Value, u16)> = vec![
        (
            format!("resources at the ceiling ({MAX_RESOURCES_PER_SET})"),
            resources(MAX_RESOURCES_PER_SET, 0),
            200,
        ),
        (
            format!("resources one past the ceiling ({})", MAX_RESOURCES_PER_SET + 1),
            resources(MAX_RESOURCES_PER_SET + 1, 0),
            400,
        ),
        (
            format!("readiness scopes at the ceiling ({MAX_READINESS_SCOPES_PER_RESOURCE})"),
            resources(1, MAX_READINESS_SCOPES_PER_RESOURCE),
            200,
        ),
        (
            format!(
                "readiness scopes one past the ceiling ({})",
                MAX_READINESS_SCOPES_PER_RESOURCE + 1
            ),
            resources(1, MAX_READINESS_SCOPES_PER_RESOURCE + 1),
            400,
        ),
    ];

    for (index, (label, resources, expected_status)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("sized_set_{index}");
        let reply = emulator
            .create(
                Kind::ResourceSet,
                json!({
                    "resourceSetName": name,
                    "resourceSetType": "AWS::DynamoDB::Table",
                    "resources": resources,
                }),
            )
            .await;
        assert_eq!(
            reply.status, expected_status,
            "case {label:?}: expected HTTP {expected_status}, got {} / {}",
            reply.status, reply.body
        );
        let stored = emulator.get(Kind::ResourceSet, &name).await.status;
        assert_eq!(
            stored,
            if expected_status == 200 { 200 } else { 404 },
            "case {label:?}: expected the store to match the answer, got HTTP {stored}"
        );
    }

    // The tag ceiling holds for both resource kinds.
    for kind in Kind::ALL {
        for (count, expected_status) in [
            (MAX_TAGS_PER_RESOURCE, 200u16),
            (MAX_TAGS_PER_RESOURCE + 1, 400u16),
        ] {
            let emulator = Emulator::new();
            let name = "tagged_record";
            let mut body = kind.create_body(name);
            body["tags"] = tag_map(count);
            let reply = emulator.create(kind, body).await;
            assert_eq!(
                reply.status, expected_status,
                "case {kind:?} with {count} tags: expected HTTP {expected_status}, got {} / {}",
                reply.status, reply.body
            );
            let stored = emulator.get(kind, name).await.status;
            assert_eq!(
                stored,
                if expected_status == 200 { 200 } else { 404 },
                "case {kind:?} with {count} tags: expected the store to match the answer, got HTTP {stored}"
            );
        }
    }
}

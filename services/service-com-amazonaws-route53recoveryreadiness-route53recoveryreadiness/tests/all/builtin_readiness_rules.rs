// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRules`, the built-in readiness-rule catalog.
//!
//! The rule under test: `ListRules` reports the SERVICE's own readiness-rule catalog —
//! for each resource type this service supports, the readiness rules Route 53
//! Application Recovery Controller publishes for that type, complete and in `ruleId`
//! order, every entry carrying that resource type plus a non-empty, unique rule id and a
//! rule description, all within the modelled lengths — narrowed to exactly the requested
//! `resourceType` when one is given, refusing a resource type the service does not
//! support with a 400 `ValidationException`, and paged by `maxResults` with a
//! `nextToken` that resumes where the page stopped. The catalog is reference data, so it
//! is the same on every call and no call changes it.
//!
//! Every assertion goes over the wire, through the same [`MultiServiceRouter`] the
//! production emulator mounts: only the HTTP status, the `x-amzn-errortype` header and
//! the JSON body are inspected, and nothing reaches into the crate's catalog constant, so
//! a response that is not computed from it cannot pass.
//!
//! The tests are tables. The rows differ along what the rule generalises over: which
//! resource type is filtered for (all twenty the model documents, not only the one the
//! issue cited), the FORM of an unsupported filter value, the page size (both published
//! bounds and one past each), and the number of pages a walk takes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::SUPPORTED_RESOURCE_TYPES;
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The resource types `ResourceSetType`'s own documentation enumerates (model:
/// `smithy_query(query="doc", shape="CreateResourceSetRequest", member="ResourceSetType")`).
/// A readiness check over a resource set of one of these types is evaluated by rules of
/// that type, so each one must be represented in the catalog. The issue cited none of
/// them; the sibling divergence report cited only `AWS::SQS::Queue`.
///
/// Written out here rather than taken from the crate's own
/// [`SUPPORTED_RESOURCE_TYPES`], so the test pins the documented values themselves; the
/// first test below also asserts the two agree.
const DOCUMENTED_RESOURCE_TYPES: &[&str] = &[
    "AWS::ApiGateway::Stage",
    "AWS::ApiGatewayV2::Stage",
    "AWS::AutoScaling::AutoScalingGroup",
    "AWS::CloudWatch::Alarm",
    "AWS::EC2::CustomerGateway",
    "AWS::DynamoDB::Table",
    "AWS::EC2::Volume",
    "AWS::ElasticLoadBalancing::LoadBalancer",
    "AWS::ElasticLoadBalancingV2::LoadBalancer",
    "AWS::Lambda::Function",
    "AWS::MSK::Cluster",
    "AWS::RDS::DBCluster",
    "AWS::Route53::HealthCheck",
    "AWS::SQS::Queue",
    "AWS::SNS::Topic",
    "AWS::SNS::Subscription",
    "AWS::EC2::VPC",
    "AWS::EC2::VPNConnection",
    "AWS::EC2::VPNGateway",
    "AWS::Route53RecoveryReadiness::DNSTargetResource",
];

/// `ListRulesOutput$ResourceType` and `$RuleId` target `__stringMax64`,
/// `$RuleDescription` targets `__stringMax256` (model:
/// `smithy_query(query="shape", shape="ListRulesOutput")`).
const RESOURCE_TYPE_MAX_LENGTH: usize = 64;
const RULE_ID_MAX_LENGTH: usize = 64;
const RULE_DESCRIPTION_MAX_LENGTH: usize = 256;

/// `MaxResults` is `range 1..1000` (model:
/// `smithy_query(query="shape", shape="MaxResults")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn field(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|v| !v.is_null())
    }

    fn string(&self, name: &str) -> Option<String> {
        self.field(name).and_then(Value::as_str).map(str::to_string)
    }

    /// The error shape the answer names: the `x-amzn-errortype` header restJson1 sets,
    /// falling back to the body's `__type`.
    fn error_code(&self) -> String {
        let raw = if self.error_type.is_empty() {
            self.string("__type").unwrap_or_default()
        } else {
            self.error_type.clone()
        };
        raw.rsplit(['#', '/']).next().unwrap_or_default().to_string()
    }

    /// The `rules` array, or a panic naming the case: every success must carry one.
    fn rules(&self, case: &str) -> Vec<Value> {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} / {}",
            self.status, self.body
        );
        self.field("rules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a rules array, got {}", self.body))
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn get(&self, uri: &str) -> Reply {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/route53-recovery-readiness/aws4_request, \
             SignedHeaders=host, Signature=deadbeef";
        let request = http::Request::builder()
            .method("GET")
            .uri(uri)
            .header("host", "route53-recovery-readiness.us-east-1.amazonaws.com")
            .header("authorization", authorization)
            .body(Bytes::new())
            .expect("build request");

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

    /// `ListRules` with the given query string (empty for none).
    async fn list_rules(&self, query: &str) -> Reply {
        let uri = if query.is_empty() {
            "/rules".to_string()
        } else {
            format!("/rules?{query}")
        };
        self.get(&uri).await
    }

    /// Every rule `ListRules` reports for `query`, following `nextToken` to the end and
    /// asserting each page honours `max_results` and that only the last page omits the
    /// token.
    async fn drain_rules(&self, query: &str, max_results: Option<i64>, case: &str) -> Vec<Value> {
        let mut rules = Vec::new();
        let mut token: Option<String> = None;
        // More rounds than there can be pages, so a non-terminating cursor fails loudly
        // instead of looping forever.
        for _ in 0..512 {
            let mut parts: Vec<String> = Vec::new();
            if !query.is_empty() {
                parts.push(query.to_string());
            }
            if let Some(max) = max_results {
                parts.push(format!("maxResults={max}"));
            }
            if let Some(cursor) = &token {
                parts.push(format!("nextToken={cursor}"));
            }
            let page = self.list_rules(&parts.join("&")).await;
            let reported = page.rules(case);
            if let Some(max) = max_results {
                assert!(
                    reported.len() as i64 <= max,
                    "case {case:?}: expected at most {max} rules on a page, got {}",
                    reported.len()
                );
            }
            rules.extend(reported.clone());
            match page.string("nextToken") {
                Some(next) => {
                    assert!(
                        !reported.is_empty(),
                        "case {case:?}: a page carrying nextToken must not be empty"
                    );
                    token = Some(next);
                }
                None => return rules,
            }
        }
        panic!("case {case:?}: the cursor never terminated after 512 pages");
    }
}

/// One reported rule, reduced to the three members the model declares.
fn rule_triple(rule: &Value, case: &str) -> (String, String, String) {
    let member = |name: &str| -> String {
        rule.get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a string {name}, got {rule}"))
            .to_string()
    };
    (
        member("resourceType"),
        member("ruleId"),
        member("ruleDescription"),
    )
}

/// The whole catalog as reported over the wire, with every entry checked against the
/// modelled constraints.
async fn reported_catalog(emulator: &Emulator, case: &str) -> Vec<(String, String, String)> {
    let rules = emulator.drain_rules("", None, case).await;
    assert!(
        !rules.is_empty(),
        "case {case:?}: expected a non-empty built-in rule catalog"
    );
    rules
        .iter()
        .map(|rule| {
            let (resource_type, rule_id, description) = rule_triple(rule, case);
            assert!(
                resource_type.len() <= RESOURCE_TYPE_MAX_LENGTH,
                "case {case:?}: resourceType must be at most {RESOURCE_TYPE_MAX_LENGTH} \
                 characters, got {resource_type:?}"
            );
            assert!(
                !rule_id.is_empty() && rule_id.len() <= RULE_ID_MAX_LENGTH,
                "case {case:?}: ruleId must be non-empty and at most {RULE_ID_MAX_LENGTH} \
                 characters, got {rule_id:?}"
            );
            assert!(
                !description.is_empty() && description.len() <= RULE_DESCRIPTION_MAX_LENGTH,
                "case {case:?}: ruleDescription must be non-empty and at most \
                 {RULE_DESCRIPTION_MAX_LENGTH} characters, got {description:?}"
            );
            (resource_type, rule_id, description)
        })
        .collect()
}

#[tokio::test]
async fn every_documented_resource_type_has_built_in_rules() {
    let emulator = Emulator::new();
    let catalog = reported_catalog(&emulator, "unfiltered catalog").await;

    let documented: BTreeSet<&str> = DOCUMENTED_RESOURCE_TYPES.iter().copied().collect();

    // The set the `resourceType` filter accepts is the set the catalog must cover.
    let accepted: BTreeSet<&str> = SUPPORTED_RESOURCE_TYPES.iter().copied().collect();
    assert_eq!(
        accepted, documented,
        "case \"supported types\": the filter's accepted set must be the documented resource \
         types, got {accepted:?}"
    );
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (resource_type, rule_id, _) in &catalog {
        assert!(
            documented.contains(resource_type.as_str()),
            "case {resource_type:?}: every reported rule must apply to a resource type the \
             service supports, got {resource_type:?} for rule {rule_id:?}"
        );
        assert!(
            ids.insert(rule_id.clone()),
            "case {rule_id:?}: rule ids must be unique across the catalog, saw it twice"
        );
        *by_type.entry(resource_type.clone()).or_default() += 1;
    }

    for resource_type in DOCUMENTED_RESOURCE_TYPES {
        let count = by_type.get(*resource_type).copied().unwrap_or(0);
        assert!(
            count > 0,
            "case {resource_type:?}: expected at least one built-in rule for this documented \
             resource type, got {count}"
        );
    }

    // Entries arrive grouped by resource type and ordered by rule id, which is what makes
    // a cursor resumable at a key.
    let keys: Vec<(String, String)> = catalog
        .iter()
        .map(|(resource_type, rule_id, _)| (resource_type.clone(), rule_id.clone()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(
        keys, sorted,
        "case \"catalog order\": expected rules ordered by (resourceType, ruleId), got {keys:?}"
    );
}

#[tokio::test]
async fn the_catalog_is_reference_data_a_read_does_not_change() {
    let emulator = Emulator::new();
    let first = reported_catalog(&emulator, "first read").await;
    let second = reported_catalog(&emulator, "second read").await;
    assert_eq!(
        first, second,
        "case \"repeated read\": the built-in catalog must be identical on every call"
    );
}

#[tokio::test]
async fn resource_type_filter_reports_exactly_that_types_rules() {
    let emulator = Emulator::new();
    let catalog = reported_catalog(&emulator, "unfiltered catalog").await;

    // Every documented type is a row, so the rule is asserted across the whole class and
    // not only for the type the sibling report cited.
    for resource_type in DOCUMENTED_RESOURCE_TYPES {
        let case = format!("resourceType={resource_type}");
        let expected: Vec<(String, String, String)> = catalog
            .iter()
            .filter(|(reported, _, _)| reported == resource_type)
            .cloned()
            .collect();

        let filtered = emulator
            .drain_rules(&format!("resourceType={resource_type}"), None, &case)
            .await;
        let reported: Vec<(String, String, String)> = filtered
            .iter()
            .map(|rule| rule_triple(rule, &case))
            .collect();

        assert!(
            !reported.is_empty(),
            "case {case:?}: expected a non-empty rule list for a supported resource type"
        );
        for (reported_type, rule_id, _) in &reported {
            assert_eq!(
                reported_type, resource_type,
                "case {case:?}: every reported rule must carry the filtered resource type, \
                 got {reported_type:?} for rule {rule_id:?}"
            );
        }
        assert_eq!(
            reported, expected,
            "case {case:?}: the filtered answer must be exactly this type's slice of the \
             catalog, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_unsupported_resource_type_filter_is_refused() {
    let emulator = Emulator::new();

    // Rows differ in the FORM of the unsupported value: a plain word, a wrong casing of a
    // supported type, a supported type with trailing text, a well-formed but unknown
    // `AWS::Service::Type`, and a value in a different namespace.
    let cases = [
        "not_a_valid_type",
        "aws::sqs::queue",
        "AWS::SQS::QueueX",
        "AWS::Fake::Resource",
        "Custom::Thing::Type",
        "AWS::SQS",
    ];
    for value in cases {
        let case = format!("resourceType={value}");
        let reply = emulator.list_rules(&format!("resourceType={value}")).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an unsupported resource-type filter must be refused with HTTP 400, \
             got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected error code ValidationException, got {:?} / {}",
            reply.error_code(),
            reply.body
        );
        assert!(
            reply.field("rules").is_none(),
            "case {case:?}: a refusal must not carry a rule list, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn pagination_walks_the_whole_catalog_at_every_page_size() {
    let emulator = Emulator::new();
    let catalog = reported_catalog(&emulator, "unfiltered catalog").await;

    // Page sizes spanning the published range: both bounds, and sizes in between that make
    // the walk take many pages, one page, and a page that divides the catalog evenly.
    for max_results in [MAX_RESULTS_MIN, 2, 7, 10, MAX_RESULTS_MAX] {
        let case = format!("maxResults={max_results}");
        let walked = emulator.drain_rules("", Some(max_results), &case).await;
        let reported: Vec<(String, String, String)> = walked
            .iter()
            .map(|rule| rule_triple(rule, &case))
            .collect();
        assert_eq!(
            reported, catalog,
            "case {case:?}: walking every page must report the whole catalog once, in order"
        );
    }

    // One page past each bound: the modelled `range 1..1000` is refused at the boundary.
    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let case = format!("maxResults={max_results}");
        let reply = emulator.list_rules(&format!("maxResults={max_results}")).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a page size outside range {MAX_RESULTS_MIN}..{MAX_RESULTS_MAX} must \
             be refused with HTTP 400, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected error code ValidationException, got {:?} / {}",
            reply.error_code(),
            reply.body
        );
    }
}

#[tokio::test]
async fn a_filtered_walk_is_paged_the_same_way() {
    let emulator = Emulator::new();
    let catalog = reported_catalog(&emulator, "unfiltered catalog").await;

    // A page size of one forces a cursor round trip per rule, for every documented type.
    for resource_type in DOCUMENTED_RESOURCE_TYPES {
        let case = format!("resourceType={resource_type}&maxResults=1");
        let expected: Vec<(String, String, String)> = catalog
            .iter()
            .filter(|(reported, _, _)| reported == resource_type)
            .cloned()
            .collect();
        let walked = emulator
            .drain_rules(&format!("resourceType={resource_type}"), Some(1), &case)
            .await;
        let reported: Vec<(String, String, String)> = walked
            .iter()
            .map(|rule| rule_triple(rule, &case))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: a one-rule-per-page walk must report this type's rules once, in order"
        );
    }
}

/// The readiness-rule catalog Route 53 Application Recovery Controller publishes, as
/// `(resourceType, [ruleId, …])` with each type's rules in `ruleId` order.
///
/// SOURCE: the service's published readiness-rule set per resource type. The groups the
/// `ListRules` wire recording pins exactly — `AWS::ApiGateway::Stage` (19 `ApiGwV1*`
/// rules), `AWS::AutoScaling::AutoScalingGroup` (6 `Asg*`), `AWS::CloudWatch::Alarm`
/// (the single `CloudWatchAlarmState`), `AWS::EC2::CustomerGateway` (3), `AWS::EC2::VPNGateway`
/// (4), `AWS::RDS::DBCluster` (7 `Rds*`) and `AWS::SNS::Topic` (5 `SnsTopic*`) — are
/// written out here so the test pins the published identifiers themselves rather than
/// reading the handler's constant, and the rows also cover the types the issue cited no
/// rule for at all (`AWS::SQS::Queue`, the two load-balancer types, `AWS::Route53::HealthCheck`,
/// `AWS::SNS::Subscription`, `AWS::Route53RecoveryReadiness::DNSTargetResource`).
///
/// The rows differ along what the rule generalises over: the resource type, the rule-id
/// prefix the service uses for it, and the per-type cardinality (from 1 rule to 19).
const PUBLISHED_CATALOG: &[(&str, &[&str])] = &[
    (
        "AWS::ApiGateway::Stage",
        &[
            "ApiGwV1ApiKeyCount",
            "ApiGwV1ApiKeySource",
            "ApiGwV1BasePath",
            "ApiGwV1BinaryMediaTypes",
            "ApiGwV1CacheClusterEnabled",
            "ApiGwV1CacheClusterSize",
            "ApiGwV1CacheClusterStatus",
            "ApiGwV1CanarySettings",
            "ApiGwV1ClientCertificate",
            "ApiGwV1EndpointConfiguration",
            "ApiGwV1MethodSettings",
            "ApiGwV1MinimumCompressionSize",
            "ApiGwV1Policy",
            "ApiGwV1Quotas",
            "ApiGwV1ResourceCount",
            "ApiGwV1StageVariables",
            "ApiGwV1ThrottlingBurstLimit",
            "ApiGwV1ThrottlingRateLimit",
            "ApiGwV1TracingEnabled",
        ],
    ),
    (
        "AWS::ApiGatewayV2::Stage",
        &[
            "ApiGwV2AccessLogSettings",
            "ApiGwV2ApiMappings",
            "ApiGwV2AutoDeploy",
            "ApiGwV2ClientCertificate",
            "ApiGwV2DefaultRouteSettings",
            "ApiGwV2DeploymentStatus",
            "ApiGwV2ProtocolType",
            "ApiGwV2Quotas",
            "ApiGwV2RouteCount",
            "ApiGwV2RouteSettings",
            "ApiGwV2StageVariables",
            "ApiGwV2ThrottlingBurstLimit",
            "ApiGwV2ThrottlingRateLimit",
        ],
    ),
    (
        "AWS::AutoScaling::AutoScalingGroup",
        &[
            "AsgAZCount",
            "AsgInstanceSizes",
            "AsgInstanceTypes",
            "AsgMinSizeAndMaxSize",
            "AsgNormalizedCapacity",
            "AsgQuotas",
        ],
    ),
    ("AWS::CloudWatch::Alarm", &["CloudWatchAlarmState"]),
    (
        "AWS::DynamoDB::Table",
        &[
            "DynamoAutoScalingConfiguration",
            "DynamoCapacity",
            "DynamoConfiguration",
            "DynamoGsiCapacity",
            "DynamoGsiConfig",
            "DynamoGsiStatus",
            "DynamoNormalizedCapacity",
            "DynamoQuotas",
            "DynamoTableStatus",
        ],
    ),
    (
        "AWS::EC2::CustomerGateway",
        &[
            "CustomerGatewayIpAddress",
            "CustomerGatewayState",
            "CustomerGatewayVPNType",
        ],
    ),
    (
        "AWS::EC2::VPC",
        &[
            "VpcCidrBlock",
            "VpcCidrBlocksInAssociationSets",
            "VpcQuotas",
            "VpcState",
            "VpcSubnetCount",
            "VpcSubnetState",
        ],
    ),
    (
        "AWS::EC2::VPNConnection",
        &[
            "VpnConnectionsCategory",
            "VpnConnectionsQuotas",
            "VpnConnectionsState",
            "VpnConnectionsTunnelState",
            "VpnConnectionsType",
        ],
    ),
    (
        "AWS::EC2::VPNGateway",
        &[
            "VpnGatewayAsn",
            "VpnGatewayAttachment",
            "VpnGatewayState",
            "VpnGatewayType",
        ],
    ),
    (
        "AWS::EC2::Volume",
        &[
            "EbsVolumeEncryption",
            "EbsVolumeIops",
            "EbsVolumeQuotas",
            "EbsVolumeSize",
            "EbsVolumeState",
            "EbsVolumeType",
        ],
    ),
    (
        "AWS::ElasticLoadBalancing::LoadBalancer",
        &[
            "ElbAZCount",
            "ElbHealthCheck",
            "ElbInstanceCount",
            "ElbListeners",
            "ElbQuotas",
            "ElbScheme",
        ],
    ),
    (
        "AWS::ElasticLoadBalancingV2::LoadBalancer",
        &[
            "ElbV2AZCount",
            "ElbV2IpAddressType",
            "ElbV2Listeners",
            "ElbV2Quotas",
            "ElbV2Scheme",
            "ElbV2TargetGroups",
        ],
    ),
    (
        "AWS::Lambda::Function",
        &[
            "LambdaFunctionDeadLetterConfig",
            "LambdaFunctionEnvironmentVariables",
            "LambdaFunctionMemorySize",
            "LambdaFunctionQuotas",
            "LambdaFunctionRuntime",
            "LambdaFunctionTimeout",
            "LambdaFunctionVpcConfig",
        ],
    ),
    (
        "AWS::MSK::Cluster",
        &[
            "MskClusterACMCertificate",
            "MskClusterBrokerAZDistribution",
            "MskClusterBrokerEBSVolumeSize",
            "MskClusterBrokerInstanceType",
            "MskClusterEncryptionInTransit",
            "MskClusterKafkaVersion",
            "MskClusterNumberOfBrokerNodes",
            "MskClusterQuotas",
            "MskClusterState",
        ],
    ),
    (
        "AWS::RDS::DBCluster",
        &[
            "RdsClusterStatus",
            "RdsEngineMode",
            "RdsEngineVersion",
            "RdsGlobalReplicaLag",
            "RdsInstanceType",
            "RdsNormalizedCapacity",
            "RdsQuotas",
        ],
    ),
    (
        "AWS::Route53::HealthCheck",
        &["Route53HealthCheckConfiguration", "Route53HealthCheckStatus"],
    ),
    (
        "AWS::Route53RecoveryReadiness::DNSTargetResource",
        &["DnsTargetResourceHealthCheck", "DnsTargetResourceRecordSet"],
    ),
    (
        "AWS::SNS::Subscription",
        &[
            "SnsSubscriptionDeliveryPolicy",
            "SnsSubscriptionFilterPolicy",
            "SnsSubscriptionProtocol",
            "SnsSubscriptionRawMessageDelivery",
            "SnsSubscriptionStatus",
        ],
    ),
    (
        "AWS::SNS::Topic",
        &[
            "SnsTopicAwsKmsKey",
            "SnsTopicDeliveryPolicy",
            "SnsTopicDisplayName",
            "SnsTopicQuotas",
            "SnsTopicSubscription",
        ],
    ),
    (
        "AWS::SQS::Queue",
        &[
            "SqsQueueDelaySeconds",
            "SqsQueueEncryption",
            "SqsQueueMessageRetentionPeriod",
            "SqsQueueQuotas",
            "SqsQueueRedrivePolicy",
            "SqsQueueVisibilityTimeout",
        ],
    ),
];

/// The rule ids this emulator used to invent for the catalog — health-check assertions
/// about ONE resource, not the service's readiness rules, which compare resources across
/// cells. None of them is a rule the service publishes, so none may be reported.
const RETIRED_INVENTED_RULE_IDS: &[&str] = &[
    "ApiGatewayStageDeployment",
    "ApiGatewayStageThrottleQuota",
    "ApiGatewayV2StageDeployment",
    "ApiGatewayV2StageThrottleQuota",
    "AutoScalingGroupInServiceInstances",
    "AutoScalingGroupMaxSizeQuota",
    "CloudWatchAlarmStateNotInAlarm",
    "CustomerGatewayStateAvailable",
    "DnsTargetResourceHealthCheckHealthy",
    "DnsTargetResourceRecordSetExists",
    "DynamoDbTableCapacityQuota",
    "DynamoDbTableReplicaActive",
    "DynamoDbTableStatusActive",
    "EbsVolumeAttachmentAttached",
    "EbsVolumeStateAvailableOrInUse",
    "LambdaFunctionConcurrencyQuota",
    "LambdaFunctionNotThrottled",
    "LoadBalancerHealthyInstances",
    "LoadBalancerV2HealthyTargets",
    "LoadBalancerV2StateActive",
    "MskClusterBrokerCount",
    "MskClusterStateActive",
    "RdsDbClusterInstanceCount",
    "RdsDbClusterStatusAvailable",
    "Route53HealthCheckStatusHealthy",
    "SnsSubscriptionConfirmed",
    "SnsTopicSubscriptionCount",
    "SqsQueueDeadLetterTarget",
    "SqsQueueOldestMessageAge",
    "VpcStateAvailable",
    "VpcSubnetCapacity",
    "VpnConnectionStateAvailable",
    "VpnConnectionTunnelsUp",
    "VpnGatewayAttachedToVpc",
    "VpnGatewayStateAvailable",
];

#[tokio::test]
async fn each_resource_type_reports_its_published_rule_set() {
    let emulator = Emulator::new();

    // One row per documented resource type: the filtered answer must be that type's
    // published rule set, complete, in `ruleId` order, and nothing else.
    for (resource_type, expected_ids) in PUBLISHED_CATALOG {
        let case = format!("resourceType={resource_type}");
        let reported: Vec<String> = emulator
            .drain_rules(&format!("resourceType={resource_type}"), None, &case)
            .await
            .iter()
            .map(|rule| rule_triple(rule, &case).1)
            .collect();
        let expected: Vec<String> = expected_ids.iter().map(|id| id.to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected this type's published readiness rules \
             ({} of them), got {reported:?}",
            expected.len()
        );
    }
}

#[tokio::test]
async fn the_published_catalog_covers_every_documented_resource_type() {
    // The table above must itself span the whole documented set, so no type can be left
    // unpinned by the row-per-type test.
    let tabled: BTreeSet<&str> = PUBLISHED_CATALOG.iter().map(|(ty, _)| *ty).collect();
    let documented: BTreeSet<&str> = DOCUMENTED_RESOURCE_TYPES.iter().copied().collect();
    assert_eq!(
        tabled, documented,
        "case \"published catalog coverage\": the published-rule table must cover exactly the \
         documented resource types"
    );
}

#[tokio::test]
async fn the_unfiltered_catalog_is_the_union_of_the_published_rule_sets() {
    let emulator = Emulator::new();
    let catalog = reported_catalog(&emulator, "unfiltered catalog").await;

    let reported: BTreeSet<(String, String)> = catalog
        .iter()
        .map(|(resource_type, rule_id, _)| (resource_type.clone(), rule_id.clone()))
        .collect();
    let expected: BTreeSet<(String, String)> = PUBLISHED_CATALOG
        .iter()
        .flat_map(|(resource_type, ids)| {
            ids.iter()
                .map(move |id| (resource_type.to_string(), id.to_string()))
        })
        .collect();
    assert_eq!(
        reported, expected,
        "case \"unfiltered catalog\": the whole catalog must be the union of every type's \
         published rule set"
    );

    // No rule this emulator once invented may be reported for any type.
    let retired: BTreeSet<&str> = RETIRED_INVENTED_RULE_IDS.iter().copied().collect();
    for (resource_type, rule_id, _) in &catalog {
        assert!(
            !retired.contains(rule_id.as_str()),
            "case {resource_type:?}: {rule_id:?} is not a readiness rule this service \
             publishes, it must not be reported"
        );
    }
}

#[tokio::test]
async fn every_rule_description_states_what_the_rule_inspects() {
    let emulator = Emulator::new();
    let catalog = reported_catalog(&emulator, "unfiltered catalog").await;

    // A readiness rule inspects the resources a resource set names, so every published
    // description is a sentence that says so. Asserted over the whole catalog, every row
    // a different rule.
    for (resource_type, rule_id, description) in &catalog {
        let case = format!("{resource_type}/{rule_id}");
        assert!(
            description.starts_with("Inspects "),
            "case {case:?}: a rule description must say what the rule inspects, \
             got {description:?}"
        );
        assert!(
            description.ends_with('.'),
            "case {case:?}: a rule description must be a sentence, got {description:?}"
        );
    }
}

/// The `(resourceType, ruleId, ruleDescription)` triples a recorded `ListRules` answer
/// carries verbatim — the only two whose exact prose the recording pins. Both are rows
/// of their type's published set above, and each is a different sentence FORM: the
/// API Gateway one compares the stages of a resource set to each other, the CloudWatch
/// one checks each alarm's state.
const RECORDED_RULE_PROSE: &[(&str, &str, &str)] = &[
    (
        "AWS::ApiGateway::Stage",
        "ApiGwV1ApiKeyCount",
        "Inspects all API Gateway stages to ensure that they have same number of API Keys linked to them.",
    ),
    (
        "AWS::CloudWatch::Alarm",
        "CloudWatchAlarmState",
        "Inspects CloudWatch alarms to ensure that each is not in the ALARM or INSUFFICIENT_DATA state.",
    ),
];

#[tokio::test]
async fn the_rule_prose_a_recording_carries_is_reported_verbatim() {
    let emulator = Emulator::new();

    for (resource_type, rule_id, description) in RECORDED_RULE_PROSE {
        let case = format!("{resource_type}/{rule_id}");
        let reported: Vec<(String, String, String)> = emulator
            .drain_rules(&format!("resourceType={resource_type}"), None, &case)
            .await
            .iter()
            .map(|rule| rule_triple(rule, &case))
            .collect();
        let found = reported
            .iter()
            .find(|(_, reported_id, _)| reported_id == rule_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected rule {rule_id:?} to be reported, got {reported:?}")
            });
        assert_eq!(
            (found.0.as_str(), found.1.as_str(), found.2.as_str()),
            (*resource_type, *rule_id, *description),
            "case {case:?}: expected the published rule triple, got {found:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let emulator = Emulator::new();

    // Rows differ in the form of the bogus token: not hex, hex of text this service never
    // tagged, and a token borrowed from nothing at all.
    let cases = ["not-a-valid-token!!", "6e6f742d7461676765642d6b6579", "0"];
    for token in cases {
        let case = format!("nextToken={token}");
        let reply = emulator.list_rules(&format!("nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a token this service did not mint must be refused with HTTP 400, \
             got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected error code ValidationException, got {:?} / {}",
            reply.error_code(),
            reply.body
        );
    }
}

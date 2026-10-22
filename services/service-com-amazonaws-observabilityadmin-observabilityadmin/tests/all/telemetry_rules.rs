// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the telemetry-rule slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. each operation answers from the ONE persisted rule record, so whatever a
//!    create or an update stored is what a later Get or List reports, and whatever
//!    was never stored (or has since been deleted) is a modelled 404;
//! 2. a rule name identifies a rule within the region the request was signed for, so
//!    a second create of the same name there is a 409 and the same name in another
//!    region is a different rule;
//! 3. every member of the stored rule that targets a modelled enum carries one of
//!    that enum's own values, and anything else is a 400;
//! 4. a rule with a multi-region scope is fanned out into the spoke regions that
//!    scope names, reported `PENDING` until the fan-out the create started comes due
//!    and `ACTIVE` after it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::{
    ComAmazonawsObservabilityadminObservabilityadminState, REPLICATION_DELAY_MS,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region most requests below sign for; the ARNs they mint carry exactly this one.
const HOME_REGION: &str = "us-west-2";
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Long enough after a create or update for the fan-out into a spoke region to have
/// come due, whatever scheduling delay the test run adds on top.
fn past_replication_delay() -> Duration {
    Duration::from_millis(REPLICATION_DELAY_MS as u64 + 250)
}

/// One emulator with an empty store, mounted exactly as the router mounts it, plus
/// the shared engine so a test can read the store a handler wrote.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_observabilityadmin_observabilityadmin::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router, engine }
}

/// A request signed for `observabilityadmin` in `region`, which is both the signal
/// the router routes on and where the handlers read the rule's region from.
fn request(region: &str, uri: &str, body: &Value) -> http::Request<bytes::Bytes> {
    http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", format!("observabilityadmin.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDOBSTEST/20260101/{region}/observabilityadmin/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(emulator: &Emulator, region: &str, uri: &str, body: &Value) -> (u16, Value) {
    let response = emulator.router.dispatch(request(region, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
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

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.observabilityadmin#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

async fn create(emulator: &Emulator, region: &str, body: &Value) -> (u16, Value) {
    call(emulator, region, "/CreateTelemetryRule", body).await
}

async fn get(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/GetTelemetryRule",
        &json!({ "RuleIdentifier": identifier }),
    )
    .await
}

async fn update(emulator: &Emulator, region: &str, identifier: &str, rule: &Value) -> (u16, Value) {
    call(
        emulator,
        region,
        "/UpdateTelemetryRule",
        &json!({ "RuleIdentifier": identifier, "Rule": rule }),
    )
    .await
}

async fn delete(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/DeleteTelemetryRule",
        &json!({ "RuleIdentifier": identifier }),
    )
    .await
}

async fn list(emulator: &Emulator, region: &str, body: &Value) -> (u16, Value) {
    call(emulator, region, "/ListTelemetryRules", body).await
}

/// The `RuleArn` a successful create or update reported.
fn rule_arn(body: &Value) -> String {
    body.get("RuleArn")
        .and_then(Value::as_str)
        .unwrap_or("<no RuleArn>")
        .to_string()
}

/// The ARN the service must mint for `name` in `region`.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:observabilityadmin:{region}:{ACCOUNT}:telemetry-rule/{name}")
}

/// Every stored summary, read page by page with `max_results` per page, as
/// `(RuleName, RuleArn)` pairs in the order the pages delivered them. Also asserts
/// no page is larger than it may be and that paging terminates.
async fn paginate(
    emulator: &Emulator,
    region: &str,
    prefix: Option<&str>,
    max_results: Option<i64>,
) -> Vec<(String, String)> {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..64 {
        let mut body = Map::new();
        if let Some(prefix) = prefix {
            body.insert("RuleNamePrefix".to_string(), json!(prefix));
        }
        if let Some(max_results) = max_results {
            body.insert("MaxResults".to_string(), json!(max_results));
        }
        if let Some(token) = &token {
            body.insert("NextToken".to_string(), json!(token));
        }
        let (status, response) = list(emulator, region, &Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "page {page} of ListTelemetryRules(max_results={max_results:?}): \
             expected 200, got {status} {response:?}"
        );
        let summaries = response
            .get("TelemetryRuleSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("page {page}: expected TelemetryRuleSummaries, got {response:?}"))
            .clone();
        if let Some(max_results) = max_results {
            assert!(
                summaries.len() as i64 <= max_results,
                "page {page}: expected at most {max_results} summaries, got {}",
                summaries.len()
            );
        }
        for summary in &summaries {
            collected.push((
                summary
                    .get("RuleName")
                    .and_then(Value::as_str)
                    .unwrap_or("<no RuleName>")
                    .to_string(),
                summary
                    .get("RuleArn")
                    .and_then(Value::as_str)
                    .unwrap_or("<no RuleArn>")
                    .to_string(),
            ));
        }
        token = response
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return collected;
        }
    }
    panic!("ListTelemetryRules never stopped handing out a NextToken");
}

/// One rule body to store and read back. The rows differ along what the round-trip
/// rule generalises over: which resource and telemetry type the rule names, how many
/// telemetry source types it carries, which destination parameters it nests, and
/// whether the optional scalar members are present at all.
fn round_trip_rules() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required TelemetryType",
            json!({ "TelemetryType": "Traces" }),
        ),
        (
            "VPC flow logs with a full destination configuration",
            json!({
                "ResourceType": "AWS::EC2::VPC",
                "TelemetryType": "Logs",
                "TelemetrySourceTypes": ["VPC_FLOW_LOGS"],
                "DestinationConfiguration": {
                    "DestinationType": "cloud-watch-logs",
                    "DestinationPattern": "/aws/vpc/<resourceId>",
                    "RetentionInDays": 30,
                    "VPCFlowLogParameters": {
                        "LogFormat": "${version} ${vpc-id}",
                        "TrafficType": "ALL",
                        "MaxAggregationInterval": 600
                    }
                },
                "SelectionCriteria": "ResourceTags.team=observability",
                "AllowFieldUpdates": true
            }),
        ),
        (
            "EKS cluster with two telemetry source types and no destination",
            json!({
                "ResourceType": "AWS::EKS::Cluster",
                "TelemetryType": "Logs",
                "TelemetrySourceTypes": ["EKS_AUDIT_LOGS", "EKS_API_LOGS"],
                "AllowFieldUpdates": false
            }),
        ),
        (
            "metrics rule with an empty source type list and a scope",
            json!({
                "ResourceType": "AWS::MSK::Cluster",
                "TelemetryType": "Metrics",
                "TelemetrySourceTypes": [],
                "Scope": "ou-1234",
                "DestinationConfiguration": {
                    "MskMonitoringParameters": { "EnhancedMonitoring": "PER_TOPIC_PER_BROKER" }
                }
            }),
        ),
        (
            "WAF logging with a nested logging filter",
            json!({
                "ResourceType": "AWS::WAFv2::WebACL",
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "DestinationType": "cloud-watch-logs",
                    "WAFLoggingParameters": {
                        "LogType": "WAF_LOGS",
                        "RedactedFields": [{ "SingleHeader": { "Name": "authorization" } }],
                        "LoggingFilter": {
                            "DefaultBehavior": "KEEP",
                            "Filters": [{
                                "Behavior": "DROP",
                                "Requirement": "MEETS_ANY",
                                "Conditions": [
                                    { "ActionCondition": { "Action": "CAPTCHA" } },
                                    { "LabelNameCondition": { "LabelName": "awswaf:managed" } }
                                ]
                            }]
                        }
                    }
                }
            }),
        ),
        (
            "insights rule for a Bedrock knowledge base with log delivery",
            json!({
                "ResourceType": "AWS::Bedrock::KnowledgeBase",
                "TelemetryType": "Insights",
                "DestinationConfiguration": {
                    "LogDeliveryParameters": { "LogTypes": ["APPLICATION_LOGS", "USAGE_LOGS"] }
                }
            }),
        ),
    ]
}

#[tokio::test]
async fn a_created_rule_is_what_every_later_read_reports() {
    let emulator = emulator();

    for (index, (case, rule)) in round_trip_rules().into_iter().enumerate() {
        let name = format!("round-trip-{index}");
        let (status, created) = create(
            &emulator,
            HOME_REGION,
            &json!({ "RuleName": name, "Rule": rule, "Tags": { "team": "obs" } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateTelemetryRule to answer 200, got {status} {created:?}"
        );
        let arn = rule_arn(&created);
        assert_eq!(
            arn,
            expected_arn(HOME_REGION, &name),
            "case {case:?}: CreateTelemetryRule minted the wrong ARN"
        );

        // The identifier resolves as a bare name AND as the full ARN, and both must
        // report the same persisted record.
        for identifier in [name.clone(), arn.clone()] {
            let (status, got) = get(&emulator, HOME_REGION, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetTelemetryRule({identifier}) to answer 200, \
                 got {status} {got:?}"
            );
            assert_eq!(
                got.get("RuleName").and_then(Value::as_str),
                Some(name.as_str()),
                "case {case:?}: GetTelemetryRule({identifier}) reported the wrong RuleName: {got:?}"
            );
            assert_eq!(
                got.get("RuleArn").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case:?}: GetTelemetryRule({identifier}) reported the wrong RuleArn: {got:?}"
            );
            assert_eq!(
                got.get("TelemetryRule"),
                Some(&rule),
                "case {case:?}: GetTelemetryRule({identifier}) did not report the rule that was \
                 created: {got:?}"
            );
            assert_eq!(
                got.get("HomeRegion").and_then(Value::as_str),
                Some(HOME_REGION),
                "case {case:?}: GetTelemetryRule({identifier}) reported the wrong HomeRegion: {got:?}"
            );
            assert_eq!(
                got.get("IsReplicated").and_then(Value::as_bool),
                Some(false),
                "case {case:?}: a rule the caller created is not a replica: {got:?}"
            );
            // The rule carries no multi-region scope, so the member does not apply.
            assert_eq!(
                got.get("RegionStatuses"),
                None,
                "case {case:?}: a single-region rule reports no RegionStatuses: {got:?}"
            );
            let created_at = got
                .get("CreatedTimeStamp")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let updated_at = got
                .get("LastUpdateTimeStamp")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            // Epoch milliseconds: any real clock is far past this, and a create sets
            // both stamps to the same instant.
            assert!(
                created_at > 1_600_000_000_000,
                "case {case:?}: expected an epoch-millisecond CreatedTimeStamp, got {created_at}"
            );
            assert_eq!(
                created_at, updated_at,
                "case {case:?}: a create sets LastUpdateTimeStamp to CreatedTimeStamp"
            );
        }

        // And the same record is what the list reports a summary of.
        let summaries = paginate(&emulator, HOME_REGION, Some(&name), None).await;
        assert_eq!(
            summaries,
            vec![(name.clone(), arn.clone())],
            "case {case:?}: ListTelemetryRules did not report exactly the created rule"
        );
    }
}

#[tokio::test]
async fn a_summary_reports_the_stored_rule_body() {
    let emulator = emulator();
    let rule = json!({
        "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
        "TelemetryType": "Logs",
        "TelemetrySourceTypes": ["ROUTE53_RESOLVER_QUERY_LOGS"],
        "DestinationConfiguration": {
            "ELBLoadBalancerLoggingParameters": { "OutputFormat": "json" }
        }
    });
    let (status, created) = create(
        &emulator,
        HOME_REGION,
        &json!({ "RuleName": "elb-logs", "Rule": rule }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");

    let (status, listed) = list(&emulator, HOME_REGION, &json!({})).await;
    assert_eq!(status, 200, "expected 200, got {status} {listed:?}");
    let summary = listed
        .get("TelemetryRuleSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| summaries.first())
        .unwrap_or_else(|| panic!("expected one summary, got {listed:?}"))
        .clone();
    let (_, got) = get(&emulator, HOME_REGION, "elb-logs").await;
    assert_eq!(
        summary.get("ResourceType"),
        rule.get("ResourceType"),
        "the summary's ResourceType comes from the stored rule: {summary:?}"
    );
    assert_eq!(
        summary.get("TelemetryType"),
        rule.get("TelemetryType"),
        "the summary's TelemetryType comes from the stored rule: {summary:?}"
    );
    assert_eq!(
        summary.get("TelemetrySourceTypes"),
        rule.get("TelemetrySourceTypes"),
        "the summary's TelemetrySourceTypes come from the stored rule: {summary:?}"
    );
    assert_eq!(
        summary.get("CreatedTimeStamp"),
        got.get("CreatedTimeStamp"),
        "the summary's CreatedTimeStamp is the stored one: {summary:?}"
    );
    assert_eq!(
        summary.get("LastUpdateTimeStamp"),
        got.get("LastUpdateTimeStamp"),
        "the summary's LastUpdateTimeStamp is the stored one: {summary:?}"
    );
}

#[tokio::test]
async fn a_second_create_of_one_name_conflicts_and_changes_nothing() {
    let emulator = emulator();
    let first = json!({ "ResourceType": "AWS::EC2::VPC", "TelemetryType": "Logs" });
    let (status, _) = create(
        &emulator,
        HOME_REGION,
        &json!({ "RuleName": "vpc-flow-logs", "Rule": first }),
    )
    .await;
    assert_eq!(status, 200, "expected the first create to answer 200");

    for (case, body) in [
        (
            "the same rule body again",
            json!({ "RuleName": "vpc-flow-logs", "Rule": first }),
        ),
        (
            "a different rule body under the same name",
            json!({
                "RuleName": "vpc-flow-logs",
                "Rule": { "ResourceType": "AWS::EKS::Cluster", "TelemetryType": "Metrics" }
            }),
        ),
    ] {
        let (status, response) = create(&emulator, HOME_REGION, &body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {response:?}"
        );
    }

    let (_, got) = get(&emulator, HOME_REGION, "vpc-flow-logs").await;
    assert_eq!(
        got.get("TelemetryRule"),
        Some(&first),
        "a refused create must not have overwritten the stored rule: {got:?}"
    );
}

/// The enum catalogs below are the generated `*_VALUES` constants, which carry
/// exactly what `smithy_query(query="shape",shape="<Enum>")` reports for each enum
/// the `TelemetryRule` closure reaches.
#[tokio::test]
async fn every_enum_member_accepts_its_own_values_and_refuses_anything_else() {
    let emulator = emulator();

    // One accepted row per enum member — each using a value the issue did not cite —
    // and one refused row for the same member.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("TelemetryType=Logs", json!({ "TelemetryType": "Logs" }), true),
        (
            "TelemetryType=Metrics",
            json!({ "TelemetryType": "Metrics" }),
            true,
        ),
        (
            "TelemetryType=Traces",
            json!({ "TelemetryType": "Traces" }),
            true,
        ),
        (
            "TelemetryType=Insights",
            json!({ "TelemetryType": "Insights" }),
            true,
        ),
        (
            "TelemetryType=logs (wrong case)",
            json!({ "TelemetryType": "logs" }),
            false,
        ),
        (
            "ResourceType=AWS::XRay::TransactionSearchConfig",
            json!({
                "TelemetryType": "Traces",
                "ResourceType": "AWS::XRay::TransactionSearchConfig"
            }),
            true,
        ),
        (
            "ResourceType=AWS::EC2::Subnet (not a value)",
            json!({ "TelemetryType": "Logs", "ResourceType": "AWS::EC2::Subnet" }),
            false,
        ),
        (
            "TelemetrySourceTypes=[EKS_SCHEDULER_LOGS]",
            json!({
                "TelemetryType": "Logs",
                "TelemetrySourceTypes": ["EKS_SCHEDULER_LOGS"]
            }),
            true,
        ),
        (
            "TelemetrySourceTypes second entry not a value",
            json!({
                "TelemetryType": "Logs",
                "TelemetrySourceTypes": ["EKS_SCHEDULER_LOGS", "EKS_NODE_LOGS"]
            }),
            false,
        ),
        (
            "DestinationType=cloud-watch-logs",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": { "DestinationType": "cloud-watch-logs" }
            }),
            true,
        ),
        (
            "DestinationType=s3 (not a value)",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": { "DestinationType": "s3" }
            }),
            false,
        ),
        (
            "ELB OutputFormat=plain",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "ELBLoadBalancerLoggingParameters": { "OutputFormat": "plain" }
                }
            }),
            true,
        ),
        (
            "ELB OutputFormat=csv (not a value)",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "ELBLoadBalancerLoggingParameters": { "OutputFormat": "csv" }
                }
            }),
            false,
        ),
        (
            "LogTypes=[SECURITY_FINDING_LOGS]",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "LogDeliveryParameters": { "LogTypes": ["SECURITY_FINDING_LOGS"] }
                }
            }),
            true,
        ),
        (
            "LogTypes=[SYSTEM_LOGS] (not a value)",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "LogDeliveryParameters": { "LogTypes": ["SYSTEM_LOGS"] }
                }
            }),
            false,
        ),
        (
            "MSK EnhancedMonitoring=PER_TOPIC_PER_PARTITION",
            json!({
                "TelemetryType": "Metrics",
                "DestinationConfiguration": {
                    "MskMonitoringParameters": { "EnhancedMonitoring": "PER_TOPIC_PER_PARTITION" }
                }
            }),
            true,
        ),
        (
            "MSK EnhancedMonitoring=ALL (not a value)",
            json!({
                "TelemetryType": "Metrics",
                "DestinationConfiguration": {
                    "MskMonitoringParameters": { "EnhancedMonitoring": "ALL" }
                }
            }),
            false,
        ),
        (
            "WAF LogType=WAF_LOGS",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "WAFLoggingParameters": { "LogType": "WAF_LOGS" }
                }
            }),
            true,
        ),
        (
            "WAF LogType=ALL_LOGS (not a value)",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "WAFLoggingParameters": { "LogType": "ALL_LOGS" }
                }
            }),
            false,
        ),
        (
            "WAF logging filter with values from every nested enum",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "WAFLoggingParameters": { "LoggingFilter": {
                        "DefaultBehavior": "DROP",
                        "Filters": [{
                            "Behavior": "KEEP",
                            "Requirement": "MEETS_ALL",
                            "Conditions": [{ "ActionCondition": { "Action": "EXCLUDED_AS_COUNT" } }]
                        }]
                    } }
                }
            }),
            true,
        ),
        (
            "WAF logging filter DefaultBehavior=SKIP (not a value)",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "WAFLoggingParameters": { "LoggingFilter": { "DefaultBehavior": "SKIP" } }
                }
            }),
            false,
        ),
        (
            "WAF logging filter Requirement=MEETS_SOME (not a value)",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "WAFLoggingParameters": { "LoggingFilter": { "Filters": [{
                        "Behavior": "KEEP",
                        "Requirement": "MEETS_SOME"
                    }] } }
                }
            }),
            false,
        ),
        (
            "WAF logging filter Action=DENY (not a value)",
            json!({
                "TelemetryType": "Logs",
                "DestinationConfiguration": {
                    "WAFLoggingParameters": { "LoggingFilter": { "Filters": [{
                        "Behavior": "KEEP",
                        "Conditions": [{ "ActionCondition": { "Action": "DENY" } }]
                    }] } }
                }
            }),
            false,
        ),
    ];

    for (index, (case, rule, accepted)) in rows.into_iter().enumerate() {
        let name = format!("enum-{index}");
        let (status, response) = create(
            &emulator,
            HOME_REGION,
            &json!({ "RuleName": name, "Rule": rule }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {response:?}"
            );
            let (status, got) = get(&emulator, HOME_REGION, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the accepted rule to be readable back, got {status}"
            );
            assert_eq!(
                got.get("TelemetryRule"),
                Some(&rule),
                "case {case:?}: the stored rule must be the one supplied: {got:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response:?}"
            );
            let (status, got) = get(&emulator, HOME_REGION, &name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused create must store nothing, got {status} {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_rule_name_outside_its_modelled_form_is_refused() {
    let emulator = emulator();
    let rule = json!({ "TelemetryType": "Logs" });

    // `RuleName` is modelled len 1..100, pattern `^[0-9A-Za-z-_.#/]+$`: each bound
    // and one past it, plus a character the pattern excludes.
    let rows: Vec<(&str, Value, u16)> = vec![
        ("one character, the lower bound", json!("a"), 200),
        ("100 characters, the upper bound", json!("b".repeat(100)), 200),
        ("empty, one below the lower bound", json!(""), 400),
        (
            "101 characters, one past the upper bound",
            json!("c".repeat(101)),
            400,
        ),
        ("a space, which the pattern excludes", json!("bad name!"), 400),
        (
            "every character the pattern allows",
            json!("Rule-0_9.#/name"),
            200,
        ),
    ];

    for (case, name, expected) in rows {
        let (status, response) =
            create(&emulator, HOME_REGION, &json!({ "RuleName": name, "Rule": rule })).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {response:?}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response:?}"
            );
        }
    }

    // Nothing the walk refused reached the store: only the three accepted names did.
    let stored: Vec<String> = paginate(&emulator, HOME_REGION, None, None)
        .await
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        stored,
        vec![
            "Rule-0_9.#/name".to_string(),
            "a".to_string(),
            "b".repeat(100)
        ],
        "only the accepted names may be stored"
    );
}

#[tokio::test]
async fn an_update_replaces_the_body_and_keeps_the_rule_s_identity() {
    let emulator = emulator();
    let created_rule = json!({
        "ResourceType": "AWS::EC2::VPC",
        "TelemetryType": "Logs",
        "TelemetrySourceTypes": ["VPC_FLOW_LOGS"],
        "DestinationConfiguration": { "DestinationType": "cloud-watch-logs", "RetentionInDays": 7 }
    });
    let (status, created) = create(
        &emulator,
        HOME_REGION,
        &json!({ "RuleName": "updated", "Rule": created_rule, "Tags": { "team": "obs" } }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");
    let arn = rule_arn(&created);
    let (_, before) = get(&emulator, HOME_REGION, "updated").await;
    let created_at = before
        .get("CreatedTimeStamp")
        .and_then(Value::as_i64)
        .expect("a stored CreatedTimeStamp");

    // One update per row, each changing the body along a different axis; after each,
    // the read must report that body, the same ARN and CreatedTimeStamp, and a
    // LastUpdateTimeStamp no earlier than the one before.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a changed destination configuration",
            json!({
                "ResourceType": "AWS::EC2::VPC",
                "TelemetryType": "Logs",
                "TelemetrySourceTypes": ["VPC_FLOW_LOGS"],
                "DestinationConfiguration": {
                    "DestinationType": "cloud-watch-logs",
                    "RetentionInDays": 3653,
                    "DestinationPattern": "/aws/vpc/<accountId>"
                }
            }),
        ),
        (
            "a different telemetry type and resource type",
            json!({ "ResourceType": "AWS::Lambda::Function", "TelemetryType": "Metrics" }),
        ),
        (
            "down to the required member only",
            json!({ "TelemetryType": "Traces" }),
        ),
        (
            "identifying the rule by its ARN rather than its name",
            json!({ "TelemetryType": "Insights", "SelectionCriteria": "ResourceTags.tier=gold" }),
        ),
    ];

    let mut previous_update = created_at;
    for (index, (case, rule)) in rows.into_iter().enumerate() {
        let identifier = if case.starts_with("identifying") {
            arn.clone()
        } else {
            "updated".to_string()
        };
        let (status, updated) = update(&emulator, HOME_REGION, &identifier, &rule).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {updated:?}"
        );
        assert_eq!(
            rule_arn(&updated),
            arn,
            "case {case:?}: an update answers the rule's existing ARN"
        );

        let (status, got) = get(&emulator, HOME_REGION, "updated").await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");
        assert_eq!(
            got.get("TelemetryRule"),
            Some(&rule),
            "case {case:?}: the read must report the updated body: {got:?}"
        );
        assert_eq!(
            got.get("CreatedTimeStamp").and_then(Value::as_i64),
            Some(created_at),
            "case {case:?}: an update leaves CreatedTimeStamp alone: {got:?}"
        );
        let updated_at = got
            .get("LastUpdateTimeStamp")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert!(
            updated_at >= previous_update,
            "case {case:?}: expected LastUpdateTimeStamp at or after {previous_update}, \
             got {updated_at}"
        );
        previous_update = updated_at;

        // The tags belong to the resource, not to the body an update replaces.
        let tags = emulator
            .engine
            .state::<ComAmazonawsObservabilityadminObservabilityadminState>()
            .lock()
            .telemetry_rules
            .get(&(HOME_REGION.to_string(), "updated".to_string()))
            .map(|record| record.tags.clone())
            .unwrap_or_default();
        assert_eq!(
            tags.get("team").map(String::as_str),
            Some("obs"),
            "case {case:?} (update {index}): an update leaves the stored tags alone, got {tags:?}"
        );
    }

    // A body the enum rule refuses is refused here too, and changes nothing.
    let (status, response) = update(
        &emulator,
        HOME_REGION,
        "updated",
        &json!({ "TelemetryType": "Everything" }),
    )
    .await;
    assert_eq!(status, 400, "expected 400, got {status} {response:?}");
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "expected ValidationException, got {response:?}"
    );
    let (_, got) = get(&emulator, HOME_REGION, "updated").await;
    assert_eq!(
        got.get("TelemetryRule")
            .and_then(|rule| rule.get("TelemetryType"))
            .and_then(Value::as_str),
        Some("Insights"),
        "a refused update must leave the stored body alone: {got:?}"
    );
}

#[tokio::test]
async fn a_deleted_rule_is_gone_for_every_later_reader() {
    let emulator = emulator();
    let rule = json!({ "ResourceType": "AWS::S3::Bucket", "TelemetryType": "Logs" });
    let (status, created) = create(
        &emulator,
        HOME_REGION,
        &json!({ "RuleName": "doomed", "Rule": rule }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");
    let arn = rule_arn(&created);

    let (status, response) = delete(&emulator, HOME_REGION, "doomed").await;
    assert_eq!(
        status, 200,
        "expected DeleteTelemetryRule to answer 200, got {status} {response:?}"
    );
    assert_eq!(
        response,
        Value::Null,
        "DeleteTelemetryRule answers an empty body, got {response:?}"
    );

    for identifier in ["doomed".to_string(), arn] {
        let (status, got) = get(&emulator, HOME_REGION, &identifier).await;
        assert_eq!(
            status, 404,
            "case {identifier:?}: expected 404 after the delete, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            "ResourceNotFoundException",
            "case {identifier:?}: expected ResourceNotFoundException, got {got:?}"
        );
        let (status, response) = delete(&emulator, HOME_REGION, &identifier).await;
        assert_eq!(
            status, 404,
            "case {identifier:?}: a second delete is a 404, got {status} {response:?}"
        );
    }

    assert!(
        paginate(&emulator, HOME_REGION, None, None).await.is_empty(),
        "ListTelemetryRules must no longer report a deleted rule"
    );
}

#[tokio::test]
async fn an_unknown_identifier_is_a_modelled_404() {
    let emulator = emulator();
    let (status, _) = create(
        &emulator,
        HOME_REGION,
        &json!({ "RuleName": "present", "Rule": { "TelemetryType": "Logs" } }),
    )
    .await;
    assert_eq!(status, 200, "expected the create to answer 200");

    // Each row names a rule that is not stored in the caller's region; an ARN has to
    // match in EVERY field, so one differing anywhere is not found.
    let rows: Vec<(&str, String)> = vec![
        ("a name nothing was created under", "absent".to_string()),
        (
            "the stored rule's name in another region",
            "present".to_string(),
        ),
        (
            "the stored rule's ARN read from another region",
            expected_arn(HOME_REGION, "present"),
        ),
        (
            "an ARN naming another account",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:123456789012:telemetry-rule/present"),
        ),
        (
            "an ARN naming another service",
            format!("arn:aws:logs:{HOME_REGION}:{ACCOUNT}:telemetry-rule/present"),
        ),
        (
            "an ARN naming another region",
            expected_arn("eu-west-1", "present"),
        ),
    ];

    for (case, identifier) in rows {
        // Every row but the first two is read from the home region; the two that are
        // about another region are read from there.
        let region = if case.contains("another region") {
            "eu-west-1"
        } else {
            HOME_REGION
        };
        let (status, got) = get(&emulator, region, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {got:?}"
        );
        for (operation, response) in [
            ("UpdateTelemetryRule", update(&emulator, region, &identifier, &json!({ "TelemetryType": "Logs" })).await),
            ("DeleteTelemetryRule", delete(&emulator, region, &identifier).await),
        ] {
            let (status, body) = response;
            assert_eq!(
                status, 404,
                "case {case:?}: expected {operation} to answer 404, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected {operation} to answer ResourceNotFoundException, \
                 got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn one_name_in_two_regions_is_two_rules() {
    let emulator = emulator();
    let rows: Vec<(&str, Value)> = vec![
        (
            HOME_REGION,
            json!({ "ResourceType": "AWS::EC2::VPC", "TelemetryType": "Logs" }),
        ),
        (
            "eu-west-1",
            json!({ "ResourceType": "AWS::EKS::Cluster", "TelemetryType": "Metrics" }),
        ),
        (
            "ap-southeast-2",
            json!({ "ResourceType": "AWS::Lambda::Function", "TelemetryType": "Traces" }),
        ),
    ];

    for (region, rule) in &rows {
        let (status, created) = create(
            &emulator,
            region,
            &json!({ "RuleName": "shared", "Rule": rule }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} {created:?}"
        );
        assert_eq!(
            rule_arn(&created),
            expected_arn(region, "shared"),
            "case {region:?}: the ARN carries the region the request was signed for"
        );
    }

    for (region, rule) in &rows {
        let (status, got) = get(&emulator, region, "shared").await;
        assert_eq!(status, 200, "case {region:?}: expected 200, got {status}");
        assert_eq!(
            got.get("TelemetryRule"),
            Some(rule),
            "case {region:?}: each region reports its own rule: {got:?}"
        );
        assert_eq!(
            got.get("HomeRegion").and_then(Value::as_str),
            Some(*region),
            "case {region:?}: the home region is the one it was created in: {got:?}"
        );
        let listed = paginate(&emulator, region, None, None).await;
        assert_eq!(
            listed,
            vec![("shared".to_string(), expected_arn(region, "shared"))],
            "case {region:?}: a list reports only the rules of its own region"
        );
    }

    // Deleting one leaves the others where they were.
    let (status, _) = delete(&emulator, "eu-west-1", "shared").await;
    assert_eq!(status, 200, "expected the delete to answer 200");
    let (status, _) = get(&emulator, "eu-west-1", "shared").await;
    assert_eq!(status, 404, "the deleted rule is gone in its own region");
    for region in [HOME_REGION, "ap-southeast-2"] {
        let (status, _) = get(&emulator, region, "shared").await;
        assert_eq!(
            status, 200,
            "case {region:?}: another region's delete must not reach this rule"
        );
    }
}

#[tokio::test]
async fn pagination_walks_every_rule_once_and_terminates() {
    let emulator = emulator();
    // Seven rules under two prefixes; `BTreeMap` ordering makes the expected page
    // sequence the names in lexicographic order.
    let names: Vec<String> = ["alpha", "beta", "delta", "gamma", "omega"]
        .iter()
        .map(|name| format!("rule-{name}"))
        .chain(["other-1", "other-2"].iter().map(|name| (*name).to_string()))
        .collect();
    for name in &names {
        let (status, response) = create(
            &emulator,
            HOME_REGION,
            &json!({ "RuleName": name, "Rule": { "TelemetryType": "Logs" } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "creating {name}: expected 200, got {status} {response:?}"
        );
    }
    let mut all: Vec<(String, String)> = names
        .iter()
        .map(|name| (name.clone(), expected_arn(HOME_REGION, name)))
        .collect();
    all.sort();

    // Every page size from one rule at a time to more than there are rules, plus the
    // modelled upper bound and no `MaxResults` at all, must deliver the same
    // sequence exactly once.
    for max_results in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(100)] {
        let walked = paginate(&emulator, HOME_REGION, None, max_results).await;
        assert_eq!(
            walked, all,
            "case MaxResults={max_results:?}: paging must walk every rule exactly once"
        );
    }

    // `RuleNamePrefix` narrows the walk, at every page size.
    let with_prefix: Vec<(String, String)> = all
        .iter()
        .filter(|(name, _)| name.starts_with("rule-"))
        .cloned()
        .collect();
    for max_results in [None, Some(1), Some(4)] {
        let walked = paginate(&emulator, HOME_REGION, Some("rule-"), max_results).await;
        assert_eq!(
            walked, with_prefix,
            "case prefix=rule-, MaxResults={max_results:?}: the prefix filter must hold"
        );
    }
    assert!(
        paginate(&emulator, HOME_REGION, Some("nothing-"), None)
            .await
            .is_empty(),
        "a prefix nothing matches reports no summaries and no token"
    );

    // `MaxResults` is modelled range 1..100: each bound, and one past each.
    for (case, max_results, expected) in [
        ("the lower bound", 1, 200),
        ("the upper bound", 100, 200),
        ("one below the lower bound", 0, 400),
        ("one past the upper bound", 101, 400),
    ] {
        let (status, response) =
            list(&emulator, HOME_REGION, &json!({ "MaxResults": max_results })).await;
        assert_eq!(
            status, expected,
            "case {case:?} (MaxResults={max_results}): expected {expected}, got {status} {response:?}"
        );
    }

    // A token this operation never issued is refused, not served.
    for (case, token) in [
        ("not hexadecimal at all", "not-a-token"),
        ("hex that decodes to an impossible rule name", "20"),
        ("hex that decodes to no name at all", ""),
    ] {
        let (status, response) =
            list(&emulator, HOME_REGION, &json!({ "NextToken": token })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }
}

#[tokio::test]
async fn a_multi_region_rule_is_pending_in_its_spokes_until_the_fan_out_comes_due() {
    let emulator = emulator();
    let spokes = ["eu-west-1", "ap-southeast-2"];
    let rule = json!({
        "ResourceType": "AWS::EC2::VPC",
        "TelemetryType": "Logs",
        "Regions": spokes,
    });
    let (status, created) = create(
        &emulator,
        HOME_REGION,
        &json!({ "RuleName": "fanned-out", "Rule": rule }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");

    // Straight after the create the fan-out is not due, so every spoke is PENDING
    // with no ARN yet and the spoke region holds nothing to read.
    let (_, got) = get(&emulator, HOME_REGION, "fanned-out").await;
    let statuses = got
        .get("RegionStatuses")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a multi-region rule reports RegionStatuses, got {got:?}"))
        .clone();
    assert_eq!(
        statuses.len(),
        spokes.len(),
        "one status per spoke region, got {statuses:?}"
    );
    for status in &statuses {
        assert_eq!(
            status.get("Status").and_then(Value::as_str),
            Some("PENDING"),
            "before the fan-out comes due a spoke is PENDING: {status:?}"
        );
        assert_eq!(
            status.get("RuleArn"),
            None,
            "a PENDING spoke reports no rule ARN: {status:?}"
        );
    }
    assert_eq!(
        got.get("IsReplicated").and_then(Value::as_bool),
        Some(false),
        "the rule the caller created is not itself a replica: {got:?}"
    );
    for spoke in spokes {
        let (status, body) = get(&emulator, spoke, "fanned-out").await;
        assert_eq!(
            status, 404,
            "case {spoke:?}: a replica does not exist before the fan-out is due, \
             got {status} {body:?}"
        );
    }

    tokio::time::sleep(past_replication_delay()).await;

    // Once it is due, each spoke is ACTIVE and carries the ARN of the rule in that
    // region — which is a record that region can now read.
    let (_, got) = get(&emulator, HOME_REGION, "fanned-out").await;
    let statuses = got
        .get("RegionStatuses")
        .and_then(Value::as_array)
        .expect("RegionStatuses")
        .clone();
    let mut reported: Vec<(String, String, String)> = statuses
        .iter()
        .map(|status| {
            (
                status
                    .get("Region")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                status
                    .get("Status")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                status
                    .get("RuleArn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    reported.sort();
    let mut expected: Vec<(String, String, String)> = spokes
        .iter()
        .map(|spoke| {
            (
                (*spoke).to_string(),
                "ACTIVE".to_string(),
                expected_arn(spoke, "fanned-out"),
            )
        })
        .collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "once the fan-out is due every spoke is ACTIVE with its own rule ARN"
    );

    for spoke in spokes {
        let (status, replica) = get(&emulator, spoke, "fanned-out").await;
        assert_eq!(
            status, 200,
            "case {spoke:?}: expected the replica to be readable, got {status} {replica:?}"
        );
        assert_eq!(
            replica.get("TelemetryRule"),
            Some(&rule),
            "case {spoke:?}: the replica carries the rule body that was fanned out: {replica:?}"
        );
        assert_eq!(
            replica.get("RuleArn").and_then(Value::as_str),
            Some(expected_arn(spoke, "fanned-out").as_str()),
            "case {spoke:?}: the replica's ARN is the one of its own region: {replica:?}"
        );
        assert_eq!(
            replica.get("IsReplicated").and_then(Value::as_bool),
            Some(true),
            "case {spoke:?}: a fan-out replica reports IsReplicated: {replica:?}"
        );
        assert_eq!(
            replica.get("HomeRegion").and_then(Value::as_str),
            Some(HOME_REGION),
            "case {spoke:?}: the replica names the home region it came from: {replica:?}"
        );
        assert_eq!(
            replica.get("RegionStatuses"),
            None,
            "case {spoke:?}: a replica was not created with multi-region scope: {replica:?}"
        );

        // "Replicated rules cannot be directly updated or deleted in the spoke
        // region": the update conflicts with the resource's state, and the delete is
        // refused with the client-input error that operation models.
        let (status, response) =
            update(&emulator, spoke, "fanned-out", &json!({ "TelemetryType": "Metrics" })).await;
        assert_eq!(
            status, 409,
            "case {spoke:?}: expected 409 updating a replica, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {spoke:?}: expected ConflictException, got {response:?}"
        );
        let (status, response) = delete(&emulator, spoke, "fanned-out").await;
        assert_eq!(
            status, 400,
            "case {spoke:?}: expected 400 deleting a replica, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {spoke:?}: expected ValidationException, got {response:?}"
        );
    }

    // Deleting the rule in its home region reclaims every replica with it.
    let (status, _) = delete(&emulator, HOME_REGION, "fanned-out").await;
    assert_eq!(status, 200, "expected the home delete to answer 200");
    for region in [HOME_REGION, spokes[0], spokes[1]] {
        let (status, body) = get(&emulator, region, "fanned-out").await;
        assert_eq!(
            status, 404,
            "case {region:?}: the home delete removes the replicas too, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn an_update_brings_the_fan_out_in_line_with_the_new_scope() {
    let emulator = emulator();
    let (status, _) = create(
        &emulator,
        HOME_REGION,
        &json!({
            "RuleName": "rescoped",
            "Rule": { "TelemetryType": "Logs", "Regions": ["eu-west-1", "eu-central-1"] }
        }),
    )
    .await;
    assert_eq!(status, 200, "expected the create to answer 200");
    tokio::time::sleep(past_replication_delay()).await;

    // Each row replaces the scope; after it, exactly the regions it names hold a
    // replica of this rule.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one spoke dropped, one kept",
            json!({ "TelemetryType": "Logs", "Regions": ["eu-west-1"] }),
            vec!["eu-west-1"],
        ),
        (
            "a spoke added beside the kept one",
            json!({ "TelemetryType": "Metrics", "Regions": ["eu-west-1", "sa-east-1"] }),
            vec!["eu-west-1", "sa-east-1"],
        ),
        (
            "the scope dropped entirely",
            json!({ "TelemetryType": "Metrics" }),
            vec![],
        ),
    ];

    for (case, rule, expected_spokes) in rows {
        let (status, response) = update(&emulator, HOME_REGION, "rescoped", &rule).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        tokio::time::sleep(past_replication_delay()).await;

        let (_, got) = get(&emulator, HOME_REGION, "rescoped").await;
        let reported: Vec<String> = got
            .get("RegionStatuses")
            .and_then(Value::as_array)
            .map(|statuses| {
                let mut regions: Vec<String> = statuses
                    .iter()
                    .map(|status| {
                        format!(
                            "{}={}",
                            status.get("Region").and_then(Value::as_str).unwrap_or(""),
                            status.get("Status").and_then(Value::as_str).unwrap_or("")
                        )
                    })
                    .collect();
                regions.sort();
                regions
            })
            .unwrap_or_default();
        let mut expected: Vec<String> = expected_spokes
            .iter()
            .map(|region| format!("{region}=ACTIVE"))
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: the reported spokes must be the ones the new scope names"
        );

        for region in ["eu-west-1", "eu-central-1", "sa-east-1"] {
            let (status, body) = get(&emulator, region, "rescoped").await;
            let expected_status = if expected_spokes.contains(&region) {
                200
            } else {
                404
            };
            assert_eq!(
                status, expected_status,
                "case {case:?}, region {region:?}: expected {expected_status}, \
                 got {status} {body:?}"
            );
            if expected_status == 200 {
                assert_eq!(
                    body.get("TelemetryRule"),
                    Some(&rule),
                    "case {case:?}, region {region:?}: the replica carries the updated body: \
                     {body:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn all_regions_fans_out_across_the_home_region_s_partition() {
    let emulator = emulator();
    let (status, created) = create(
        &emulator,
        HOME_REGION,
        &json!({
            "RuleName": "everywhere",
            "Rule": { "TelemetryType": "Logs", "AllRegions": true }
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");
    tokio::time::sleep(past_replication_delay()).await;

    let (_, got) = get(&emulator, HOME_REGION, "everywhere").await;
    let regions: Vec<String> = got
        .get("RegionStatuses")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("AllRegions reports RegionStatuses, got {got:?}"))
        .iter()
        .map(|status| {
            status
                .get("Region")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();

    // Commercial regions other than the home one are in; the home region and the
    // other partitions are not.
    for present in ["eu-west-1", "ap-southeast-2", "sa-east-1", "us-east-1"] {
        assert!(
            regions.iter().any(|region| region == present),
            "case {present:?}: expected it among the fan-out regions, got {regions:?}"
        );
    }
    for absent in [HOME_REGION, "cn-north-1", "us-gov-west-1", "us-iso-east-1"] {
        assert!(
            !regions.iter().any(|region| region == absent),
            "case {absent:?}: expected it NOT among the fan-out regions, got {regions:?}"
        );
    }
    let (status, replica) = get(&emulator, "eu-west-1", "everywhere").await;
    assert_eq!(
        status, 200,
        "an AllRegions rule is readable in a commercial spoke, got {status} {replica:?}"
    );
    let (status, outside) = get(&emulator, "cn-north-1", "everywhere").await;
    assert_eq!(
        status, 404,
        "an AllRegions rule does not reach another partition, got {status} {outside:?}"
    );
}

#[tokio::test]
async fn a_fan_out_into_a_region_whose_name_is_taken_reports_failed() {
    let emulator = emulator();
    let occupant = json!({ "ResourceType": "AWS::EKS::Cluster", "TelemetryType": "Metrics" });
    let (status, _) = create(
        &emulator,
        "eu-west-1",
        &json!({ "RuleName": "contested", "Rule": occupant }),
    )
    .await;
    assert_eq!(status, 200, "expected the occupying create to answer 200");

    let (status, _) = create(
        &emulator,
        HOME_REGION,
        &json!({
            "RuleName": "contested",
            "Rule": { "TelemetryType": "Logs", "Regions": ["eu-west-1", "sa-east-1"] }
        }),
    )
    .await;
    assert_eq!(status, 200, "expected the fanned-out create to answer 200");
    tokio::time::sleep(past_replication_delay()).await;

    let (_, got) = get(&emulator, HOME_REGION, "contested").await;
    let mut reported: Vec<String> = got
        .get("RegionStatuses")
        .and_then(Value::as_array)
        .expect("RegionStatuses")
        .iter()
        .map(|status| {
            format!(
                "{}={}",
                status.get("Region").and_then(Value::as_str).unwrap_or(""),
                status.get("Status").and_then(Value::as_str).unwrap_or("")
            )
        })
        .collect();
    reported.sort();
    assert_eq!(
        reported,
        vec!["eu-west-1=FAILED".to_string(), "sa-east-1=ACTIVE".to_string()],
        "a spoke whose name was already taken reports FAILED, the free one ACTIVE"
    );
    assert!(
        got.get("RegionStatuses")
            .and_then(Value::as_array)
            .expect("RegionStatuses")
            .iter()
            .any(|status| status.get("FailureReason").and_then(Value::as_str).is_some()),
        "a FAILED spoke says why: {got:?}"
    );

    // The rule that was already there is untouched: it is nobody's replica.
    let (status, held) = get(&emulator, "eu-west-1", "contested").await;
    assert_eq!(status, 200, "expected 200, got {status} {held:?}");
    assert_eq!(
        held.get("TelemetryRule"),
        Some(&occupant),
        "the fan-out must not overwrite a rule that was already there: {held:?}"
    );
    assert_eq!(
        held.get("IsReplicated").and_then(Value::as_bool),
        Some(false),
        "a rule the fan-out could not replace is not a replica: {held:?}"
    );
}

#[tokio::test]
async fn an_unusable_multi_region_scope_is_refused() {
    let emulator = emulator();

    // `Regions`' and `AllRegions`' own documentation: each is "mutually exclusive"
    // with the other. A named region must also be a region of the home region's own
    // partition, and the list may be no longer than the fan-out this service will
    // perform — the model gives `Regions` a minimum length and no maximum, so that
    // ceiling is the service's own.
    let rows: Vec<(&str, Value, u16)> = vec![
        (
            "Regions alone",
            json!({ "TelemetryType": "Logs", "Regions": ["eu-west-1"] }),
            200,
        ),
        (
            "AllRegions alone",
            json!({ "TelemetryType": "Logs", "AllRegions": true }),
            200,
        ),
        (
            "AllRegions false beside Regions",
            json!({ "TelemetryType": "Logs", "AllRegions": false, "Regions": ["eu-west-1"] }),
            200,
        ),
        (
            "both at once",
            json!({ "TelemetryType": "Logs", "AllRegions": true, "Regions": ["eu-west-1"] }),
            400,
        ),
        (
            "several regions of the home partition",
            json!({
                "TelemetryType": "Logs",
                "Regions": ["eu-west-3", "me-central-1", "ca-west-1", "ap-northeast-3"]
            }),
            200,
        ),
        (
            "a region of another partition",
            json!({ "TelemetryType": "Logs", "Regions": ["eu-west-1", "us-gov-west-1"] }),
            400,
        ),
        (
            "a region that is not a region at all",
            json!({ "TelemetryType": "Logs", "Regions": ["eu-west-1", "Europe"] }),
            400,
        ),
        (
            "a region code no partition holds",
            json!({ "TelemetryType": "Logs", "Regions": ["us-aaaa-1"] }),
            400,
        ),
        (
            "more regions than the service will fan out into",
            json!({
                "TelemetryType": "Logs",
                "Regions": (0..60).map(|n| format!("us-aaa{n}-1")).collect::<Vec<String>>()
            }),
            400,
        ),
    ];

    for (index, (case, rule, expected)) in rows.into_iter().enumerate() {
        let name = format!("scope-{index}");
        let (status, response) = create(
            &emulator,
            HOME_REGION,
            &json!({ "RuleName": name, "Rule": rule }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {response:?}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response:?}"
            );
            let (status, body) = get(&emulator, HOME_REGION, &name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused create stores nothing, got {status} {body:?}"
            );
        }
    }
}

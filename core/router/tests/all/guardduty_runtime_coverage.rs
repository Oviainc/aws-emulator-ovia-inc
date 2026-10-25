// SPDX-License-Identifier: CC0-1.0

//! GuardDuty's Runtime Monitoring coverage report is COMPUTED from the account's own
//! compute resources, which belong to the services that own them.
//!
//! This suite needs more than one service on one engine — GuardDuty reaches EC2 and
//! Amazon EKS through the in-process callback, so a single-service router has nobody to
//! ask — which is why it lives here rather than in the GuardDuty crate, whose own suite
//! (`coverage_listing.rs`) covers the refusals and the empty report.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! * THE ROWS. `ListCoverage` reports one row per resource the account holds that
//!   Runtime Monitoring covers — EC2's instances and Amazon EKS's clusters — carrying
//!   that resource's own identity and details, the detector and account it belongs to,
//!   and the instant its coverage facts last changed. A resource the account stops
//!   holding (a terminated instance) leaves the report.
//! * THE STATUS. A resource is `HEALTHY` only where the GuardDuty security agent IS
//!   deployed on it, which for an Amazon EKS cluster is the `aws-guardduty-agent`
//!   add-on being installed and running; the row of a resource with no agent is
//!   `UNHEALTHY` and says why. The `managementType` says who deploys that agent, from
//!   the detector's own Runtime Monitoring configuration, and which resource types are
//!   reported at all follows the feature that covers them.
//! * THE QUERY. `filterCriteria` narrows the report by the attribute each criterion key
//!   names (`Equals` admitting, `NotEquals` excluding, several criteria all holding),
//!   `sortCriteria` orders it by any of the sort keys in either direction, and
//!   `maxResults`/`nextToken` page it with every row appearing exactly once.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests driven — on a sized thread.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

// The form encoder and the XML element reader this suite needs live ONCE in the test
// crate, in `query_wire`: a suite that writes its own drifts from the others.
use crate::query_wire::{element, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The add-on that IS the GuardDuty security agent on an Amazon EKS cluster.
const GUARDDUTY_AGENT_ADDON: &str = "aws-guardduty-agent";

/// How long a poll waits for a clock-owed status move (an EKS cluster or add-on
/// reaching `ACTIVE`) before the test gives up and fails.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
/// The signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One answer, reduced to what a client branches on.
#[derive(Debug)]
struct Answer {
    status: u16,
    text: String,
    body: serde_json::Value,
}

fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    body: String,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type)
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    Answer { status, text, body }
}

/// Drive one ec2Query action: `POST /` with the parameters in the form body.
fn ec2(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push_str(&format!("&{name}={}", form_encode(value)));
    }
    send(
        router,
        runtime,
        "ec2",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        body,
    )
}

/// Drive one Amazon EKS request (restJson1).
fn eks(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> Answer {
    let payload = body.map(|json| json.to_string()).unwrap_or_default();
    send(
        router,
        runtime,
        "eks",
        method,
        uri,
        "application/json",
        payload,
    )
}

/// Drive one GuardDuty request (restJson1).
fn guardduty(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> Answer {
    let payload = body.map(|json| json.to_string()).unwrap_or_default();
    send(
        router,
        runtime,
        "guardduty",
        method,
        uri,
        "application/json",
        payload,
    )
}

/// Run `probe` until it answers `Some`, or fail the test after [`SETTLE_TIMEOUT`].
///
/// A clock-owed move (`CREATING` -> `ACTIVE`) is due a fixed span after the call that
/// started it, so this waits for the span rather than assuming it has passed.
fn settled<T>(case: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the resource did not settle within {SETTLE_TIMEOUT:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// One `ListCoverage` answer's rows, failing the test unless the call succeeded.
fn coverage_rows(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    detector_id: &str,
    body: serde_json::Value,
    case: &str,
) -> Vec<serde_json::Value> {
    let answer = guardduty(
        router,
        runtime,
        "POST",
        &format!("/detector/{detector_id}/coverage"),
        Some(body),
    );
    assert_eq!(
        answer.status, 200,
        "case {case:?}: list-coverage expected 200, got {answer:?}"
    );
    answer
        .body
        .get("resources")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a resources list, got {answer:?}"))
        .clone()
}

/// The `resourceId` of each row, in the order the report carried them.
fn row_ids(rows: &[serde_json::Value]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.get("resourceId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row's member, as text (`""` when the row does not carry it).
fn text_at(row: &serde_json::Value, path: &[&str]) -> String {
    let mut value = row;
    for step in path {
        match value.get(step) {
            Some(next) => value = next,
            None => return String::new(),
        }
    }
    value.as_str().unwrap_or_default().to_string()
}

/// The row whose `resourceId` is `id`, failing the test when the report has none.
fn row_of<'rows>(
    rows: &'rows [serde_json::Value],
    id: &str,
    case: &str,
) -> &'rows serde_json::Value {
    rows.iter()
        .find(|row| text_at(row, &["resourceId"]) == id)
        .unwrap_or_else(|| panic!("case {case:?}: expected a row for {id:?}, got {rows:?}"))
}

/// A `features` entry for a `CreateDetector`/`UpdateDetector` body.
fn feature(name: &str, status: &str, additional: &[(&str, &str)]) -> serde_json::Value {
    let configuration: Vec<serde_json::Value> = additional
        .iter()
        .map(|(name, status)| serde_json::json!({"name": name, "status": status}))
        .collect();
    let mut entry = serde_json::json!({"name": name, "status": status});
    if !configuration.is_empty() {
        entry["additionalConfiguration"] = serde_json::Value::Array(configuration);
    }
    entry
}

#[test]
fn guardduty_reports_the_runtime_coverage_of_the_accounts_own_resources() {
    std::thread::Builder::new()
        .name("guardduty-runtime-coverage".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            let account_id = engine::emulator_account_id().to_string();

            // ---------------------------------------------------------------
            // Setup: the account's own resources, created through the services
            // that own them — two EC2 instances of different types, and two
            // Amazon EKS clusters, one of which gets the agent add-on.
            // ---------------------------------------------------------------
            let vpc = ec2(&router, &runtime, "CreateVpc", &[("CidrBlock", "10.7.0.0/16")]);
            assert_eq!(vpc.status, 200, "setup: CreateVpc must succeed, got {vpc:?}");
            let vpc_id = element(&vpc.text, "vpcId");
            let mut subnets: Vec<String> = Vec::new();
            for cidr_block in ["10.7.1.0/24", "10.7.2.0/24"] {
                let answer = ec2(
                    &router,
                    &runtime,
                    "CreateSubnet",
                    &[("VpcId", vpc_id.as_str()), ("CidrBlock", cidr_block)],
                );
                assert_eq!(
                    answer.status, 200,
                    "setup: CreateSubnet {cidr_block} must succeed, got {answer:?}"
                );
                subnets.push(element(&answer.text, "subnetId"));
            }

            // Two instances, launched separately so each carries a different
            // instance type: the row must report the resource's OWN type.
            let mut instances: Vec<(String, String)> = Vec::new();
            for instance_type in ["t3.micro", "m5.large"] {
                let answer = ec2(
                    &router,
                    &runtime,
                    "RunInstances",
                    &[
                        ("ImageId", "ami-0123456789abcdef0"),
                        ("MinCount", "1"),
                        ("MaxCount", "1"),
                        ("InstanceType", instance_type),
                        ("SubnetId", subnets[0].as_str()),
                    ],
                );
                assert_eq!(
                    answer.status, 200,
                    "setup: RunInstances {instance_type} must succeed, got {answer:?}"
                );
                let instance_id = element(&answer.text, "instanceId");
                instances.push((instance_id, instance_type.to_string()));
            }
            instances.sort();

            let covered_cluster = "coverage-with-agent";
            let bare_cluster = "coverage-without-agent";
            for name in [covered_cluster, bare_cluster] {
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    "/clusters",
                    Some(serde_json::json!({
                        "name": name,
                        "roleArn": format!("arn:aws:iam::{account_id}:role/eksClusterRole"),
                        "resourcesVpcConfig": {"subnetIds": subnets.clone()},
                    })),
                );
                assert_eq!(
                    answer.status, 200,
                    "setup: CreateCluster {name} must succeed, got {answer:?}"
                );
            }
            // An add-on can only be installed on an ACTIVE cluster, and a fresh
            // cluster is `CREATING` until the clock owes it `ACTIVE`.
            settled("the cluster reaches ACTIVE", || {
                let answer = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{covered_cluster}"),
                    None,
                );
                (text_at(&answer.body, &["cluster", "status"]) == "ACTIVE").then_some(())
            });
            let created = eks(
                &router,
                &runtime,
                "POST",
                &format!("/clusters/{covered_cluster}/addons"),
                Some(serde_json::json!({"addonName": GUARDDUTY_AGENT_ADDON})),
            );
            assert_eq!(
                created.status, 200,
                "setup: CreateAddon must succeed, got {created:?}"
            );
            let addon_version = text_at(&created.body, &["addon", "addonVersion"]);
            assert!(
                !addon_version.is_empty(),
                "setup: the add-on must report a version, got {created:?}"
            );
            // The add-on is `CREATING` until the clock owes it `ACTIVE`, which is
            // what makes the cluster covered.
            settled("the add-on reaches ACTIVE", || {
                let answer = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{covered_cluster}/addons/{GUARDDUTY_AGENT_ADDON}"),
                    None,
                );
                (text_at(&answer.body, &["addon", "status"]) == "ACTIVE").then_some(())
            });

            // A detector with Runtime Monitoring enabled and GuardDuty managing the
            // agents: the configuration under which every resource type is covered.
            let detector = guardduty(
                &router,
                &runtime,
                "POST",
                "/detector",
                Some(serde_json::json!({
                    "enable": true,
                    "features": [feature(
                        "RUNTIME_MONITORING",
                        "ENABLED",
                        &[
                            ("EC2_AGENT_MANAGEMENT", "ENABLED"),
                            ("EKS_ADDON_MANAGEMENT", "ENABLED"),
                        ],
                    )],
                })),
            );
            assert_eq!(
                detector.status, 200,
                "setup: CreateDetector must succeed, got {detector:?}"
            );
            let detector_id = text_at(&detector.body, &["detectorId"]);
            assert!(
                !detector_id.is_empty(),
                "setup: the create must mint a detectorId, got {detector:?}"
            );

            // ---------------------------------------------------------------
            // RULE: the rows are the account's resources, with their own facts.
            // ---------------------------------------------------------------
            let all = coverage_rows(
                &router,
                &runtime,
                &detector_id,
                serde_json::json!({}),
                "the whole report",
            );
            let expected_ids: Vec<String> = {
                let mut ids: Vec<String> = instances
                    .iter()
                    .map(|(instance_id, _)| instance_id.clone())
                    .collect();
                ids.push(covered_cluster.to_string());
                ids.push(bare_cluster.to_string());
                ids.sort();
                ids
            };
            let mut reported_ids = row_ids(&all);
            reported_ids.sort();
            assert_eq!(
                reported_ids, expected_ids,
                "the report must carry one row per covered resource of the account, got {all:?}"
            );

            for (instance_id, instance_type) in &instances {
                let case = format!("the EC2 row for {instance_id}");
                let row = row_of(&all, instance_id, &case);
                for (path, expected) in [
                    (vec!["detectorId"], detector_id.clone()),
                    (vec!["accountId"], account_id.clone()),
                    (vec!["coverageStatus"], "UNHEALTHY".to_string()),
                    (
                        vec!["resourceDetails", "resourceType"],
                        "EC2".to_string(),
                    ),
                    (
                        vec!["resourceDetails", "ec2InstanceDetails", "instanceId"],
                        instance_id.clone(),
                    ),
                    (
                        vec!["resourceDetails", "ec2InstanceDetails", "instanceType"],
                        instance_type.clone(),
                    ),
                    (
                        vec!["resourceDetails", "ec2InstanceDetails", "managementType"],
                        "AUTO_MANAGED".to_string(),
                    ),
                ] {
                    assert_eq!(
                        text_at(row, &path),
                        expected,
                        "case {case:?}: expected {path:?} to be {expected:?}, got {row:?}"
                    );
                }
                // No agent runs anywhere in this emulator, so an instance's row says
                // so rather than claiming coverage, and reports no agent version.
                assert!(
                    text_at(row, &["issue"]).contains(instance_id),
                    "case {case:?}: an uncovered resource's row must say why, naming it, got {row:?}"
                );
                assert!(
                    row.get("resourceDetails")
                        .and_then(|details| details.get("ec2InstanceDetails"))
                        .and_then(|details| details.get("agentDetails"))
                        .is_none(),
                    "case {case:?}: no agent is deployed, so no version is reported, got {row:?}"
                );
                assert!(
                    row.get("updatedAt").is_some(),
                    "case {case:?}: the row must carry the instant its facts last changed, got {row:?}"
                );
            }

            let covered = row_of(&all, covered_cluster, "the covered cluster's row");
            for (path, expected) in [
                (vec!["coverageStatus"], "HEALTHY".to_string()),
                (vec!["resourceDetails", "resourceType"], "EKS".to_string()),
                (
                    vec!["resourceDetails", "eksClusterDetails", "clusterName"],
                    covered_cluster.to_string(),
                ),
                (
                    vec![
                        "resourceDetails",
                        "eksClusterDetails",
                        "addonDetails",
                        "addonVersion",
                    ],
                    addon_version.clone(),
                ),
                (
                    vec![
                        "resourceDetails",
                        "eksClusterDetails",
                        "addonDetails",
                        "addonStatus",
                    ],
                    "ACTIVE".to_string(),
                ),
                (
                    vec!["resourceDetails", "eksClusterDetails", "managementType"],
                    "AUTO_MANAGED".to_string(),
                ),
            ] {
                assert_eq!(
                    text_at(covered, &path),
                    expected,
                    "the covered cluster's row must report {path:?} as {expected:?}, got {covered:?}"
                );
            }
            assert!(
                covered.get("issue").is_none_or(serde_json::Value::is_null),
                "a covered resource has no reason to be unhealthy, got {covered:?}"
            );

            let bare = row_of(&all, bare_cluster, "the uncovered cluster's row");
            assert_eq!(
                text_at(bare, &["coverageStatus"]),
                "UNHEALTHY",
                "a cluster with no agent add-on is not covered, got {bare:?}"
            );
            assert!(
                text_at(bare, &["issue"]).contains(GUARDDUTY_AGENT_ADDON),
                "the reason must name the agent add-on it is missing, got {bare:?}"
            );
            assert!(
                bare.get("resourceDetails")
                    .and_then(|details| details.get("eksClusterDetails"))
                    .and_then(|details| details.get("addonDetails"))
                    .is_none(),
                "a cluster with no agent add-on reports no add-on details, got {bare:?}"
            );

            // ---------------------------------------------------------------
            // RULE: filterCriteria narrows the report.
            // ---------------------------------------------------------------
            let instance_ids: Vec<String> = instances
                .iter()
                .map(|(instance_id, _)| instance_id.clone())
                .collect();
            let cluster_ids = {
                let mut ids = vec![bare_cluster.to_string(), covered_cluster.to_string()];
                ids.sort();
                ids
            };
            let filter_rows: Vec<(String, serde_json::Value, Vec<String>)> = vec![
                (
                    "RESOURCE_TYPE equals EC2".to_string(),
                    serde_json::json!([{"criterionKey": "RESOURCE_TYPE", "filterCondition": {"equals": ["EC2"]}}]),
                    instance_ids.clone(),
                ),
                (
                    "RESOURCE_TYPE equals EKS".to_string(),
                    serde_json::json!([{"criterionKey": "RESOURCE_TYPE", "filterCondition": {"equals": ["EKS"]}}]),
                    cluster_ids.clone(),
                ),
                (
                    "RESOURCE_TYPE notEquals EC2".to_string(),
                    serde_json::json!([{"criterionKey": "RESOURCE_TYPE", "filterCondition": {"notEquals": ["EC2"]}}]),
                    cluster_ids.clone(),
                ),
                (
                    "RESOURCE_TYPE equals both types".to_string(),
                    serde_json::json!([{"criterionKey": "RESOURCE_TYPE", "filterCondition": {"equals": ["EC2", "EKS"]}}]),
                    expected_ids.clone(),
                ),
                (
                    "COVERAGE_STATUS equals HEALTHY".to_string(),
                    serde_json::json!([{"criterionKey": "COVERAGE_STATUS", "filterCondition": {"equals": ["HEALTHY"]}}]),
                    vec![covered_cluster.to_string()],
                ),
                (
                    "COVERAGE_STATUS equals UNHEALTHY".to_string(),
                    serde_json::json!([{"criterionKey": "COVERAGE_STATUS", "filterCondition": {"equals": ["UNHEALTHY"]}}]),
                    {
                        let mut ids = instance_ids.clone();
                        ids.push(bare_cluster.to_string());
                        ids.sort();
                        ids
                    },
                ),
                (
                    "INSTANCE_ID equals one instance".to_string(),
                    serde_json::json!([{"criterionKey": "INSTANCE_ID", "filterCondition": {"equals": [instance_ids[0]]}}]),
                    vec![instance_ids[0].clone()],
                ),
                (
                    "EKS_CLUSTER_NAME equals the covered cluster".to_string(),
                    serde_json::json!([{"criterionKey": "EKS_CLUSTER_NAME", "filterCondition": {"equals": [covered_cluster]}}]),
                    vec![covered_cluster.to_string()],
                ),
                (
                    "CLUSTER_NAME equals the uncovered cluster".to_string(),
                    serde_json::json!([{"criterionKey": "CLUSTER_NAME", "filterCondition": {"equals": [bare_cluster]}}]),
                    vec![bare_cluster.to_string()],
                ),
                (
                    "ADDON_VERSION equals the installed version".to_string(),
                    serde_json::json!([{"criterionKey": "ADDON_VERSION", "filterCondition": {"equals": [addon_version]}}]),
                    vec![covered_cluster.to_string()],
                ),
                (
                    "MANAGEMENT_TYPE equals AUTO_MANAGED".to_string(),
                    serde_json::json!([{"criterionKey": "MANAGEMENT_TYPE", "filterCondition": {"equals": ["AUTO_MANAGED"]}}]),
                    expected_ids.clone(),
                ),
                (
                    "MANAGEMENT_TYPE equals MANUAL".to_string(),
                    serde_json::json!([{"criterionKey": "MANAGEMENT_TYPE", "filterCondition": {"equals": ["MANUAL"]}}]),
                    Vec::new(),
                ),
                (
                    "ACCOUNT_ID equals the account that holds them".to_string(),
                    serde_json::json!([{"criterionKey": "ACCOUNT_ID", "filterCondition": {"equals": [account_id]}}]),
                    expected_ids.clone(),
                ),
                (
                    "ACCOUNT_ID notEquals the account that holds them".to_string(),
                    serde_json::json!([{"criterionKey": "ACCOUNT_ID", "filterCondition": {"notEquals": [account_id]}}]),
                    Vec::new(),
                ),
                (
                    "two criteria, both of which must hold".to_string(),
                    serde_json::json!([
                        {"criterionKey": "RESOURCE_TYPE", "filterCondition": {"equals": ["EKS"]}},
                        {"criterionKey": "COVERAGE_STATUS", "filterCondition": {"equals": ["UNHEALTHY"]}},
                    ]),
                    vec![bare_cluster.to_string()],
                ),
                (
                    "equals and notEquals on one criterion".to_string(),
                    serde_json::json!([{"criterionKey": "RESOURCE_TYPE", "filterCondition": {
                        "equals": ["EC2", "EKS"], "notEquals": ["EKS"],
                    }}]),
                    instance_ids.clone(),
                ),
                (
                    // No ECS cluster inventory is reachable, so no row is in one.
                    "ECS_CLUSTER_NAME equals any name".to_string(),
                    serde_json::json!([{"criterionKey": "ECS_CLUSTER_NAME", "filterCondition": {"equals": ["any-cluster"]}}]),
                    Vec::new(),
                ),
            ];
            for (case, criterion, expected) in filter_rows {
                let rows = coverage_rows(
                    &router,
                    &runtime,
                    &detector_id,
                    serde_json::json!({"filterCriteria": {"filterCriterion": criterion}}),
                    &case,
                );
                let mut got = row_ids(&rows);
                got.sort();
                assert_eq!(
                    got, expected,
                    "case {case:?}: expected the rows {expected:?}, got {got:?}"
                );
            }

            // ---------------------------------------------------------------
            // RULE: sortCriteria orders the report, in either direction.
            // ---------------------------------------------------------------
            // The attribute each sort key reads off a row, so the order the report
            // comes back in can be checked against the rows themselves.
            let sort_attribute = |row: &serde_json::Value, key: &str| -> String {
                match key {
                    "ACCOUNT_ID" => text_at(row, &["accountId"]),
                    "COVERAGE_STATUS" => text_at(row, &["coverageStatus"]),
                    "ISSUE" => text_at(row, &["issue"]),
                    "ADDON_VERSION" => text_at(
                        row,
                        &[
                            "resourceDetails",
                            "eksClusterDetails",
                            "addonDetails",
                            "addonVersion",
                        ],
                    ),
                    "CLUSTER_NAME" | "EKS_CLUSTER_NAME" => {
                        text_at(row, &["resourceDetails", "eksClusterDetails", "clusterName"])
                    }
                    "ECS_CLUSTER_NAME" => String::new(),
                    "INSTANCE_ID" => {
                        text_at(row, &["resourceDetails", "ec2InstanceDetails", "instanceId"])
                    }
                    other => panic!("no attribute is defined for the sort key {other:?}"),
                }
            };
            for key in [
                "ACCOUNT_ID",
                "COVERAGE_STATUS",
                "ISSUE",
                "ADDON_VERSION",
                "CLUSTER_NAME",
                "EKS_CLUSTER_NAME",
                "ECS_CLUSTER_NAME",
                "INSTANCE_ID",
            ] {
                for order in ["ASC", "DESC"] {
                    let case = format!("sorted by {key} {order}");
                    let rows = coverage_rows(
                        &router,
                        &runtime,
                        &detector_id,
                        serde_json::json!({"sortCriteria": {"attributeName": key, "orderBy": order}}),
                        &case,
                    );
                    let mut got = row_ids(&rows);
                    got.sort();
                    assert_eq!(
                        got, expected_ids,
                        "case {case:?}: ordering must not change which rows are reported"
                    );
                    let attributes: Vec<String> = rows
                        .iter()
                        .map(|row| sort_attribute(row, key))
                        .collect();
                    let ordered = attributes.windows(2).all(|pair| {
                        if order == "ASC" {
                            pair[0] <= pair[1]
                        } else {
                            pair[0] >= pair[1]
                        }
                    });
                    assert!(
                        ordered,
                        "case {case:?}: expected the rows ordered by their own {key}, got {attributes:?}"
                    );
                }
            }
            // `UPDATED_AT` orders by the instant, which the rows carry as a number.
            for order in ["ASC", "DESC"] {
                let case = format!("sorted by UPDATED_AT {order}");
                let rows = coverage_rows(
                    &router,
                    &runtime,
                    &detector_id,
                    serde_json::json!({"sortCriteria": {"attributeName": "UPDATED_AT", "orderBy": order}}),
                    &case,
                );
                let instants: Vec<f64> = rows
                    .iter()
                    .map(|row| {
                        row.get("updatedAt")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or_default()
                    })
                    .collect();
                let ordered = instants.windows(2).all(|pair| {
                    if order == "ASC" {
                        pair[0] <= pair[1]
                    } else {
                        pair[0] >= pair[1]
                    }
                });
                assert!(
                    ordered,
                    "case {case:?}: expected the rows ordered by their own instant, got {instants:?}"
                );
            }
            // One concrete ordering, so a comparator that answered "ordered" for any
            // sequence could not satisfy the rows above: `HEALTHY` sorts before
            // `UNHEALTHY` ascending, and after it descending.
            for (order, first) in [("ASC", "HEALTHY"), ("DESC", "UNHEALTHY")] {
                let case = format!("COVERAGE_STATUS {order} reports {first} first");
                let rows = coverage_rows(
                    &router,
                    &runtime,
                    &detector_id,
                    serde_json::json!({"sortCriteria": {"attributeName": "COVERAGE_STATUS", "orderBy": order}}),
                    &case,
                );
                assert_eq!(
                    text_at(&rows[0], &["coverageStatus"]),
                    first,
                    "case {case:?}: got {rows:?}"
                );
            }

            // ---------------------------------------------------------------
            // RULE: maxResults/nextToken page the report.
            // ---------------------------------------------------------------
            let unpaged = row_ids(&coverage_rows(
                &router,
                &runtime,
                &detector_id,
                serde_json::json!({"sortCriteria": {"attributeName": "ACCOUNT_ID"}}),
                "the unpaged order",
            ));
            for page_size in 1..=5 {
                let case = format!("paged {page_size} at a time");
                let mut walked: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                for _ in 0..=unpaged.len() {
                    let mut body = serde_json::json!({
                        "maxResults": page_size,
                        "sortCriteria": {"attributeName": "ACCOUNT_ID"},
                    });
                    if let Some(token) = token.as_ref() {
                        body["nextToken"] = serde_json::Value::String(token.clone());
                    }
                    let answer = guardduty(
                        &router,
                        &runtime,
                        "POST",
                        &format!("/detector/{detector_id}/coverage"),
                        Some(body),
                    );
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: a page expected 200, got {answer:?}"
                    );
                    let rows = answer
                        .body
                        .get("resources")
                        .and_then(serde_json::Value::as_array)
                        .unwrap_or_else(|| panic!("case {case:?}: no resources in {answer:?}"))
                        .clone();
                    assert!(
                        rows.len() <= page_size,
                        "case {case:?}: a page carried {} rows, more than asked for",
                        rows.len()
                    );
                    walked.extend(row_ids(&rows));
                    token = answer
                        .body
                        .get("nextToken")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    if token.is_none() {
                        break;
                    }
                }
                assert!(
                    token.is_none(),
                    "case {case:?}: the walk must end with no further token"
                );
                assert_eq!(
                    walked, unpaged,
                    "case {case:?}: paging must report each row exactly once, in the same order"
                );
            }

            // ---------------------------------------------------------------
            // RULE: the detector's configuration decides what is covered.
            // ---------------------------------------------------------------
            let update = |features: serde_json::Value| {
                let answer = guardduty(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/detector/{detector_id}"),
                    Some(serde_json::json!({"features": features})),
                );
                assert_eq!(
                    answer.status, 200,
                    "setup: UpdateDetector must succeed, got {answer:?}"
                );
            };

            // Only the Amazon EKS resources are covered by EKS Runtime Monitoring.
            update(serde_json::json!([
                feature("RUNTIME_MONITORING", "DISABLED", &[]),
                feature("EKS_RUNTIME_MONITORING", "ENABLED", &[]),
            ]));
            let mut got = row_ids(&coverage_rows(
                &router,
                &runtime,
                &detector_id,
                serde_json::json!({}),
                "EKS Runtime Monitoring only",
            ));
            got.sort();
            assert_eq!(
                got, cluster_ids,
                "with only EKS Runtime Monitoring enabled, only the clusters are covered"
            );

            // With neither feature enabled, nothing is covered.
            update(serde_json::json!([feature(
                "EKS_RUNTIME_MONITORING",
                "DISABLED",
                &[]
            )]));
            assert!(
                coverage_rows(
                    &router,
                    &runtime,
                    &detector_id,
                    serde_json::json!({}),
                    "no Runtime Monitoring at all",
                )
                .is_empty(),
                "a detector whose Runtime Monitoring is disabled covers nothing"
            );

            // Runtime Monitoring again, this time with the customer managing the
            // agents: every resource is reported, and says the deployment is theirs.
            update(serde_json::json!([feature(
                "RUNTIME_MONITORING",
                "ENABLED",
                &[
                    ("EC2_AGENT_MANAGEMENT", "DISABLED"),
                    ("EKS_ADDON_MANAGEMENT", "DISABLED"),
                ],
            )]));
            let manual = coverage_rows(
                &router,
                &runtime,
                &detector_id,
                serde_json::json!({"filterCriteria": {"filterCriterion": [
                    {"criterionKey": "MANAGEMENT_TYPE", "filterCondition": {"equals": ["MANUAL"]}},
                ]}}),
                "the customer manages the agents",
            );
            let mut got = row_ids(&manual);
            got.sort();
            assert_eq!(
                got, expected_ids,
                "with GuardDuty's agent management off, every row is MANUAL, got {manual:?}"
            );

            // ---------------------------------------------------------------
            // RULE: a resource the account no longer holds leaves the report.
            // ---------------------------------------------------------------
            let terminated = ec2(
                &router,
                &runtime,
                "TerminateInstances",
                &[("InstanceId.1", instance_ids[0].as_str())],
            );
            assert_eq!(
                terminated.status, 200,
                "setup: TerminateInstances must succeed, got {terminated:?}"
            );
            let mut got = row_ids(&coverage_rows(
                &router,
                &runtime,
                &detector_id,
                serde_json::json!({}),
                "after one instance is terminated",
            ));
            got.sort();
            let remaining: Vec<String> = expected_ids
                .iter()
                .filter(|id| *id != &instance_ids[0])
                .cloned()
                .collect();
            assert_eq!(
                got, remaining,
                "a terminated instance is no longer a resource of the account"
            );
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the coverage suite must not panic");
}

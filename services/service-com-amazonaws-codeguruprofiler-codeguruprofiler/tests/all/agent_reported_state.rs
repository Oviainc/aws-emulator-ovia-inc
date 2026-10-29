// SPDX-License-Identifier: CC0-1.0

//! What `ConfigureAgent` does with the state an agent reports about itself, end to end
//! over HTTP through the router this service mounts in production.
//!
//! The rule every case here asserts: a call through which a caller REPORTS its own state
//! stores that state against the identity the request names, so each reporter's latest
//! report is held separately — a further report from the same reporter supersedes the one
//! stored for it, a report from another reporter stands beside it, and the report belongs
//! to the resource it was made against and to no other. The configuration the call answers
//! with is the group's own orchestration decision, so it never echoes what was reported.
//!
//! The rows differ along what that rule generalises over: whether the agent identified the
//! instance it runs on, how long that identifier is (both ends of the modelled
//! `FleetInstanceId` length range), which metadata fields the report carried and how many
//! (none, one, the whole modelled catalogue), what the values hold (an ARN, a number,
//! multi-byte text, a kilobyte), how many instances report to one group, which of two
//! groups a report is addressed to, and whether the group orchestrates its agents at all.

use std::collections::BTreeMap;

use crate::profiling_group_lifecycle::{Answer, Harness};

/// This service's state type, read through the shared engine the harness mounts: the
/// stored report is what `ConfigureAgent` writes, and the model declares no operation that
/// reads it back (the modelled `AgentConfiguration` carries the service's decision FOR an
/// agent, not the agent's account of itself), so the store is where a report is observed —
/// the same way the sibling suite observes submitted anomaly feedback.
type State =
    service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState;

/// Every field a report's `metadata` may name, as the model's `MetadataField` enum lists
/// them — `smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#MetadataField")`
/// gives these nine values — so a case can report the WHOLE catalogue and not only the two
/// fields a single recorded agent happened to send.
const METADATA_FIELDS: [&str; 9] = [
    "ComputePlatform",
    "AgentId",
    "AwsRequestId",
    "ExecutionEnvironment",
    "LambdaFunctionArn",
    "LambdaMemoryLimitInMB",
    "LambdaRemainingTimeInMilliseconds",
    "LambdaTimeGapBetweenInvokesInMilliseconds",
    "LambdaPreviousExecutionTimeInMilliseconds",
];

/// The `computePlatform` values the model's `ComputePlatform` enum declares
/// (`smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#ComputePlatform")`),
/// which is also what an agent reports under the `ComputePlatform` metadata field.
const COMPUTE_PLATFORMS: [&str; 2] = ["Default", "AWSLambda"];

/// One report, as the stored record holds it: (the instance it identified itself as, the
/// metadata it carried).
type Stored = Vec<(Option<String>, BTreeMap<String, String>)>;

/// What one group holds right now, in stored order (least recently reporting instance
/// first).
fn stored(harness: &Harness, group: &str) -> Stored {
    let state = harness.engine().state::<State>();
    let guard = state.lock();
    let group = guard
        .groups
        .get(group)
        .unwrap_or_else(|| panic!("no stored group named {group}"));
    group
        .agent_reports
        .iter()
        .map(|report| {
            (
                report.fleet_instance_id.clone(),
                report
                    .metadata
                    .iter()
                    .map(|(field, value)| (field.clone(), value.clone()))
                    .collect(),
            )
        })
        .collect()
}

/// The instant one group's record for `fleet_instance_id` was reported, or `None` when it
/// holds no report under that identity.
fn reported_at(harness: &Harness, group: &str, fleet_instance_id: Option<&str>) -> Option<f64> {
    let state = harness.engine().state::<State>();
    let guard = state.lock();
    guard
        .groups
        .get(group)
        .and_then(|group| group.agent_report(fleet_instance_id))
        .map(|report| report.reported_at.as_secs_f64())
}

/// A group, created with the orchestration decision the case needs.
async fn create_group(harness: &Harness, name: &str, profiling_enabled: bool) {
    let answer = harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=token-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "profilingGroupName": name,
                "agentOrchestrationConfig": { "profilingEnabled": profiling_enabled },
            }))
            .expect("a serializable body"),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {name}: CreateProfilingGroup must succeed, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// `ConfigureAgent`: `POST /profilingGroups/{profilingGroupName}/configureAgent`
/// (`smithy_query(query="operation", operation="ConfigureAgent")`), answered 200.
async fn report(harness: &Harness, group: &str, body: serde_json::Value) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups/{group}/configureAgent"),
            "application/json",
            serde_json::to_vec(&body).expect("a serializable body"),
        )
        .await
}

/// The request body of a report: `fleetInstanceId` and `metadata` only when the case sends
/// them, since both members are optional.
fn report_body(
    fleet_instance_id: Option<&str>,
    metadata: Option<&BTreeMap<String, String>>,
) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    if let Some(id) = fleet_instance_id {
        body.insert("fleetInstanceId".to_string(), serde_json::Value::from(id));
    }
    if let Some(metadata) = metadata {
        body.insert(
            "metadata".to_string(),
            serde_json::Value::Object(
                metadata
                    .iter()
                    .map(|(field, value)| (field.clone(), serde_json::Value::from(value.clone())))
                    .collect(),
            ),
        );
    }
    serde_json::Value::Object(body)
}

/// How long an agent profiles before reporting again, as the handler's
/// `AgentConfiguration$periodInSeconds` states it.
const PERIOD_IN_SECONDS: i64 = 300;

/// An accepted report answers 200 with the group's own orchestration decision — never
/// anything the report carried.
fn assert_configuration(case: &str, answer: &Answer, profiling_enabled: bool) {
    assert_eq!(
        answer.status, 200,
        "case {case}: ConfigureAgent must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    // `configuration` is the operation's `@httpPayload`, so the body IS the
    // `AgentConfiguration` rather than an object wrapping it.
    let configuration = answer.json();
    assert_eq!(
        configuration.get("shouldProfile").and_then(|v| v.as_bool()),
        Some(profiling_enabled),
        "case {case}: shouldProfile must be the group's stored profilingEnabled, got {configuration}"
    );
    assert_eq!(
        configuration
            .get("periodInSeconds")
            .and_then(|v| v.as_i64()),
        Some(PERIOD_IN_SECONDS),
        "case {case}: periodInSeconds must be the configured profiling period, got {configuration}"
    );
}

/// One report the table below makes.
struct ReportCase {
    label: &'static str,
    /// The instance the agent identifies itself as, or `None` when it identifies none.
    fleet_instance_id: Option<String>,
    /// The metadata sent, or `None` when the report carries no `metadata` member at all.
    metadata: Option<BTreeMap<String, String>>,
}

impl ReportCase {
    /// The metadata the stored record must hold: what was sent, and the empty map when the
    /// report named none.
    fn expected_metadata(&self) -> BTreeMap<String, String> {
        self.metadata.clone().unwrap_or_default()
    }
}

/// `metadata` as a map, from (field, value) pairs.
fn metadata(pairs: &[(&str, &str)]) -> Option<BTreeMap<String, String>> {
    Some(
        pairs
            .iter()
            .map(|(field, value)| (field.to_string(), value.to_string()))
            .collect(),
    )
}

/// The whole modelled catalogue in one report, each field carrying a value of the kind
/// that field names: a platform, an id, an ARN, a memory limit, millisecond counts.
fn whole_catalogue() -> Option<BTreeMap<String, String>> {
    let values = [
        COMPUTE_PLATFORMS[1],
        "agent-7",
        "8f9b1c2d-3e4f-5a6b-7c8d-9e0f1a2b3c4d",
        "AWS_Lambda_java17",
        "arn:aws:lambda:us-east-1:000000000000:function:profiled",
        "1024",
        "27351",
        "60000",
        "412",
    ];
    Some(
        METADATA_FIELDS
            .iter()
            .zip(values)
            .map(|(field, value)| (field.to_string(), value.to_string()))
            .collect(),
    )
}

/// The table: rows differ in whether the instance is identified and how long its id is
/// (1 and 255 are the `FleetInstanceId` length bounds the model declares), in which
/// metadata fields the report carried and how many, and in what the values hold.
fn report_cases() -> Vec<ReportCase> {
    vec![
        ReportCase {
            label: "an EC2 instance id, reporting its compute platform and agent id",
            fleet_instance_id: Some("i-0123456789abcdef0".to_string()),
            metadata: metadata(&[
                ("ComputePlatform", COMPUTE_PLATFORMS[0]),
                ("AgentId", "agent-1"),
            ]),
        },
        ReportCase {
            label: "a Fargate task id, reporting the whole modelled catalogue",
            fleet_instance_id: Some("1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d".to_string()),
            metadata: whole_catalogue(),
        },
        ReportCase {
            label: "an instance that reports no metadata member at all",
            fleet_instance_id: Some("i-00000000000000001".to_string()),
            metadata: None,
        },
        ReportCase {
            label: "an instance that reports an empty metadata map",
            fleet_instance_id: Some("i-00000000000000002".to_string()),
            metadata: Some(BTreeMap::new()),
        },
        ReportCase {
            label: "a one-character instance id, the modelled minimum",
            fleet_instance_id: Some("i".to_string()),
            metadata: metadata(&[("ComputePlatform", COMPUTE_PLATFORMS[1])]),
        },
        ReportCase {
            label: "a 255-character instance id, the modelled maximum",
            fleet_instance_id: Some(format!("i-{}", "0".repeat(253))),
            metadata: metadata(&[("AgentId", "agent-at-the-bound")]),
        },
        ReportCase {
            label: "an instance reporting a multi-byte value and a kilobyte one",
            fleet_instance_id: Some("i-multibyte".to_string()),
            metadata: Some(BTreeMap::from([
                (
                    "ExecutionEnvironment".to_string(),
                    "ünïcode ✓ окружение 🙂".to_string(),
                ),
                ("AwsRequestId".to_string(), "r".repeat(1024)),
            ])),
        },
        ReportCase {
            label: "an agent that identifies no instance",
            fleet_instance_id: None,
            metadata: metadata(&[("ComputePlatform", COMPUTE_PLATFORMS[0])]),
        },
    ]
}

#[tokio::test]
async fn each_reported_state_is_stored_against_the_instance_that_reported_it() {
    let harness = Harness::new();
    // Two groups, differing in the orchestration decision they answer with, so the
    // configuration a report receives is visibly the group's and not the report's.
    let (reported_to, other) = ("agent-report-target", "agent-report-bystander");
    create_group(&harness, reported_to, true).await;
    create_group(&harness, other, false).await;

    assert!(
        stored(&harness, reported_to).is_empty(),
        "case a created group: it holds no agent report before one is made, got {:?}",
        stored(&harness, reported_to)
    );

    let mut expected: Stored = Vec::new();
    for case in report_cases() {
        let label = case.label;
        let answer = report(
            &harness,
            reported_to,
            report_body(case.fleet_instance_id.as_deref(), case.metadata.as_ref()),
        )
        .await;
        assert_configuration(label, &answer, true);

        expected.push((case.fleet_instance_id.clone(), case.expected_metadata()));
        let held = stored(&harness, reported_to);
        assert_eq!(
            held, expected,
            "case {label}: the group must hold every instance's reported state, in \
             reporting order, got {held:?}"
        );

        // A report against one group reaches no other.
        let bystander = stored(&harness, other);
        assert!(
            bystander.is_empty(),
            "case {label}: a report to {reported_to} must not reach {other}, got {bystander:?}"
        );

        // The report moved the group's ping instant, which a read of the group reports.
        let described = harness.describe(reported_to).await.json();
        let pinged = described
            .pointer("/profilingStatus/latestAgentOrchestratedAt")
            .cloned();
        assert!(
            pinged.is_some(),
            "case {label}: a report must leave latestAgentOrchestratedAt reported, \
             got {described}"
        );
    }

    // The same instances report to the other group independently, and receive THAT
    // group's orchestration decision.
    for case in report_cases() {
        let answer = report(
            &harness,
            other,
            report_body(case.fleet_instance_id.as_deref(), case.metadata.as_ref()),
        )
        .await;
        assert_configuration(case.label, &answer, false);
    }
    assert_eq!(
        stored(&harness, other).len(),
        report_cases().len(),
        "case the second group: each group holds the reports made to it, got {:?}",
        stored(&harness, other)
    );
    assert_eq!(
        stored(&harness, reported_to),
        expected,
        "case the first group: reports to {other} must leave {reported_to} as it was"
    );
}

#[tokio::test]
async fn a_further_report_from_one_instance_supersedes_the_stored_one() {
    let harness = Harness::new();
    let group = "agent-report-supersede";
    create_group(&harness, group, true).await;

    let instance = "i-0f1e2d3c4b5a69780";
    let bystander_instance = "i-00112233445566778";
    // Each row is the report that supersedes the one before it from the same instance: the
    // platform flips, fields are added, replaced, dropped, and the metadata emptied — the
    // record must hold the LATEST state, never a merge of every report ever made.
    let rows: [(&str, Option<BTreeMap<String, String>>); 6] = [
        (
            "first, platform and agent id",
            metadata(&[
                ("ComputePlatform", COMPUTE_PLATFORMS[0]),
                ("AgentId", "agent-a"),
            ]),
        ),
        (
            "the platform changes and a lambda field joins",
            metadata(&[
                ("ComputePlatform", COMPUTE_PLATFORMS[1]),
                ("AgentId", "agent-a"),
                (
                    "LambdaFunctionArn",
                    "arn:aws:lambda:us-east-1:000000000000:function:p",
                ),
            ]),
        ),
        (
            "the agent id changes and the lambda field is dropped",
            metadata(&[
                ("ComputePlatform", COMPUTE_PLATFORMS[1]),
                ("AgentId", "agent-b"),
            ]),
        ),
        ("the whole modelled catalogue", whole_catalogue()),
        ("an empty metadata map", Some(BTreeMap::new())),
        ("no metadata member at all", None),
    ];

    // A second instance reports first, so the stored order is observable: the superseding
    // report becomes the group's most recent one.
    let bystander_metadata = metadata(&[("AgentId", "agent-bystander")]);
    assert_configuration(
        "the bystanding instance, reporting first",
        &report(
            &harness,
            group,
            report_body(Some(bystander_instance), bystander_metadata.as_ref()),
        )
        .await,
        true,
    );

    let mut previous_at = 0.0_f64;
    for (case, sent) in rows {
        let answer = report(&harness, group, report_body(Some(instance), sent.as_ref())).await;
        assert_configuration(case, &answer, true);

        let held = stored(&harness, group);
        assert_eq!(
            held,
            vec![
                (
                    Some(bystander_instance.to_string()),
                    bystander_metadata.clone().unwrap_or_default(),
                ),
                (Some(instance.to_string()), sent.clone().unwrap_or_default()),
            ],
            "case {case}: one instance carries one record, the state it reported last, and \
             the superseding report is the most recent one, got {held:?}"
        );

        let at = reported_at(&harness, group, Some(instance))
            .unwrap_or_else(|| panic!("case {case}: the report must be stored"));
        assert!(
            at >= previous_at,
            "case {case}: the stored report instant must not move backwards, got {at} \
             after {previous_at}"
        );
        previous_at = at;
    }

    // An agent that identifies no instance is an identity of its own: its report neither
    // supersedes nor is superseded by an identified instance's.
    assert_configuration(
        "an unidentified agent beside the identified ones",
        &report(&harness, group, report_body(None, metadata(&[("AgentId", "anon")]).as_ref())).await,
        true,
    );
    let held = stored(&harness, group);
    assert_eq!(
        held.len(),
        3,
        "case an unidentified agent: it stands beside both identified instances, got {held:?}"
    );
    assert_eq!(
        held.last().map(|(id, _)| id.clone()),
        Some(None),
        "case an unidentified agent: its report is the most recent one, got {held:?}"
    );
    assert!(
        reported_at(&harness, group, None).is_some(),
        "case an unidentified agent: its report must be readable under that identity, \
         got {held:?}"
    );
}

#[tokio::test]
async fn what_one_group_holds_is_bounded_by_the_stores_retention_limit() {
    let harness = Harness::new();
    let limit = service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::MAX_RETAINED_AGENT_REPORTS_PER_GROUP;
    // Each row is how far past the bound a group is pushed: at it, one over, and well
    // over. Each row gets its own group, so a row never starts from what the previous one
    // left stored.
    for over in [0usize, 1, 8] {
        let group = &format!("agent-report-retention-{over}");
        create_group(&harness, group, true).await;
        let reports = limit + over;
        for n in 0..reports {
            let instance = format!("i-{n:017x}");
            let answer = report(
                &harness,
                group,
                report_body(
                    Some(&instance),
                    metadata(&[("AgentId", "agent-many")]).as_ref(),
                ),
            )
            .await;
            assert_configuration(&format!("{over} over the bound, report {n}"), &answer, true);
        }

        let held = stored(&harness, group);
        assert_eq!(
            held.len(),
            limit.min(reports),
            "case {over} over the bound: a group retains at most {limit} reports, got {}",
            held.len()
        );
        // The records retained are the most recently reporting instances: the least
        // recent are evicted first, and the report just made is never the one dropped.
        let expected: Vec<Option<String>> = (reports.saturating_sub(limit)..reports)
            .map(|n| Some(format!("i-{n:017x}")))
            .collect();
        let held_ids: Vec<Option<String>> = held.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(
            held_ids, expected,
            "case {over} over the bound: the retained records must be those of the most \
             recently reporting instances, got {held_ids:?}"
        );
    }
}

#[tokio::test]
async fn reported_bytes_are_bounded_as_well_as_the_number_of_reports() {
    let harness = Harness::new();
    let byte_limit = service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::MAX_RETAINED_AGENT_REPORT_BYTES_PER_GROUP;
    let count_limit = service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::MAX_RETAINED_AGENT_REPORTS_PER_GROUP;

    // Rows: the SIZE of each report's metadata, chosen so a handful of them pass the byte
    // bound long before the count bound — a quarter of the budget per report, and the
    // whole budget in one report.
    for value_bytes in [byte_limit / 4, byte_limit] {
        let group = &format!("agent-report-bytes-{value_bytes}");
        create_group(&harness, group, true).await;
        // Enough reports to pass the byte bound several times over, and far fewer than
        // the count bound, so what evicts here is the byte budget alone.
        let reports = 8;
        assert!(
            reports < count_limit,
            "case {value_bytes} bytes per report: the row must stay under the count bound \
             of {count_limit}, got {reports}"
        );
        let mut made: Vec<Option<String>> = Vec::new();
        for n in 0..reports {
            let instance = format!("i-bytes-{n}");
            let answer = report(
                &harness,
                group,
                report_body(
                    Some(&instance),
                    metadata(&[("AwsRequestId", &"r".repeat(value_bytes))]).as_ref(),
                ),
            )
            .await;
            assert_configuration(
                &format!("{value_bytes} bytes per report, report {n}"),
                &answer,
                true,
            );
            made.push(Some(instance));
        }

        let held_ids: Vec<Option<String>> = stored(&harness, group)
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        assert!(
            held_ids.len() < reports,
            "case {value_bytes} bytes per report: {reports} reports of that size pass the \
             {byte_limit}-byte budget, so some must have been evicted, got {held_ids:?}"
        );
        assert!(
            !held_ids.is_empty(),
            "case {value_bytes} bytes per report: the report just made is never evicted, \
             got {held_ids:?}"
        );
        assert_eq!(
            held_ids.as_slice(),
            &made[made.len() - held_ids.len()..],
            "case {value_bytes} bytes per report: the retained records must be those of \
             the most recently reporting instances, got {held_ids:?}"
        );
    }
}

#[tokio::test]
async fn a_request_the_model_refuses_stores_no_report() {
    let harness = Harness::new();
    let group = "agent-report-refused";
    create_group(&harness, group, true).await;

    // (case, the body sent): the modelled `FleetInstanceId` length range (1..255) the
    // boundary walk enforces, broken below the minimum, just past the maximum and well
    // past it, with and without metadata beside it.
    let rows: [(&str, serde_json::Value); 4] = [
        (
            "an empty fleetInstanceId, below the modelled minimum",
            serde_json::json!({ "fleetInstanceId": "" }),
        ),
        (
            "a fleetInstanceId one past the modelled maximum",
            serde_json::json!({ "fleetInstanceId": format!("i-{}", "0".repeat(254)) }),
        ),
        (
            "a kilobyte of fleetInstanceId",
            serde_json::json!({ "fleetInstanceId": "i".repeat(1024) }),
        ),
        (
            "an over-long fleetInstanceId reported with metadata",
            serde_json::json!({
                "fleetInstanceId": "x".repeat(256),
                "metadata": { "ComputePlatform": "AWSLambda", "AgentId": "agent-refused" },
            }),
        ),
    ];

    for (case, body) in rows {
        let answer = report(&harness, group, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: a request the model refuses must answer 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case}: expected ValidationException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        let held = stored(&harness, group);
        assert!(
            held.is_empty(),
            "case {case}: a refused request must store nothing, got {held:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_groups_reports_go_with_it() {
    let harness = Harness::new();
    let group = "agent-report-deleted";
    create_group(&harness, group, true).await;
    let instance = "i-0a0b0c0d0e0f01020";
    assert_configuration(
        "a report to seed the group",
        &report(
            &harness,
            group,
            report_body(
                Some(instance),
                metadata(&[("ComputePlatform", COMPUTE_PLATFORMS[0])]).as_ref(),
            ),
        )
        .await,
        true,
    );

    let gone = harness
        .send(
            "DELETE",
            &format!("/profilingGroups/{group}"),
            "application/json",
            Vec::new(),
        )
        .await;
    assert_eq!(
        gone.status, 204,
        "case the delete: DeleteProfilingGroup must answer 204, got {} / {}",
        gone.status,
        String::from_utf8_lossy(&gone.body)
    );

    // A report to a group the store cannot resolve is the modelled 404, and stores nothing
    // to resurrect it with.
    let missing = report(
        &harness,
        group,
        report_body(Some(instance), metadata(&[("AgentId", "after-delete")]).as_ref()),
    )
    .await;
    assert_eq!(
        missing.status, 404,
        "case a deleted group: ConfigureAgent must answer 404, got {} / {}",
        missing.status,
        String::from_utf8_lossy(&missing.body)
    );

    // Re-created under the same name, the group holds none of the deleted group's
    // reports, and is reported to again from scratch.
    create_group(&harness, group, true).await;
    assert!(
        stored(&harness, group).is_empty(),
        "case the re-created group: a new group holds no report, got {:?}",
        stored(&harness, group)
    );
    assert_configuration(
        "the re-created group",
        &report(
            &harness,
            group,
            report_body(Some(instance), metadata(&[("AgentId", "agent-again")]).as_ref()),
        )
        .await,
        true,
    );
    assert_eq!(
        stored(&harness, group),
        vec![(
            Some(instance.to_string()),
            BTreeMap::from([("AgentId".to_string(), "agent-again".to_string())]),
        )],
        "case the re-created group: it must hold exactly the report just made"
    );
}

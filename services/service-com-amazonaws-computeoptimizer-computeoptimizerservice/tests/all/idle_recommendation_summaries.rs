// SPDX-License-Identifier: CC0-1.0

//! What a recommendation summary says about the account's IDLE resources, end to end over
//! the wire.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests prove
//! is what a client sees — never what the state struct holds.
//!
//! The rule under test: `GetRecommendationSummaries` summarises EVERY resource type the
//! service tracks for the account — the rightsizing findings catalog AND the idle resource
//! analysis `GetIdleRecommendations` reports — so each summary of a resource type the idle
//! analysis covers carries a count of the account's resources of that type at each
//! `IdleFinding` (`idleSummaries`), zero where none was found idle, what dropping the idle
//! ones would save (`idleSavingsOpportunity`), and an `aggregatedSavingsOpportunity` that is
//! the rightsizing opportunity plus the idle one; a resource type the idle analysis does not
//! cover carries neither idle member.
//!
//! Each assertion is made over a TABLE of idle records recorded into the analysis — differing
//! in resource type, in `IdleFinding`, in how many resources share a group, in cost (including
//! a resource that costs nothing) and in owning account — and the expectation is always what
//! the per-resource idle reader reports for the same records, so a summary that is not
//! computed from the stored analysis fails here.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::IDLE_FINDING_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    reports_rightsizing_recommendations, service_state, IdleResourceAnalysis, ANALYSIS_DELAY,
};

use crate::harness::{call, emulator_with_engine, opt_in};

/// The ONE account this emulator presents, and so the only account a summary can cover.
fn own_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// An organization member account this standalone emulator records none of: analysis held
/// against it is nothing a caller of this service may be told about, so it must reach no
/// summary.
const MEMBER_ACCOUNT: &str = "210987654321";

/// The `RecommendationSourceType` each `IdleRecommendationResourceType` is summarised under.
///
/// Source: the model's two enums — `IdleRecommendationResourceType` (what an idle record
/// names its resource with) and `RecommendationSourceType` (what a summary names its group
/// with). They spell four of the same resources differently (`EC2Instance` against
/// `Ec2Instance`, `EBSVolume` against `EbsVolume`, `ECSService` against `EcsService`,
/// `RDSDBInstance` against `RdsDBInstance`) and the rest identically.
const SUMMARY_GROUP_OF_IDLE_TYPE: &[(&str, &str)] = &[
    ("EC2Instance", "Ec2Instance"),
    ("EBSVolume", "EbsVolume"),
    ("ECSService", "EcsService"),
    ("RDSDBInstance", "RdsDBInstance"),
    ("AutoScalingGroup", "AutoScalingGroup"),
    ("NatGateway", "NatGateway"),
    ("DynamoDBTable", "DynamoDBTable"),
    ("ElastiCacheCluster", "ElastiCacheCluster"),
    ("MemoryDBCluster", "MemoryDBCluster"),
    ("DocumentDBCluster", "DocumentDBCluster"),
    ("WorkSpaces", "WorkSpaces"),
    ("SageMakerEndpoint", "SageMakerEndpoint"),
];

/// The summary group an `IdleRecommendationResourceType` belongs to.
fn summary_group_of(idle_resource_type: &str) -> &'static str {
    SUMMARY_GROUP_OF_IDLE_TYPE
        .iter()
        .find(|(idle_type, _)| *idle_type == idle_resource_type)
        .map(|(_, group)| *group)
        .unwrap_or_else(|| panic!("{idle_resource_type:?} is not an IdleRecommendationResourceType"))
}

/// One idle record to add to the analysis.
struct Recorded {
    arn_service: &'static str,
    arn_resource: &'static str,
    /// The `IdleRecommendationResourceType` the ARN above makes the resource.
    idle_resource_type: &'static str,
    /// An `IdleFinding` value.
    finding: &'static str,
    monthly_cost: f64,
    account_id: &'static str,
}

/// The table of idle records the suite records on top of the analysis a booted emulator
/// already holds.
///
/// The rows differ along everything the rule generalises over: the resource type (one the
/// rightsizing catalog also holds resources of, one it holds none of), the `IdleFinding`
/// reached, how many resources share one group, what the resource costs (including a resource
/// that costs nothing, which saves nothing) and which account owns it.
fn recorded() -> Vec<Recorded> {
    vec![
        // An EC2 instance: a resource type the rightsizing catalog also holds, so this
        // group's aggregated opportunity has both a rightsizing and an idle part.
        Recorded {
            arn_service: "ec2",
            arn_resource: "instance/i-0123456789abcdef0",
            idle_resource_type: "EC2Instance",
            finding: "Idle",
            monthly_cost: 60.0,
            account_id: own_account(),
        },
        // Two more DynamoDB tables, at two different findings, of a resource type the
        // rightsizing catalog holds nothing of: a group's counts are per finding, and more
        // than one resource can share a finding group.
        // Two more DynamoDB tables share one group, at two different findings.
        Recorded {
            arn_service: "dynamodb",
            arn_resource: "table/legacy-events",
            idle_resource_type: "DynamoDBTable",
            finding: "Idle",
            monthly_cost: 12.34,
            account_id: own_account(),
        },
        Recorded {
            arn_service: "dynamodb",
            arn_resource: "table/legacy-archive",
            idle_resource_type: "DynamoDBTable",
            finding: "Unused",
            monthly_cost: 7.5,
            account_id: own_account(),
        },
        // A WorkSpace that costs nothing: counted like any other idle resource, but it adds
        // nothing to what dropping the group's idle resources saves.
        Recorded {
            arn_service: "workspaces",
            arn_resource: "workspace/ws-00000000000c0",
            idle_resource_type: "WorkSpaces",
            finding: "Unattached",
            monthly_cost: 0.0,
            account_id: own_account(),
        },
        // A cost carrying more than two decimal places: a group's saving is the whole of what
        // its resources cost TO THE CENT, so its percentage is still a percentage.
        Recorded {
            arn_service: "elasticache",
            arn_resource: "cluster:sessions-staging",
            idle_resource_type: "ElastiCacheCluster",
            finding: "Idle",
            monthly_cost: 9.999,
            account_id: own_account(),
        },
        // Another account's idle resource: this emulator presents one standalone account, so
        // no summary it answers may count it.
        Recorded {
            arn_service: "sagemaker",
            arn_resource: "endpoint/member-scoring",
            idle_resource_type: "SageMakerEndpoint",
            finding: "Unused",
            monthly_cost: 500.0,
            account_id: MEMBER_ACCOUNT,
        },
    ]
}

/// Record the table into the store `GetIdleRecommendations` reports from, the way Compute
/// Optimizer's own analysis of an observed workload would: no operation of this API creates
/// an idle recommendation.
fn record(engine: &Arc<engine::InMemoryEngine>, rows: &[Recorded]) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for row in rows {
        guard.idle_recommendations.insert(IdleResourceAnalysis {
            account_id: row.account_id.to_string(),
            arn_service: row.arn_service.to_string(),
            arn_resource: row.arn_resource.to_string(),
            finding: row.finding.to_string(),
            utilization_metrics: Vec::new(),
            days_observed: 30.0,
            monthly_cost: row.monthly_cost,
            tags: Vec::new(),
        });
    }
}

/// An emulator whose account is opted in, whose analysis is due and whose idle analysis holds
/// the table above as well as the records a booted emulator already carries.
async fn analysed_emulator_with_recorded_idle_resources() -> smithy_server_runtime::MultiServiceRouter
{
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    record(&engine, &recorded());
    // The service owes the analysis its enrollment started one `ANALYSIS_DELAY` after the
    // enrollment is written; the extra margin covers scheduler jitter.
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    router
}

/// Every summary the service reports, walking the pages to the last one.
async fn all_summaries(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut request = json!({});
    loop {
        let (status, body) = call(router, "GetRecommendationSummaries", request.clone()).await;
        assert_eq!(
            status, 200,
            "summaries for {request}: expected 200, got {status}: {body}"
        );
        summaries.extend(
            body["recommendationSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("recommendationSummaries must be a list, got {body}"))
                .clone(),
        );
        match body["nextToken"].as_str() {
            Some(token) => request = json!({"nextToken": token}),
            None => return summaries,
        }
    }
}

/// What the per-resource idle reader reports for one group of the caller's account: how many
/// of its resources are at each `IdleFinding`, and what dropping them all would save.
#[derive(Debug, Default)]
struct IdleGroup {
    counts: BTreeMap<String, f64>,
    savings: f64,
}

/// Every idle recommendation `GetIdleRecommendations` reports, grouped by the summary group
/// its resource type belongs to — the expectation the summaries are judged against, so the
/// two readers of one analysis cannot disagree.
async fn idle_groups(router: &smithy_server_runtime::MultiServiceRouter) -> BTreeMap<String, IdleGroup> {
    let (status, body) = call(router, "GetIdleRecommendations", json!({})).await;
    assert_eq!(
        status, 200,
        "idle recommendations: expected 200, got {status}: {body}"
    );
    let reported = body["idleRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("idleRecommendations must be a list, got {body}"))
        .clone();
    assert!(
        body["nextToken"].is_null(),
        "the suite's records must fit one page of idle recommendations, got {body}"
    );

    let mut grouped: BTreeMap<String, IdleGroup> = BTreeMap::new();
    for recommendation in &reported {
        let case = recommendation["resourceArn"].as_str().unwrap_or_default();
        assert_eq!(
            recommendation["accountId"].as_str(),
            Some(own_account()),
            "case {case:?}: this emulator reports one account's idle resources, got \
             {recommendation}"
        );
        let idle_resource_type = recommendation["resourceType"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a resourceType in {recommendation}"));
        let finding = recommendation["finding"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a finding in {recommendation}"))
            .to_string();
        let saving = recommendation["savingsOpportunity"]["estimatedMonthlySavings"]["value"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a saving in {recommendation}"));

        let group = grouped
            .entry(summary_group_of(idle_resource_type).to_string())
            .or_default();
        *group.counts.entry(finding).or_default() += 1.0;
        group.savings += saving;
    }
    grouped
}

/// A number member of a response, which the wire carries as a JSON number.
fn number(value: &Value, case: &str) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a number, got {value}"))
}

/// The estimated monthly saving of one savings opportunity of a summary.
fn saving_of(summary: &Value, member: &str, case: &str) -> f64 {
    number(
        &summary[member]["estimatedMonthlySavings"]["value"],
        &format!("{case}/{member}"),
    )
}

/// `idleSummaries` and `idleSavingsOpportunity` are the account's IDLE resources of the
/// summary's own resource type counted and costed: one entry per `IdleFinding` value with the
/// number of resources at it, and a saving that totals what the per-resource idle reader says
/// dropping those same resources saves. A resource type nothing was found idle of carries
/// neither member.
#[tokio::test]
async fn idle_members_count_and_cost_the_account_s_idle_resources_of_that_type() {
    let router = analysed_emulator_with_recorded_idle_resources().await;

    let expected = idle_groups(&router).await;
    assert!(
        expected.len() > 1,
        "the analysis must span more than one resource type for this to prove anything, got \
         {expected:?}"
    );
    let summaries = all_summaries(&router).await;

    for summary in &summaries {
        let case = summary["recommendationResourceType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match expected.get(&case) {
            // A resource type the service found resources of idle: every `IdleFinding` is
            // reported with its count, and the saving is what dropping them saves.
            Some(group) => {
                let reported = summary["idleSummaries"].as_array().cloned().unwrap_or_else(|| {
                    panic!("case {case:?}: idleSummaries must report the resources found idle, got {summary}")
                });
                let names: Vec<&str> = reported
                    .iter()
                    .map(|entry| entry["name"].as_str().unwrap_or_default())
                    .collect();
                assert_eq!(
                    names, *IDLE_FINDING_VALUES,
                    "case {case:?}: expected a count at each IdleFinding \
                     {IDLE_FINDING_VALUES:?}, got {names:?}"
                );
                for entry in &reported {
                    let finding = entry["name"].as_str().unwrap_or_default().to_string();
                    let count = number(&entry["value"], &format!("{case}/{finding}"));
                    let want = group.counts.get(&finding).copied().unwrap_or(0.0);
                    assert_eq!(
                        count, want,
                        "case {case:?}/{finding:?}: expected the {want} resources the idle \
                         reader reports at that finding, got {count}"
                    );
                }

                let idle = saving_of(summary, "idleSavingsOpportunity", &case);
                assert!(
                    (idle - group.savings).abs() < 0.01,
                    "case {case:?}: expected the {} the idle reader says dropping these \
                     resources saves, got {idle}",
                    group.savings
                );
                let percentage = number(
                    &summary["idleSavingsOpportunity"]["savingsOpportunityPercentage"],
                    &case,
                );
                assert_eq!(
                    percentage == 0.0,
                    idle == 0.0,
                    "case {case:?}: an idle percentage is zero exactly when the saving is, got \
                     {percentage} for {idle}"
                );
                assert!(
                    (0.0..=100.0).contains(&percentage),
                    "case {case:?}: a saving is a percentage of monthly cost, got {percentage}"
                );
                if idle > 0.0 {
                    // The recommendation for a resource nothing is using is to stop paying
                    // for it, so the whole of what the group costs is saved.
                    assert_eq!(
                        percentage, 100.0,
                        "case {case:?}: dropping these idle resources saves the whole of what \
                         they cost, got {percentage} for {idle}"
                    );
                }
            }
            // A resource type nothing of which was found idle: where the idle analysis covers
            // the resource type the counts are reported and all zero — the zero count, not
            // the absent member, is how "nothing found idle" is said — and where it does not
            // cover it at all neither idle member is reported.
            None => {
                let covers_idle = SUMMARY_GROUP_OF_IDLE_TYPE
                    .iter()
                    .any(|(_, group)| *group == case);
                if !covers_idle {
                    assert!(
                        summary["idleSummaries"].is_null()
                            && summary["idleSavingsOpportunity"].is_null(),
                        "case {case:?}: the idle analysis does not cover this resource type, so \
                         neither idle member is reported, got {summary}"
                    );
                    continue;
                }
                let reported = summary["idleSummaries"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: the idle analysis covers this resource type, so its \
                             summary reports a count at each IdleFinding, got {summary}"
                        )
                    });
                let names: Vec<&str> = reported
                    .iter()
                    .map(|entry| entry["name"].as_str().unwrap_or_default())
                    .collect();
                assert_eq!(
                    names, *IDLE_FINDING_VALUES,
                    "case {case:?}: expected a count at each IdleFinding \
                     {IDLE_FINDING_VALUES:?}, got {names:?}"
                );
                for entry in &reported {
                    let finding = entry["name"].as_str().unwrap_or_default().to_string();
                    assert_eq!(
                        number(&entry["value"], &format!("{case}/{finding}")),
                        0.0,
                        "case {case:?}/{finding:?}: no resource of this type was found idle, \
                         so its count is zero, got {summary}"
                    );
                }
                assert_eq!(
                    saving_of(summary, "idleSavingsOpportunity", &case),
                    0.0,
                    "case {case:?}: no resource of this type was found idle, so dropping them \
                     saves nothing, got {summary}"
                );
            }
        }
    }

    // Every group the idle reader reports is summarised: an idle resource of a type the
    // rightsizing catalog holds nothing of has a summary of its own.
    let summarised: Vec<&str> = summaries
        .iter()
        .map(|summary| summary["recommendationResourceType"].as_str().unwrap_or_default())
        .collect();
    for group in expected.keys() {
        assert!(
            summarised.contains(&group.as_str()),
            "case {group:?}: a resource type the service found resources of idle must be \
             summarised, got {summarised:?}"
        );
    }
}

/// `aggregatedSavingsOpportunity` is the savings opportunity of EVERY recommendation the
/// service holds for the resource type: the rightsizing opportunity plus the idle one, never
/// a copy of either — with a percentage of what all of those resources cost.
#[tokio::test]
async fn the_aggregated_saving_is_the_rightsizing_saving_plus_the_idle_one() {
    let router = analysed_emulator_with_recorded_idle_resources().await;

    let idle_by_group = idle_groups(&router).await;
    let summaries = all_summaries(&router).await;
    let mut groups_with_both = 0;

    for summary in &summaries {
        let case = summary["recommendationResourceType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // A group the service only CLASSIFIES carries no savings opportunity at all — it has
        // no recommended configuration to price — so there is nothing of it to aggregate.
        if !reports_rightsizing_recommendations(&case) {
            assert!(
                summary["savingsOpportunity"].is_null()
                    && summary["aggregatedSavingsOpportunity"].is_null(),
                "case {case:?}: a group the service only classifies states no saving, got \
                 {summary}"
            );
            continue;
        }
        let rightsizing = saving_of(summary, "savingsOpportunity", &case);
        let aggregated = saving_of(summary, "aggregatedSavingsOpportunity", &case);
        let idle = idle_by_group
            .get(&case)
            .map(|group| group.savings)
            .unwrap_or(0.0);
        assert!(
            (aggregated - (rightsizing + idle)).abs() < 0.01,
            "case {case:?}: expected the rightsizing saving {rightsizing} plus the idle one \
             {idle}, got {aggregated}"
        );
        if rightsizing > 0.0 && idle > 0.0 {
            groups_with_both += 1;
            assert!(
                aggregated > rightsizing && aggregated > idle,
                "case {case:?}: a resource type with both kinds of recommendation aggregates \
                 more than either ({rightsizing} and {idle}), got {aggregated}"
            );
        }
        let percentage = number(
            &summary["aggregatedSavingsOpportunity"]["savingsOpportunityPercentage"],
            &case,
        );
        assert!(
            (0.0..=100.0).contains(&percentage),
            "case {case:?}: a saving is a percentage of monthly cost, got {percentage}"
        );
        assert_eq!(
            percentage == 0.0,
            aggregated == 0.0,
            "case {case:?}: a percentage is zero exactly when the saving is, got {percentage} \
             for {aggregated}"
        );
    }

    assert!(
        groups_with_both > 0,
        "at least one resource type must hold both a rightsizing and an idle recommendation \
         for this to prove anything: {summaries:?}"
    );
}

/// The idle members report what the STORE holds: recording one more idle resource changes
/// exactly its own group's count and saving, and the aggregated saving with it, while every
/// other group answers as before.
#[tokio::test]
async fn recording_one_more_idle_resource_changes_exactly_its_own_group() {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;

    let before = all_summaries(&router).await;

    // One more idle MemoryDB cluster of the caller's account: its group gains one `Unused`
    // resource and its cost in savings.
    let added = Recorded {
        arn_service: "memorydb",
        arn_resource: "cluster/leaderboard-staging",
        idle_resource_type: "MemoryDBCluster",
        finding: "Unused",
        monthly_cost: 25.5,
        account_id: own_account(),
    };
    let group = summary_group_of(added.idle_resource_type).to_string();
    let added_cost = added.monthly_cost;
    record(&engine, &[added]);

    let after = all_summaries(&router).await;
    assert_eq!(
        before.len(),
        after.len(),
        "recording a resource of a type already summarised adds no summary, got {after:?} \
         after {before:?}"
    );

    for (was, is) in before.iter().zip(&after) {
        let case = is["recommendationResourceType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if case != group {
            assert_eq!(
                was, is,
                "case {case:?}: another group's analysis did not change, so its summary must \
                 not either"
            );
            continue;
        }
        let count_of = |summary: &Value| {
            summary["idleSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .find(|entry| entry["name"] == json!("Unused"))
                .map(|entry| number(&entry["value"], &case))
                .unwrap_or(0.0)
        };
        assert_eq!(
            count_of(is),
            count_of(was) + 1.0,
            "case {case:?}: the group gained one Unused resource, got {is} after {was}"
        );
        let idle_before = saving_of(was, "idleSavingsOpportunity", &case);
        let idle_after = saving_of(is, "idleSavingsOpportunity", &case);
        assert!(
            (idle_after - (idle_before + added_cost)).abs() < 0.01,
            "case {case:?}: the idle saving grew by what the recorded resource costs \
             ({added_cost}), got {idle_after} after {idle_before}"
        );
        let aggregated_before = saving_of(was, "aggregatedSavingsOpportunity", &case);
        let aggregated_after = saving_of(is, "aggregatedSavingsOpportunity", &case);
        assert!(
            (aggregated_after - (aggregated_before + added_cost)).abs() < 0.01,
            "case {case:?}: the aggregated saving grew with the idle one, got \
             {aggregated_after} after {aggregated_before}"
        );
    }
}

/// A resource type the service holds ONLY idle resources of is summarised with the idle
/// members, a rightsizing finding vocabulary whose counts are all zero (the service analyses
/// no resource of that type for rightsizing), and a rightsizing opportunity of nothing — so
/// its aggregated saving is the idle one.
#[tokio::test]
async fn a_resource_type_only_found_idle_is_summarised_with_no_rightsizing_counts() {
    let router = analysed_emulator_with_recorded_idle_resources().await;

    let summaries = all_summaries(&router).await;
    // Rows: the resource types this service holds no rightsizing analysis of, each a
    // different `IdleRecommendationResourceType` of the idle analysis.
    for idle_type in [
        "NatGateway",
        "DynamoDBTable",
        "ElastiCacheCluster",
        "MemoryDBCluster",
        "DocumentDBCluster",
        "WorkSpaces",
        "SageMakerEndpoint",
    ] {
        let case = summary_group_of(idle_type);
        let summary = summaries
            .iter()
            .find(|summary| summary["recommendationResourceType"] == json!(case))
            .unwrap_or_else(|| {
                panic!("case {case:?}: a resource type found idle must be summarised, got {summaries:?}")
            });
        for entry in summary["summaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
        {
            let finding = entry["name"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                number(&entry["value"], &format!("{case}/{finding}")),
                0.0,
                "case {case:?}/{finding:?}: the service analyses no resource of this type for \
                 rightsizing, so its finding counts are zero, got {summary}"
            );
        }
        assert!(
            !summary["idleSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: a resource type found idle reports its idle counts, got {summary}"
        );
        assert_eq!(
            saving_of(summary, "savingsOpportunity", case),
            0.0,
            "case {case:?}: the service holds no rightsizing recommendation for this resource \
             type, so there is nothing to save by rightsizing it, got {summary}"
        );
        assert_eq!(
            saving_of(summary, "aggregatedSavingsOpportunity", case),
            saving_of(summary, "idleSavingsOpportunity", case),
            "case {case:?}: with no rightsizing saving the aggregated saving is the idle one, \
             got {summary}"
        );
        for rating in ["high", "medium", "low", "veryLow"] {
            assert_eq!(
                number(&summary["currentPerformanceRiskRatings"][rating], case),
                0.0,
                "case {case:?}/{rating}: the risk ratings count analysed resources, of which \
                 this resource type has none, got {summary}"
            );
        }
    }
}

/// Summarising is a READ: two identical calls answer identically, and reading the summaries
/// leaves the idle analysis exactly as it was.
#[tokio::test]
async fn reading_the_idle_summaries_changes_nothing_a_later_caller_sees() {
    let router = analysed_emulator_with_recorded_idle_resources().await;

    let first = all_summaries(&router).await;
    let idle = call(&router, "GetIdleRecommendations", json!({})).await.1;
    let second = all_summaries(&router).await;
    assert_eq!(
        first, second,
        "GetRecommendationSummaries reports; reading it twice, with the idle reader in \
         between ({idle}), answers the same thing"
    );
}

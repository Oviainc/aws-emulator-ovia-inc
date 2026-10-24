// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeAccountHealth`.
//!
//! The emulator is driven over the wire through the same [`Emulator`] harness the
//! configuration-lifecycle suite uses, so the assertions are about what a client sees.
//!
//! RULES pinned here:
//!
//! * `DescribeAccountHealth` reports the account's analysed-resource population from the
//!   persisted resource collection — one monitored resource per covered CloudFormation
//!   stack and one per `(AppBoundaryKey, TagValue)` pair — as both
//!   `AnalyzedResourceCount` and `ResourceHours` (one resource-analysis hour per
//!   monitored resource in the last hour).
//! * Every count it reports is a tally over the records the store holds: the `ONGOING`
//!   insights of each `InsightType` for `OpenProactiveInsights` /
//!   `OpenReactiveInsights`, and the distinct metrics the stored anomalies name as the
//!   evidence the analysis read for `MetricsAnalyzed`. So the account-level read agrees
//!   with `ListInsights` and `DescribeResourceCollectionHealth` over the same store, and
//!   a count is zero only while the set it tallies is empty.
//! * The read changes nothing a later caller sees.
//!
//! No modelled operation mints an insight or an anomaly — DevOps Guru derives both from
//! its own analysis — so the rows that need them seed the store the way that analysis
//! would, through the shared harness, and then ask the service over HTTP.

use serde_json::{json, Value};

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::{
    AnomalySourceDetails, CloudWatchMetricsDetail, CloudWatchMetricsDimension,
    PerformanceInsightsMetricDimensionGroup, PerformanceInsightsMetricQuery,
    PerformanceInsightsMetricsDetail,
};
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    AnomalyRecord, InsightRecord,
};

use super::configuration_lifecycle::{Emulator, Reply};
use super::resource_collection_health::{insight, CLOSED, ONGOING, PROACTIVE, REACTIVE};

/// `GET /accounts/health` — `DescribeAccountHealth`'s http binding, from
/// `smithy_query(query="operation",operation="DescribeAccountHealth")`.
async fn describe_account_health(emulator: &Emulator) -> Reply {
    emulator.call("GET", "/accounts/health", None).await
}

/// An `UpdateResourceCollection` filter carrying the submitted stacks and tag entries.
fn collection(stacks: &[&str], tag_entries: &[(&str, &[&str])]) -> Value {
    let mut filter = json!({});
    if !stacks.is_empty() {
        filter["CloudFormation"] = json!({ "StackNames": stacks });
    }
    if !tag_entries.is_empty() {
        filter["Tags"] = tag_entries
            .iter()
            .map(|(key, values)| json!({ "AppBoundaryKey": key, "TagValues": values }))
            .collect();
    }
    filter
}

/// One coverage step: an `Action`, the stack names and the tag entries it carries.
type Step<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, &'a [&'a str])]);

struct HealthCase<'a> {
    name: &'a str,
    steps: &'a [Step<'a>],
    /// The monitored-resource count the steps leave covered.
    expect_resources: i64,
}

/// The number at a JSON pointer, as a client reads it.
fn number_at(reply: &Reply, pointer: &str) -> Option<i64> {
    reply.at(pointer).and_then(Value::as_i64)
}

/// RULE: the counts `DescribeAccountHealth` reports are computed from the coverage
/// `UpdateResourceCollection` persisted — `AnalyzedResourceCount` and `ResourceHours`
/// are the number of covered stack names plus the number of covered
/// `(AppBoundaryKey, TagValue)` pairs — and the analysis-derived numbers are zero.
///
/// The rows differ along which half of the filter is covered, the count, repetition
/// within one request and across requests, and removal; the empty account the recorded
/// unit reads is the first row.
#[tokio::test]
async fn account_health_counts_the_persisted_coverage() {
    let cases = [
        HealthCase {
            name: "no coverage declared at all",
            steps: &[],
            expect_resources: 0,
        },
        HealthCase {
            name: "one covered stack",
            steps: &[("ADD", &["my-stack"], &[])],
            expect_resources: 1,
        },
        HealthCase {
            name: "three stacks, one repeated inside the request",
            steps: &[("ADD", &["alpha", "beta", "beta", "gamma"], &[])],
            expect_resources: 3,
        },
        HealthCase {
            name: "the same stack added by two requests",
            steps: &[("ADD", &["dup"], &[]), ("ADD", &["dup"], &[])],
            expect_resources: 1,
        },
        HealthCase {
            name: "one app boundary key with two tag values",
            steps: &[("ADD", &[], &[("devops-guru-service-name", &["prod", "staging"])])],
            expect_resources: 2,
        },
        HealthCase {
            // One app-boundary key per account (`APP_BOUNDARY_KEYS_PER_ACCOUNT`), so a
            // larger tag coverage is more VALUES under that one key.
            name: "one app boundary key with three tag values, added by two requests",
            steps: &[
                ("ADD", &[], &[("boundary-one", &["only"])]),
                ("ADD", &[], &[("boundary-one", &["a", "b"])]),
            ],
            expect_resources: 3,
        },
        HealthCase {
            name: "stacks and tags covered together",
            steps: &[("ADD", &["s1", "s2"], &[("boundary", &["v"])])],
            expect_resources: 3,
        },
        HealthCase {
            name: "a removed stack stops being monitored",
            steps: &[("ADD", &["keep", "drop"], &[]), ("REMOVE", &["drop"], &[])],
            expect_resources: 1,
        },
        HealthCase {
            name: "removing a key's last tag value leaves nothing monitored",
            steps: &[
                ("ADD", &[], &[("boundary", &["v"])]),
                ("REMOVE", &[], &[("boundary", &["v"])]),
            ],
            expect_resources: 0,
        },
        HealthCase {
            name: "removing coverage that was never added changes nothing",
            steps: &[
                ("ADD", &["present"], &[]),
                ("REMOVE", &["absent"], &[("boundary", &["v"])]),
            ],
            expect_resources: 1,
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        for (action, stacks, tag_entries) in case.steps {
            let reply = emulator
                .update_resource_collection(action, collection(stacks, tag_entries))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: the {action} setup step must succeed, got {}",
                case.name, reply.raw
            );
        }

        let health = describe_account_health(&emulator).await;
        assert_eq!(
            health.status, 200,
            "case {:?}: DescribeAccountHealth must succeed, got {}",
            case.name, health.raw
        );
        assert_eq!(
            number_at(&health, "/AnalyzedResourceCount"),
            Some(case.expect_resources),
            "case {:?}: expected AnalyzedResourceCount {}, got {}",
            case.name,
            case.expect_resources,
            health.raw
        );
        assert_eq!(
            number_at(&health, "/ResourceHours"),
            Some(case.expect_resources),
            "case {:?}: expected ResourceHours {} (one hour per monitored resource), got {}",
            case.name,
            case.expect_resources,
            health.raw
        );
        // No insight and no anomaly was seeded in these rows, so every analysis tally is
        // taken over an empty set. The tables below drive the same members over a
        // non-empty store.
        for analysis_member in ["MetricsAnalyzed", "OpenProactiveInsights", "OpenReactiveInsights"] {
            assert_eq!(
                number_at(&health, &format!("/{analysis_member}")),
                Some(0),
                "case {:?}: expected {analysis_member} 0 — the store holds nothing to tally — got {}",
                case.name,
                health.raw
            );
        }
    }
}

/// RULE: `DescribeAccountHealth` is a report, so repeating it answers the same numbers
/// and leaves the records a later reader sees — the coverage, the insights and the
/// anomalies — untouched.
#[tokio::test]
async fn account_health_is_a_repeatable_read() {
    let emulator = Emulator::new();
    let setup = emulator
        .update_resource_collection("ADD", collection(&["one", "two"], &[("boundary", &["v"])]))
        .await;
    assert_eq!(
        setup.status, 200,
        "the coverage setup must succeed, got {}",
        setup.raw
    );
    // A non-empty store, so a count that moved on being read would show.
    emulator.seed_insights(vec![
        insight("i1", PROACTIVE, ONGOING, None, &["one"], &[], &[]),
        insight("i2", REACTIVE, ONGOING, None, &["two"], &[], &[]),
    ]);
    emulator.seed_anomalies(vec![anomaly(
        "a1",
        vec![cloud_watch_metric("AWS/RDS", "CPUUtilization", &[], "Average", 60)],
        vec![],
    )]);

    let first = describe_account_health(&emulator).await;
    let second = describe_account_health(&emulator).await;
    assert_eq!(
        first.status, 200,
        "the first read must succeed, got {}",
        first.raw
    );
    assert_eq!(
        (second.status, &second.body),
        (first.status, &first.body),
        "case {:?}: two reads must answer the same, got {} then {}",
        "repeated DescribeAccountHealth",
        first.raw,
        second.raw
    );
    assert_eq!(
        number_at(&first, "/AnalyzedResourceCount"),
        Some(3),
        "case {:?}: two stacks and one tag value are three monitored resources, got {}",
        "repeated DescribeAccountHealth",
        first.raw
    );
    assert_eq!(
        (
            number_at(&first, "/OpenProactiveInsights"),
            number_at(&first, "/OpenReactiveInsights"),
            number_at(&first, "/MetricsAnalyzed"),
        ),
        (Some(1), Some(1), Some(1)),
        "case {:?}: the seeded records must be tallied, got {}",
        "repeated DescribeAccountHealth",
        first.raw
    );

    // The coverage itself must be exactly what was declared: the read wrote nothing.
    let coverage = emulator
        .call("GET", "/resource-collections/AWS_CLOUD_FORMATION", None)
        .await;
    assert_eq!(
        coverage
            .at("/ResourceCollection/CloudFormation/StackNames")
            .cloned(),
        Some(json!(["one", "two"])),
        "case {:?}: the coverage a later reader sees must be unchanged, got {}",
        "coverage after two health reads",
        coverage.raw
    );
    // And so must the insights a later reader lists.
    let listed = emulator
        .call(
            "POST",
            "/insights",
            Some(&json!({ "StatusFilter": { "Ongoing": { "Type": "PROACTIVE" } } })),
        )
        .await;
    assert_eq!(
        listed
            .at("/ProactiveInsights")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "case {:?}: the insight a later reader lists must be unchanged, got {}",
        "insights after two health reads",
        listed.raw
    );
}

// ---------------------------------------------------------------------------
// The account-level insight tally.
//
// No modelled operation mints an insight, so these rows seed the store's `insights`
// map the way DevOps Guru's analysis would — through the same seeder the
// resource-collection health suite uses — and then read the account health over the
// wire. With an empty map every count is zero, which on its own could not tell a
// computed tally from a constant.
// ---------------------------------------------------------------------------

/// One row of the insight table: the case name, the insights seeded, and the open
/// proactive / open reactive counts expected for the account.
type InsightCase<'a> = (&'a str, Vec<InsightRecord>, (i64, i64));

/// RULE: `DescribeAccountHealth` reports, as `OpenProactiveInsights` and
/// `OpenReactiveInsights`, how many of the insights the store holds are `ONGOING` of
/// each `InsightType`, over the whole account — whatever resource collection an insight
/// is about, and whether or not it is about one at all.
///
/// The rows differ along insight type, status, how many insights there are, and which
/// resources an insight names (a covered stack, an uncovered one, a tag pair, none).
#[tokio::test]
async fn account_health_counts_the_open_insights_the_store_holds() {
    let cases: Vec<InsightCase> = vec![
        ("no insight at all", vec![], (0, 0)),
        (
            "one ongoing proactive insight",
            vec![insight("i1", PROACTIVE, ONGOING, None, &["watched"], &[], &[])],
            (1, 0),
        ),
        (
            "one ongoing reactive insight",
            vec![insight("i1", REACTIVE, ONGOING, None, &["watched"], &[], &[])],
            (0, 1),
        ),
        (
            "one ongoing insight of each type",
            vec![
                insight("i1", PROACTIVE, ONGOING, None, &["watched"], &[], &[]),
                insight("i2", REACTIVE, ONGOING, None, &["watched"], &[], &[]),
            ],
            (1, 1),
        ),
        (
            "three ongoing proactive and two ongoing reactive insights",
            vec![
                insight("i1", PROACTIVE, ONGOING, None, &["watched"], &[], &[]),
                insight("i2", PROACTIVE, ONGOING, None, &["watched"], &[], &[]),
                insight("i3", PROACTIVE, ONGOING, None, &[], &[], &["RDS"]),
                insight("i4", REACTIVE, ONGOING, None, &["watched"], &[], &[]),
                insight("i5", REACTIVE, ONGOING, None, &["elsewhere"], &[], &[]),
            ],
            (3, 2),
        ),
        (
            "a closed insight is not an open one",
            vec![
                insight("i1", PROACTIVE, CLOSED, Some(1_000), &["watched"], &[], &[]),
                insight("i2", REACTIVE, CLOSED, Some(2_000), &["watched"], &[], &[]),
            ],
            (0, 0),
        ),
        (
            "open and closed insights of both types together",
            vec![
                insight("i1", PROACTIVE, ONGOING, None, &["watched"], &[], &[]),
                insight("i2", PROACTIVE, CLOSED, Some(1_000), &["watched"], &[], &[]),
                insight("i3", REACTIVE, ONGOING, None, &["watched"], &[], &[]),
                insight("i4", REACTIVE, CLOSED, Some(1_000), &["watched"], &[], &[]),
            ],
            (1, 1),
        ),
        (
            "an insight about no resources at all is still open in this account",
            vec![insight("i1", PROACTIVE, ONGOING, None, &[], &[], &[])],
            (1, 0),
        ),
        (
            "an insight about an uncovered stack is still open in this account",
            vec![insight(
                "i1",
                REACTIVE,
                ONGOING,
                None,
                &["never-covered"],
                &[],
                &[],
            )],
            (0, 1),
        ),
        (
            "an insight about a tag pair counts like any other",
            vec![insight(
                "i1",
                PROACTIVE,
                ONGOING,
                None,
                &[],
                &[("Devops-guru-app", "web")],
                &[],
            )],
            (1, 0),
        ),
    ];

    for (name, insights, (proactive, reactive)) in cases {
        let emulator = Emulator::new();
        let setup = emulator
            .update_resource_collection("ADD", collection(&["watched"], &[]))
            .await;
        assert_eq!(
            setup.status, 200,
            "case {name:?}: the coverage setup must succeed, got {}",
            setup.raw
        );
        emulator.seed_insights(insights);

        let health = describe_account_health(&emulator).await;
        assert_eq!(
            health.status, 200,
            "case {name:?}: DescribeAccountHealth must succeed, got {}",
            health.raw
        );
        assert_eq!(
            (
                number_at(&health, "/OpenProactiveInsights"),
                number_at(&health, "/OpenReactiveInsights"),
            ),
            (Some(proactive), Some(reactive)),
            "case {name:?}: expected OpenProactiveInsights {proactive} and \
             OpenReactiveInsights {reactive}, got {}",
            health.raw
        );
        // The coverage drives its own members only: the insight tally must not move it.
        assert_eq!(
            number_at(&health, "/AnalyzedResourceCount"),
            Some(1),
            "case {name:?}: the one covered stack is one monitored resource, got {}",
            health.raw
        );
    }
}

/// RULE: the account-level open counts are the same tally the narrower reads of this one
/// store take, so `DescribeAccountHealth` cannot contradict `ListInsights` (the ONGOING
/// insights of a type) or a member's `InsightHealth` in
/// `DescribeResourceCollectionHealth`.
#[tokio::test]
async fn account_health_agrees_with_the_narrower_reads_of_the_same_store() {
    let emulator = Emulator::new();
    let setup = emulator
        .update_resource_collection("ADD", collection(&["my-stack"], &[]))
        .await;
    assert_eq!(
        setup.status, 200,
        "the coverage setup must succeed, got {}",
        setup.raw
    );
    emulator.seed_insights(vec![
        insight("p1", PROACTIVE, ONGOING, None, &["my-stack"], &[], &[]),
        insight("p2", PROACTIVE, ONGOING, None, &["my-stack"], &[], &[]),
        insight("r1", REACTIVE, ONGOING, None, &["my-stack"], &[], &[]),
    ]);

    let health = describe_account_health(&emulator).await;
    let member = emulator
        .call(
            "GET",
            "/accounts/health/resource-collection/AWS_CLOUD_FORMATION",
            None,
        )
        .await;

    for (insight_type, account_member, member_member, expected) in [
        ("PROACTIVE", "/OpenProactiveInsights", "/CloudFormation/0/Insight/OpenProactiveInsights", 2),
        ("REACTIVE", "/OpenReactiveInsights", "/CloudFormation/0/Insight/OpenReactiveInsights", 1),
    ] {
        let listed = emulator
            .call(
                "POST",
                "/insights",
                Some(&json!({ "StatusFilter": { "Ongoing": { "Type": insight_type } } })),
            )
            .await;
        let list_pointer = if insight_type == "PROACTIVE" {
            "/ProactiveInsights"
        } else {
            "/ReactiveInsights"
        };
        let listed_count = listed
            .at(list_pointer)
            .and_then(Value::as_array)
            .map(|items| items.len() as i64);
        assert_eq!(
            listed_count,
            Some(expected),
            "case {insight_type:?}: ListInsights must list {expected} ongoing insights, got {}",
            listed.raw
        );
        assert_eq!(
            number_at(&health, account_member),
            Some(expected),
            "case {insight_type:?}: DescribeAccountHealth must report {expected} open \
             insights, as ListInsights does, got {}",
            health.raw
        );
        assert_eq!(
            number_at(&member, member_member),
            Some(expected),
            "case {insight_type:?}: the covered member must report the same {expected} open \
             insights, got {}",
            member.raw
        );
    }
}

// ---------------------------------------------------------------------------
// The analysed-metric count.
//
// A metric enters this store only as the evidence an anomaly carries in its
// `SourceDetails`, so these rows seed the `anomalies` map the analysis would write.
// ---------------------------------------------------------------------------

/// One CloudWatch metric, as an anomaly's source details name it. `Stat` and `Period` are
/// how the metric was READ, not which metric it is, so a row may vary them.
fn cloud_watch_metric(
    namespace: &str,
    metric_name: &str,
    dimensions: &[(&str, &str)],
    stat: &str,
    period: i32,
) -> CloudWatchMetricsDetail {
    CloudWatchMetricsDetail {
        metric_name: Some(metric_name.to_string()),
        namespace: Some(namespace.to_string()),
        dimensions: Some(
            dimensions
                .iter()
                .map(|(name, value)| CloudWatchMetricsDimension {
                    name: Some(name.to_string()),
                    value: Some(value.to_string()),
                })
                .collect(),
        ),
        stat: Some(stat.to_string()),
        period,
        ..CloudWatchMetricsDetail::default()
    }
}

/// One Performance Insights metric, named by its display name and the query that selects
/// it.
fn performance_insights_metric(
    display_name: &str,
    metric: &str,
    group: &str,
) -> PerformanceInsightsMetricsDetail {
    PerformanceInsightsMetricsDetail {
        metric_display_name: Some(display_name.to_string()),
        metric_query: Some(PerformanceInsightsMetricQuery {
            metric: Some(metric.to_string()),
            group_by: Some(PerformanceInsightsMetricDimensionGroup {
                group: Some(group.to_string()),
                ..PerformanceInsightsMetricDimensionGroup::default()
            }),
            ..PerformanceInsightsMetricQuery::default()
        }),
        ..PerformanceInsightsMetricsDetail::default()
    }
}

/// One anomaly carrying `cloud_watch` and `performance_insights` as its source details,
/// or no source details at all when it names no metric.
fn anomaly(
    id: &str,
    cloud_watch: Vec<CloudWatchMetricsDetail>,
    performance_insights: Vec<PerformanceInsightsMetricsDetail>,
) -> AnomalyRecord {
    let names_metrics = !cloud_watch.is_empty() || !performance_insights.is_empty();
    AnomalyRecord {
        id: id.to_string(),
        severity: "HIGH".to_string(),
        status: "ONGOING".to_string(),
        source_details: names_metrics.then_some(AnomalySourceDetails {
            cloud_watch_metrics: (!cloud_watch.is_empty()).then_some(cloud_watch),
            performance_insights_metrics: (!performance_insights.is_empty())
                .then_some(performance_insights),
        }),
        ..AnomalyRecord::default()
    }
}

/// One row of the metric table: the case name, the anomalies seeded, and the
/// `MetricsAnalyzed` expected.
type MetricCase<'a> = (&'a str, Vec<AnomalyRecord>, i64);

/// RULE: `MetricsAnalyzed` is how many DISTINCT metrics the store records the analysis as
/// having read — the metrics named by the source details of the anomalies it holds,
/// counted by metric identity, so one metric that several anomalies name is one metric.
///
/// The rows differ along the metric source, how many metrics an anomaly names, how many
/// anomalies name them, which part of a metric's identity differs, the order a dimension
/// set is listed in, and the statistic and period the metric was read at.
#[tokio::test]
async fn account_health_counts_the_metrics_the_stored_anomalies_name() {
    let cases: Vec<MetricCase> = vec![
        ("no anomaly at all", vec![], 0),
        (
            "an anomaly with no source details names no metric",
            vec![anomaly("a1", vec![], vec![])],
            0,
        ),
        (
            "one anomaly naming one metric",
            vec![anomaly(
                "a1",
                vec![cloud_watch_metric(
                    "AWS/RDS",
                    "CPUUtilization",
                    &[("DBInstanceIdentifier", "db-1")],
                    "Average",
                    60,
                )],
                vec![],
            )],
            1,
        ),
        (
            "one anomaly naming two metrics",
            vec![anomaly(
                "a1",
                vec![
                    cloud_watch_metric("AWS/RDS", "CPUUtilization", &[], "Average", 60),
                    cloud_watch_metric("AWS/RDS", "ReadLatency", &[], "Average", 60),
                ],
                vec![],
            )],
            2,
        ),
        (
            "two anomalies naming the same metric count it once",
            vec![
                anomaly(
                    "a1",
                    vec![cloud_watch_metric("AWS/RDS", "CPUUtilization", &[], "Average", 60)],
                    vec![],
                ),
                anomaly(
                    "a2",
                    vec![cloud_watch_metric("AWS/RDS", "CPUUtilization", &[], "Average", 60)],
                    vec![],
                ),
            ],
            1,
        ),
        (
            "the same metric name in two namespaces is two metrics",
            vec![anomaly(
                "a1",
                vec![
                    cloud_watch_metric("AWS/RDS", "CPUUtilization", &[], "Average", 60),
                    cloud_watch_metric("AWS/EC2", "CPUUtilization", &[], "Average", 60),
                ],
                vec![],
            )],
            2,
        ),
        (
            "one metric on two dimension sets is two metrics",
            vec![anomaly(
                "a1",
                vec![
                    cloud_watch_metric(
                        "AWS/RDS",
                        "CPUUtilization",
                        &[("DBInstanceIdentifier", "db-1")],
                        "Average",
                        60,
                    ),
                    cloud_watch_metric(
                        "AWS/RDS",
                        "CPUUtilization",
                        &[("DBInstanceIdentifier", "db-2")],
                        "Average",
                        60,
                    ),
                ],
                vec![],
            )],
            2,
        ),
        (
            "one dimension set listed in either order is one metric",
            vec![
                anomaly(
                    "a1",
                    vec![cloud_watch_metric(
                        "AWS/RDS",
                        "CPUUtilization",
                        &[("DBInstanceIdentifier", "db-1"), ("Role", "WRITER")],
                        "Average",
                        60,
                    )],
                    vec![],
                ),
                anomaly(
                    "a2",
                    vec![cloud_watch_metric(
                        "AWS/RDS",
                        "CPUUtilization",
                        &[("Role", "WRITER"), ("DBInstanceIdentifier", "db-1")],
                        "Average",
                        60,
                    )],
                    vec![],
                ),
            ],
            1,
        ),
        (
            "one metric read at two statistics and periods is one metric",
            vec![anomaly(
                "a1",
                vec![
                    cloud_watch_metric("AWS/RDS", "CPUUtilization", &[], "Average", 60),
                    cloud_watch_metric("AWS/RDS", "CPUUtilization", &[], "Maximum", 300),
                ],
                vec![],
            )],
            1,
        ),
        (
            "metrics from both sources are counted together",
            vec![anomaly(
                "a1",
                vec![cloud_watch_metric("AWS/RDS", "CPUUtilization", &[], "Average", 60)],
                vec![
                    performance_insights_metric("db.load.avg", "db.load.avg", "db.wait_event"),
                    performance_insights_metric("db.sampledload.avg", "db.load.avg", "db.sql"),
                ],
            )],
            3,
        ),
        (
            "the same Performance Insights metric named twice is one metric",
            vec![
                anomaly(
                    "a1",
                    vec![],
                    vec![performance_insights_metric(
                        "db.load.avg",
                        "db.load.avg",
                        "db.wait_event",
                    )],
                ),
                anomaly(
                    "a2",
                    vec![],
                    vec![performance_insights_metric(
                        "db.load.avg",
                        "db.load.avg",
                        "db.wait_event",
                    )],
                ),
            ],
            1,
        ),
    ];

    for (name, anomalies, expected) in cases {
        let emulator = Emulator::new();
        emulator.seed_anomalies(anomalies);

        let health = describe_account_health(&emulator).await;
        assert_eq!(
            health.status, 200,
            "case {name:?}: DescribeAccountHealth must succeed, got {}",
            health.raw
        );
        assert_eq!(
            number_at(&health, "/MetricsAnalyzed"),
            Some(expected),
            "case {name:?}: expected MetricsAnalyzed {expected}, got {}",
            health.raw
        );
    }
}


// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeAccountHealth`.
//!
//! The emulator is driven over the wire through the same [`Emulator`] harness the
//! configuration-lifecycle suite uses, so the assertions are about what a client sees.
//!
//! RULE pinned here: `DescribeAccountHealth` reports the account's analysed-resource
//! population from the persisted resource collection — one monitored resource per
//! covered CloudFormation stack and one per `(AppBoundaryKey, TagValue)` pair — as both
//! `AnalyzedResourceCount` and `ResourceHours` (one resource-analysis hour per monitored
//! resource in the last hour), while `MetricsAnalyzed`, `OpenProactiveInsights` and
//! `OpenReactiveInsights` stay zero because no analysis runs in this emulator. The read
//! changes nothing a later caller sees.

use serde_json::{json, Value};

use super::configuration_lifecycle::{Emulator, Reply};

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
        for analysis_member in ["MetricsAnalyzed", "OpenProactiveInsights", "OpenReactiveInsights"] {
            assert_eq!(
                number_at(&health, &format!("/{analysis_member}")),
                Some(0),
                "case {:?}: expected {analysis_member} 0 — no analysis runs in this emulator — got {}",
                case.name,
                health.raw
            );
        }
    }
}

/// RULE: `DescribeAccountHealth` is a report, so repeating it answers the same numbers
/// and leaves the coverage a later reader sees untouched.
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
}

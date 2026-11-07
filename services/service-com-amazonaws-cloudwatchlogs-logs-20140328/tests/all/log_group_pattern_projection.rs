// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the projection `DescribeLogGroups` answers a `logGroupNamePattern`
//! lookup with.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: a lookup by `logGroupNamePattern` reports a REDUCED projection of
//! each log group — exactly the identity and classification members `arn`, `creationTime`,
//! `logGroupArn`, `logGroupClass`, `logGroupName` and `metricFilterCount` — and none of the
//! members describing what the group holds or how it is configured, which the prefix lookup
//! of the same group carries; the members both lookups report agree.
//!
//! The table spans the classes the rule generalises over: which of the configuration members
//! a group actually carries (its class, its retention, its key, its filters, its events) does
//! not change which members the pattern lookup reports.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, Logs};

/// The members a pattern lookup reports, from the reduced projection the rule names.
const PATTERN_MEMBERS: [&str; 6] = [
    "arn",
    "creationTime",
    "logGroupArn",
    "logGroupClass",
    "logGroupName",
    "metricFilterCount",
];

/// Members the prefix lookup of a configured group carries and the pattern lookup does not:
/// what the group HOLDS and how it is configured.
const PREFIX_ONLY_MEMBERS: [&str; 5] = [
    "retentionInDays",
    "storedBytes",
    "kmsKeyId",
    "dataProtectionStatus",
    "deletionProtectionEnabled",
];

/// A KMS key ARN of the form `CreateLogGroup$kmsKeyId` documents ("The Amazon Resource Name
/// (ARN) of the KMS key to use when encrypting log data").
const CMK: &str = "arn:aws:kms:eu-west-2:000000000000:key/11112222-3333-4444-5555-666677778888";

/// One log group of the table, and what it is configured with.
struct Row {
    /// The group's name; every row's name carries [`PATTERN`] so one lookup finds them all.
    name: &'static str,
    /// `logGroupClass`, or `None` to take the documented default.
    class: Option<&'static str>,
    /// `retentionInDays`, when the row puts a retention policy.
    retention: Option<i64>,
    /// Whether the row associates a customer managed key with the group.
    encrypted: bool,
    /// How many metric filters the row attaches to the group.
    metric_filters: usize,
    /// Whether the row writes a log event into the group, so it carries stored bytes.
    holds_events: bool,
}

/// The substring every row's name carries.
const PATTERN: &str = "projection";

/// The one entry a lookup reports for `name`.
fn entry(page: &Value, name: &str) -> Value {
    page.get("logGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeLogGroups must return logGroups, got {page}"))
        .iter()
        .find(|group| group.get("logGroupName").and_then(Value::as_str) == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("case {name}: the lookup must report the group, got {page}"))
}

/// The member names one reported entry carries, sorted.
fn reported_members(entry: &Value) -> Vec<String> {
    let mut members: Vec<String> = entry
        .as_object()
        .unwrap_or_else(|| panic!("a reported log group is an object, got {entry}"))
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(member, _)| member.clone())
        .collect();
    members.sort();
    members
}

/// A pattern lookup reports exactly the reduced projection, whatever the group carries, and
/// agrees with the prefix lookup on every member it reports.
#[tokio::test]
async fn a_pattern_lookup_reports_exactly_the_reduced_projection() {
    let rows: [Row; 4] = [
        Row {
            name: "projection/bare",
            class: None,
            retention: None,
            encrypted: false,
            metric_filters: 0,
            holds_events: false,
        },
        Row {
            name: "projection/configured",
            class: Some("STANDARD"),
            retention: Some(14),
            encrypted: true,
            metric_filters: 2,
            holds_events: true,
        },
        Row {
            name: "projection/infrequent",
            class: Some("INFREQUENT_ACCESS"),
            retention: Some(30),
            encrypted: false,
            metric_filters: 0,
            holds_events: false,
        },
        Row {
            name: "projection/filtered",
            class: None,
            retention: None,
            encrypted: false,
            metric_filters: 1,
            holds_events: true,
        },
    ];

    let logs = Logs::new();
    for row in &rows {
        let mut create = json!({"logGroupName": row.name});
        if let Some(class) = row.class {
            create["logGroupClass"] = json!(class);
        }
        if row.encrypted {
            create["kmsKeyId"] = json!(CMK);
        }
        logs.ok("CreateLogGroup", &create).await;
        if let Some(days) = row.retention {
            logs.ok(
                "PutRetentionPolicy",
                &json!({"logGroupName": row.name, "retentionInDays": days}),
            )
            .await;
        }
        for index in 0..row.metric_filters {
            logs.ok(
                "PutMetricFilter",
                &json!({
                    "logGroupName": row.name,
                    "filterName": format!("filter-{index}"),
                    "filterPattern": "ERROR",
                    "metricTransformations": [{
                        "metricName": format!("Errors{index}"),
                        "metricNamespace": "Projection",
                        "metricValue": "1",
                    }],
                }),
            )
            .await;
        }
        if row.holds_events {
            logs.ok(
                "CreateLogStream",
                &json!({"logGroupName": row.name, "logStreamName": "stream"}),
            )
            .await;
            logs.ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": row.name,
                    "logStreamName": "stream",
                    "logEvents": batch(&[(base_ms(), "ERROR something happened")]),
                }),
            )
            .await;
        }
    }

    let by_pattern = logs
        .ok("DescribeLogGroups", &json!({"logGroupNamePattern": PATTERN}))
        .await;

    for row in &rows {
        let pattern_entry = entry(&by_pattern, row.name);
        let mut expected: Vec<String> = PATTERN_MEMBERS.iter().map(|m| (*m).to_string()).collect();
        expected.sort();
        assert_eq!(
            reported_members(&pattern_entry),
            expected,
            "case {}: expected exactly the reduced projection, got {pattern_entry}",
            row.name
        );

        let by_prefix = logs
            .ok(
                "DescribeLogGroups",
                &json!({"logGroupNamePrefix": row.name}),
            )
            .await;
        let prefix_entry = entry(&by_prefix, row.name);
        for member in PATTERN_MEMBERS {
            assert_eq!(
                pattern_entry.get(member),
                prefix_entry.get(member),
                "case {} ({member}): the two lookups must agree, got {pattern_entry} and \
                 {prefix_entry}",
                row.name
            );
        }
        // The reduced projection is reduced: what the group holds and how it is configured is
        // reported by the prefix lookup only.
        for member in PREFIX_ONLY_MEMBERS {
            assert!(
                pattern_entry.get(member).is_none_or(Value::is_null),
                "case {} ({member}): a pattern lookup reports no {member}, got {pattern_entry}",
                row.name
            );
        }
        // The counts and classifications the pattern lookup does report are the group's own.
        assert_eq!(
            pattern_entry.get("metricFilterCount").and_then(Value::as_i64),
            Some(i64::try_from(row.metric_filters).unwrap_or(0)),
            "case {}: metricFilterCount follows the filters attached, got {pattern_entry}",
            row.name
        );
        assert_eq!(
            pattern_entry.get("logGroupClass").and_then(Value::as_str),
            Some(row.class.unwrap_or("STANDARD")),
            "case {}: logGroupClass follows the create, got {pattern_entry}",
            row.name
        );
    }
}

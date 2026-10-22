// SPDX-License-Identifier: CC0-1.0

//! A workspace's CONFIGURATION, end to end over HTTP: what
//! `DescribeWorkspaceConfiguration` reports for a workspace nobody has configured, what
//! `UpdateWorkspaceConfiguration` persists, and which requests it refuses.
//!
//! The rule under test is one sentence: `UpdateWorkspaceConfiguration` writes the
//! members the request carries onto the workspace's stored configuration, leaving the
//! ones it omits alone, and `DescribeWorkspaceConfiguration` reports that stored record
//! — so every assertion below reads a value back out of the service rather than out of
//! the request it was sent in.
//!
//! The request harness is the one `workspace_lifecycle` defines, so these suites drive
//! the same `MultiServiceRouter::dispatch` path.

use super::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::WORKSPACE_CONFIGURATION_STATUS_CODE_VALUES;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    CONFIG_STATUS_ACTIVE, CONFIG_STATUS_UPDATING, DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS,
    DEFAULT_RETENTION_PERIOD_IN_DAYS, DEFAULT_RULE_QUERY_OFFSET_IN_SECONDS,
    LABEL_SET_LIMITS_PER_WORKSPACE,
};
use std::collections::BTreeMap;

/// `GET /workspaces/{workspaceId}/configuration`.
async fn describe(amp: &Amp, region: &str, workspace_id: &str) -> Reply {
    amp.call(
        "GET",
        &format!("/workspaces/{workspace_id}/configuration"),
        region,
        "",
    )
    .await
}

/// `PATCH /workspaces/{workspaceId}/configuration`.
async fn update(amp: &Amp, region: &str, workspace_id: &str, body: &serde_json::Value) -> Reply {
    amp.call(
        "PATCH",
        &format!("/workspaces/{workspace_id}/configuration"),
        region,
        &body.to_string(),
    )
    .await
}

/// One `limitsPerLabelSet` entry, as the wire carries it.
fn label_set(labels: &[(&str, &str)], max_series: Option<i64>) -> serde_json::Value {
    let label_set: serde_json::Map<String, serde_json::Value> = labels
        .iter()
        .map(|(name, value)| ((*name).to_string(), serde_json::json!(value)))
        .collect();
    let limits = match max_series {
        Some(max_series) => serde_json::json!({ "maxSeries": max_series }),
        None => serde_json::json!({}),
    };
    serde_json::json!({ "labelSet": label_set, "limits": limits })
}

/// One label set and the limit for it, as a case WRITES it: labels in source order.
type LabelSetCase<'a> = (&'a [(&'a str, &'a str)], Option<i64>);

/// One label set and the limit for it, as a reply REPORTS it: labels sorted, so a
/// `LabelSet` map's iteration order never decides whether a case passes.
type StoredLabelSet = (BTreeMap<String, String>, Option<i64>);

/// A configuration as a test compares it: the label sets normalised to a comparable
/// form.
#[derive(Debug, PartialEq, Eq)]
struct Config {
    status: String,
    retention_period_in_days: Option<i64>,
    limits: Vec<StoredLabelSet>,
    out_of_order_time_window_in_seconds: Option<i64>,
    rule_query_offset_in_seconds: Option<i64>,
}

/// The configuration a `DescribeWorkspaceConfiguration` reply reports.
fn config_of(reply: &Reply) -> Config {
    let configuration = &reply.body["workspaceConfiguration"];
    Config {
        status: text(&configuration["status"]["statusCode"]),
        retention_period_in_days: configuration["retentionPeriodInDays"].as_i64(),
        limits: configuration["limitsPerLabelSet"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        let labels = entry["labelSet"]
                            .as_object()
                            .map(|map| {
                                map.iter()
                                    .map(|(name, value)| (name.clone(), text(value)))
                                    .collect()
                            })
                            .unwrap_or_default();
                        (labels, entry["limits"]["maxSeries"].as_i64())
                    })
                    .collect()
            })
            .unwrap_or_default(),
        out_of_order_time_window_in_seconds: configuration["outOfOrderTimeWindowInSeconds"]
            .as_i64(),
        rule_query_offset_in_seconds: configuration["ruleQueryOffsetInSeconds"].as_i64(),
    }
}

/// The retention period an unconfigured workspace reports.
fn default_retention() -> Option<i64> {
    Some(i64::from(DEFAULT_RETENTION_PERIOD_IN_DAYS))
}

/// The out-of-order sample window an unconfigured workspace reports.
fn default_out_of_order_window() -> Option<i64> {
    Some(i64::from(DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS))
}

/// The rule-evaluation query offset an unconfigured workspace reports.
fn default_rule_query_offset() -> Option<i64> {
    Some(i64::from(DEFAULT_RULE_QUERY_OFFSET_IN_SECONDS))
}

/// The label sets a case expects, in the comparable form [`Config`] holds.
fn expected_limits(entries: &[LabelSetCase<'_>]) -> Vec<StoredLabelSet> {
    entries
        .iter()
        .map(|(labels, max_series)| {
            (
                labels
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect(),
                *max_series,
            )
        })
        .collect()
}

/// Both statuses this crate reports for a workspace configuration are values the
/// modelled `WorkspaceConfigurationStatusCode` enum declares — the catalog comes from
/// the model, not from this test.
#[test]
fn reported_statuses_are_modelled_configuration_status_codes() {
    for status in [CONFIG_STATUS_ACTIVE, CONFIG_STATUS_UPDATING] {
        assert!(
            WORKSPACE_CONFIGURATION_STATUS_CODE_VALUES.contains(&status),
            "case {status:?}: not a modelled WorkspaceConfigurationStatusCode \
             ({WORKSPACE_CONFIGURATION_STATUS_CODE_VALUES:?})"
        );
    }
}

/// A workspace nobody has configured already HAS a configuration: it is active, it
/// enforces no label set limit, and it retains metrics for the service's default period.
/// One row per region a workspace can be created in, since the configuration belongs to
/// the workspace rather than to the request.
#[tokio::test]
async fn a_fresh_workspace_reports_the_default_configuration() {
    let amp = Amp::new();
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let workspace_id = amp.active_workspace(region, "defaults").await;
        let described = describe(&amp, region, &workspace_id).await;
        assert_eq!(
            described.status, 200,
            "case {region:?}: describe must answer 200, got {described:?}"
        );
        assert_eq!(
            config_of(&described),
            Config {
                status: CONFIG_STATUS_ACTIVE.to_string(),
                retention_period_in_days: default_retention(),
                limits: Vec::new(),
                out_of_order_time_window_in_seconds: default_out_of_order_window(),
                rule_query_offset_in_seconds: default_rule_query_offset(),
            },
            "case {region:?}: an unconfigured workspace must report the default configuration"
        );
    }
}

/// An update persists every member it carries and a describe reads them back: one row
/// per thing the rule generalises over — which members the request names, how many label
/// sets it carries, how many labels each of those holds, whether a label set names a
/// limit at all, and the published bound of each numeric member.
#[tokio::test]
async fn update_persists_every_member_and_describe_reads_it_back() {
    struct Case {
        name: &'static str,
        request: serde_json::Value,
        limits: Vec<StoredLabelSet>,
        retention_period_in_days: Option<i64>,
        out_of_order_time_window_in_seconds: Option<i64>,
        rule_query_offset_in_seconds: Option<i64>,
    }

    let cases = vec![
        Case {
            // `retentionPeriodInDays` is modelled `@range(min: 1)`, so 1 is the
            // published lower bound.
            name: "retention period at the published minimum",
            request: serde_json::json!({ "retentionPeriodInDays": 1 }),
            limits: Vec::new(),
            retention_period_in_days: Some(1),
            out_of_order_time_window_in_seconds: default_out_of_order_window(),
            rule_query_offset_in_seconds: default_rule_query_offset(),
        },
        Case {
            name: "retention period well above the default",
            request: serde_json::json!({ "retentionPeriodInDays": 365 }),
            limits: Vec::new(),
            retention_period_in_days: Some(365),
            out_of_order_time_window_in_seconds: default_out_of_order_window(),
            rule_query_offset_in_seconds: default_rule_query_offset(),
        },
        Case {
            // `LimitsPerLabelSetEntry$maxSeries`: "Setting this to 0 causes no label set
            // limit to be enforced" — a documented value, and the modelled
            // `@range(min: 0)` lower bound.
            name: "one label set with maxSeries at the published minimum",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", "finance")], Some(0))],
            }),
            limits: expected_limits(&[(&[("team", "finance")], Some(0))]),
            retention_period_in_days: default_retention(),
            out_of_order_time_window_in_seconds: default_out_of_order_window(),
            rule_query_offset_in_seconds: default_rule_query_offset(),
        },
        Case {
            name: "three label sets of one, two and three labels, one without a limit",
            request: serde_json::json!({
                "limitsPerLabelSet": [
                    label_set(&[("env", "prod")], Some(100_000)),
                    label_set(&[("env", "staging"), ("team", "obs")], None),
                    label_set(
                        &[("env", "dev"), ("team", "obs"), ("tier", "free")],
                        Some(1),
                    ),
                ],
            }),
            limits: expected_limits(&[
                (&[("env", "prod")], Some(100_000)),
                (&[("env", "staging"), ("team", "obs")], None),
                (
                    &[("env", "dev"), ("team", "obs"), ("tier", "free")],
                    Some(1),
                ),
            ]),
            retention_period_in_days: default_retention(),
            out_of_order_time_window_in_seconds: default_out_of_order_window(),
            rule_query_offset_in_seconds: default_rule_query_offset(),
        },
        Case {
            // `outOfOrderTimeWindowInSeconds` and `ruleQueryOffsetInSeconds` are both
            // modelled `@range(min: 0)`, so 0 is the published lower bound of each.
            name: "every member, with both windows at the published minimum",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", "search")], Some(25))],
                "retentionPeriodInDays": 30,
                "outOfOrderTimeWindowInSeconds": 0,
                "ruleQueryOffsetInSeconds": 0,
            }),
            limits: expected_limits(&[(&[("team", "search")], Some(25))]),
            retention_period_in_days: Some(30),
            out_of_order_time_window_in_seconds: Some(0),
            rule_query_offset_in_seconds: Some(0),
        },
        Case {
            name: "every member, with windows well above the minimum",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", "ads"), ("env", "prod")], Some(5_000))],
                "retentionPeriodInDays": 150,
                "outOfOrderTimeWindowInSeconds": 600,
                "ruleQueryOffsetInSeconds": 30,
            }),
            limits: expected_limits(&[(&[("team", "ads"), ("env", "prod")], Some(5_000))]),
            retention_period_in_days: Some(150),
            out_of_order_time_window_in_seconds: Some(600),
            rule_query_offset_in_seconds: Some(30),
        },
        Case {
            name: "an empty label set list enforces no limit at all",
            request: serde_json::json!({ "limitsPerLabelSet": [] }),
            limits: Vec::new(),
            retention_period_in_days: default_retention(),
            out_of_order_time_window_in_seconds: default_out_of_order_window(),
            rule_query_offset_in_seconds: default_rule_query_offset(),
        },
        Case {
            // `LimitsPerLabelSet$labelSet`: for the metric name label `__name__` the
            // value is a metric name, which may carry `:`.
            name: "a label set keyed by the metric name label",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("__name__", "http_requests:rate5m")], Some(7))],
            }),
            limits: expected_limits(&[(&[("__name__", "http_requests:rate5m")], Some(7))]),
            retention_period_in_days: default_retention(),
            out_of_order_time_window_in_seconds: default_out_of_order_window(),
            rule_query_offset_in_seconds: default_rule_query_offset(),
        },
    ];

    let amp = Amp::new();
    let region = "us-west-2";
    for case in cases {
        let name = case.name;
        let workspace_id = amp.active_workspace(region, "configured").await;

        let updated = update(&amp, region, &workspace_id, &case.request).await;
        assert_eq!(
            updated.status, 202,
            "case {name:?}: update must answer 202, got {updated:?}"
        );
        assert_eq!(
            text(&updated.body["status"]["statusCode"]),
            CONFIG_STATUS_UPDATING,
            "case {name:?}: an accepted update reports the configuration as updating, got {updated:?}"
        );

        // Straight after the call the configuration is still UPDATING, and it already
        // reports the values the update wrote.
        let updating = describe(&amp, region, &workspace_id).await;
        assert_eq!(
            updating.status, 200,
            "case {name:?}: describe must answer 200, got {updating:?}"
        );
        assert_eq!(
            config_of(&updating),
            Config {
                status: CONFIG_STATUS_UPDATING.to_string(),
                retention_period_in_days: case.retention_period_in_days,
                limits: case.limits.clone(),
                out_of_order_time_window_in_seconds: case.out_of_order_time_window_in_seconds,
                rule_query_offset_in_seconds: case.rule_query_offset_in_seconds,
            },
            "case {name:?}: a describe during the update must report the stored configuration"
        );

        // Once the clock owes the transition the configuration is ACTIVE again, with
        // the same stored values.
        wait_for_due_transition();
        let settled = describe(&amp, region, &workspace_id).await;
        assert_eq!(
            config_of(&settled),
            Config {
                status: CONFIG_STATUS_ACTIVE.to_string(),
                retention_period_in_days: case.retention_period_in_days,
                limits: case.limits.clone(),
                out_of_order_time_window_in_seconds: case.out_of_order_time_window_in_seconds,
                rule_query_offset_in_seconds: case.rule_query_offset_in_seconds,
            },
            "case {name:?}: the settled configuration must still report what was stored"
        );

        // A read changes nothing a later reader sees.
        let reread = describe(&amp, region, &workspace_id).await;
        assert_eq!(
            config_of(&reread),
            config_of(&settled),
            "case {name:?}: a second describe must report the same configuration"
        );
    }
}

/// An update writes the members it names and leaves the rest of the stored
/// configuration alone: one row per member written second, each asserting that what the
/// FIRST update stored survives.
#[tokio::test]
async fn an_update_leaves_the_members_it_omits_alone() {
    struct Case {
        name: &'static str,
        second: serde_json::Value,
        limits: Vec<StoredLabelSet>,
        retention_period_in_days: Option<i64>,
        out_of_order_time_window_in_seconds: Option<i64>,
        rule_query_offset_in_seconds: Option<i64>,
    }

    // What every case stores first.
    let first = serde_json::json!({
        "limitsPerLabelSet": [label_set(&[("team", "finance")], Some(42))],
        "retentionPeriodInDays": 90,
        "outOfOrderTimeWindowInSeconds": 120,
        "ruleQueryOffsetInSeconds": 15,
    });
    let first_limits = expected_limits(&[(&[("team", "finance")], Some(42))]);

    let cases = vec![
        Case {
            name: "retention period only",
            second: serde_json::json!({ "retentionPeriodInDays": 7 }),
            limits: first_limits.clone(),
            retention_period_in_days: Some(7),
            out_of_order_time_window_in_seconds: Some(120),
            rule_query_offset_in_seconds: Some(15),
        },
        Case {
            name: "label sets only",
            second: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("env", "prod")], Some(9))],
            }),
            limits: expected_limits(&[(&[("env", "prod")], Some(9))]),
            retention_period_in_days: Some(90),
            out_of_order_time_window_in_seconds: Some(120),
            rule_query_offset_in_seconds: Some(15),
        },
        Case {
            name: "retention period and the out of order window",
            second: serde_json::json!({
                "retentionPeriodInDays": 10,
                "outOfOrderTimeWindowInSeconds": 0,
            }),
            limits: first_limits.clone(),
            retention_period_in_days: Some(10),
            out_of_order_time_window_in_seconds: Some(0),
            rule_query_offset_in_seconds: Some(15),
        },
        Case {
            name: "retention period and the rule query offset",
            second: serde_json::json!({
                "retentionPeriodInDays": 11,
                "ruleQueryOffsetInSeconds": 300,
            }),
            limits: first_limits.clone(),
            retention_period_in_days: Some(11),
            out_of_order_time_window_in_seconds: Some(120),
            rule_query_offset_in_seconds: Some(300),
        },
        Case {
            name: "an empty label set list clears the stored limits only",
            second: serde_json::json!({ "limitsPerLabelSet": [] }),
            limits: Vec::new(),
            retention_period_in_days: Some(90),
            out_of_order_time_window_in_seconds: Some(120),
            rule_query_offset_in_seconds: Some(15),
        },
    ];

    let amp = Amp::new();
    let region = "us-east-2";
    for case in cases {
        let name = case.name;
        let workspace_id = amp.active_workspace(region, "patched").await;

        let first_reply = update(&amp, region, &workspace_id, &first).await;
        assert_eq!(
            first_reply.status, 202,
            "case {name:?}: the first update must answer 202, got {first_reply:?}"
        );
        wait_for_due_transition();

        let second_reply = update(&amp, region, &workspace_id, &case.second).await;
        assert_eq!(
            second_reply.status, 202,
            "case {name:?}: the second update must answer 202, got {second_reply:?}"
        );
        wait_for_due_transition();

        let described = describe(&amp, region, &workspace_id).await;
        assert_eq!(
            config_of(&described),
            Config {
                status: CONFIG_STATUS_ACTIVE.to_string(),
                retention_period_in_days: case.retention_period_in_days,
                limits: case.limits.clone(),
                out_of_order_time_window_in_seconds: case.out_of_order_time_window_in_seconds,
                rule_query_offset_in_seconds: case.rule_query_offset_in_seconds,
            },
            "case {name:?}: the second update must leave the members it omits alone"
        );
    }
}

/// Which requests the service refuses, and which it accepts: the rules
/// `LimitsPerLabelSet$labelSet` and the operation's own documentation state but the
/// model cannot express, beside the published bounds the model DOES carry — one row per
/// rule, each with an accepted row so no refusal is wider than its rule.
#[tokio::test]
async fn update_refuses_a_request_that_breaks_a_documented_rule() {
    struct Case {
        name: &'static str,
        request: serde_json::Value,
        status: u16,
    }

    let cases = vec![
        Case {
            // "You must specify at least one of limitsPerLabelSet or
            // retentionPeriodInDays for the request to be valid."
            name: "no member at all",
            request: serde_json::json!({}),
            status: 400,
        },
        Case {
            name: "neither label sets nor a retention period",
            request: serde_json::json!({
                "outOfOrderTimeWindowInSeconds": 60,
                "ruleQueryOffsetInSeconds": 60,
            }),
            status: 400,
        },
        Case {
            name: "label sets alone satisfy the at-least-one rule",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", "obs")], Some(1))],
            }),
            status: 202,
        },
        Case {
            name: "a retention period alone satisfies the at-least-one rule",
            request: serde_json::json!({ "retentionPeriodInDays": 2 }),
            status: 202,
        },
        Case {
            // "Label values ... must contain at least one character that isn't
            // whitespace."
            name: "a label value that is nothing but whitespace",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", "  ")], Some(1))],
            }),
            status: 400,
        },
        Case {
            name: "a label value with whitespace around it keeps at least one other character",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", " obs ")], Some(1))],
            }),
            status: 202,
        },
        Case {
            // "ASCII control characters are not accepted."
            name: "a label value carrying an ASCII control character",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", "ob\u{1}s")], Some(1))],
            }),
            status: 400,
        },
        Case {
            // "Label values accept ASCII characters."
            name: "a label value outside ASCII",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", "observabilité")], Some(1))],
            }),
            status: 400,
        },
        Case {
            // "If the label name is metric name label __name__, then the metric part of
            // the name must conform to the following pattern: [a-zA-Z_:][a-zA-Z0-9_:]*"
            name: "a metric name starting with a digit",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("__name__", "5xx_errors")], Some(1))],
            }),
            status: 400,
        },
        Case {
            name: "a metric name carrying a character outside the pattern",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("__name__", "http-requests")], Some(1))],
            }),
            status: 400,
        },
        Case {
            name: "a metric name starting with an underscore and carrying a colon",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("__name__", "_http:requests_total")], Some(1))],
            }),
            status: 202,
        },
        Case {
            // The same value is fine for any other label name: the pattern is the
            // metric name's, not every value's.
            name: "a hyphenated value under a label name that is not the metric name",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", "http-requests")], Some(1))],
            }),
            status: 202,
        },
        Case {
            // `retentionPeriodInDays` is modelled `@range(min: 1)`; 0 is one past it.
            name: "a retention period one past the published minimum",
            request: serde_json::json!({ "retentionPeriodInDays": 0 }),
            status: 400,
        },
        Case {
            // `maxSeries` is modelled `@range(min: 0)`; -1 is one past it.
            name: "a label set limit one past the published minimum",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", "obs")], Some(-1))],
            }),
            status: 400,
        },
        Case {
            // `outOfOrderTimeWindowInSeconds` is modelled `@range(min: 0)`.
            name: "an out of order window one past the published minimum",
            request: serde_json::json!({
                "retentionPeriodInDays": 5,
                "outOfOrderTimeWindowInSeconds": -1,
            }),
            status: 400,
        },
        Case {
            // `ruleQueryOffsetInSeconds` is modelled `@range(min: 0)`.
            name: "a rule query offset one past the published minimum",
            request: serde_json::json!({
                "retentionPeriodInDays": 5,
                "ruleQueryOffsetInSeconds": -1,
            }),
            status: 400,
        },
        Case {
            // `LabelName` is modelled with the pattern `^[a-zA-Z_][a-zA-Z0-9_]*$`.
            name: "a label name outside the modelled pattern",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("1team", "obs")], Some(1))],
            }),
            status: 400,
        },
        Case {
            // `LabelValue` is modelled `@length(min: 1)`.
            name: "an empty label value",
            request: serde_json::json!({
                "limitsPerLabelSet": [label_set(&[("team", "")], Some(1))],
            }),
            status: 400,
        },
    ];

    let amp = Amp::new();
    let region = "eu-central-1";
    for case in cases {
        let name = case.name;
        // A fresh workspace per row: an accepted row writes, and a refused one must be
        // refused on its own merits rather than on an earlier row's pending update.
        let workspace_id = amp.active_workspace(region, "validated").await;
        let reply = update(&amp, region, &workspace_id, &case.request).await;
        assert_eq!(
            reply.status, case.status,
            "case {name:?}: expected HTTP {}, got {reply:?}",
            case.status
        );
        if case.status == 400 {
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {name:?}: a refused request must answer ValidationException, got {reply:?}"
            );
            // Nothing was written: the configuration is untouched and still active.
            let described = describe(&amp, region, &workspace_id).await;
            assert_eq!(
                config_of(&described),
                Config {
                    status: CONFIG_STATUS_ACTIVE.to_string(),
                    retention_period_in_days: default_retention(),
                    limits: Vec::new(),
                    out_of_order_time_window_in_seconds: default_out_of_order_window(),
                    rule_query_offset_in_seconds: default_rule_query_offset(),
                },
                "case {name:?}: a refused update must persist nothing"
            );
        }
    }
}

/// The store is bounded: a list of exactly `LABEL_SET_LIMITS_PER_WORKSPACE` label sets
/// is accepted, one past it is refused with the modelled `ServiceQuotaExceededException`
/// (402), and the refusal stores nothing.
#[tokio::test]
async fn update_refuses_more_label_sets_than_the_store_bounds() {
    let amp = Amp::new();
    let region = "us-east-1";

    for (name, count, status) in [
        ("exactly the bound", LABEL_SET_LIMITS_PER_WORKSPACE, 202u16),
        ("one past the bound", LABEL_SET_LIMITS_PER_WORKSPACE + 1, 402u16),
    ] {
        let workspace_id = amp.active_workspace(region, "bounded").await;
        let entries: Vec<serde_json::Value> = (0..count)
            .map(|index| {
                serde_json::json!({
                    "labelSet": { "env": format!("env_{index}") },
                    "limits": { "maxSeries": 1 },
                })
            })
            .collect();
        let reply = update(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "limitsPerLabelSet": entries }),
        )
        .await;
        assert_eq!(
            reply.status, status,
            "case {name:?}: {count} label sets must answer HTTP {status}, got {:?}",
            reply.status
        );

        wait_for_due_transition();
        let described = describe(&amp, region, &workspace_id).await;
        let stored = config_of(&described).limits.len();
        if status == 202 {
            assert_eq!(
                stored, count,
                "case {name:?}: the accepted update must store every label set"
            );
        } else {
            assert_eq!(
                reply.error_code(),
                "ServiceQuotaExceededException",
                "case {name:?}: must answer ServiceQuotaExceededException, got {reply:?}"
            );
            assert_eq!(
                stored, 0,
                "case {name:?}: the refused update must store no label set"
            );
        }
    }
}

/// Neither operation invents a workspace: an id no workspace holds is a 404, and the
/// refusal names the resource it looked for.
#[tokio::test]
async fn an_absent_workspace_is_not_found() {
    let amp = Amp::new();
    let region = "us-east-1";
    // One id that was never minted, and one whose workspace was deleted.
    let deleted = amp.active_workspace(region, "to-delete").await;
    let removed = amp
        .call("DELETE", &format!("/workspaces/{deleted}"), region, "")
        .await;
    assert_eq!(
        removed.status, 202,
        "delete must answer 202, got {removed:?}"
    );
    wait_for_due_transition();

    for workspace_id in ["ws-00000000-0000-0000-0000-000000000000", deleted.as_str()] {
        let described = describe(&amp, region, workspace_id).await;
        assert_eq!(
            described.status, 404,
            "case {workspace_id:?}: describe must answer 404, got {described:?}"
        );
        assert_eq!(
            described.error_code(),
            "ResourceNotFoundException",
            "case {workspace_id:?}: describe must answer ResourceNotFoundException, got {described:?}"
        );
        assert_eq!(
            text(&described.body["resourceId"]),
            workspace_id,
            "case {workspace_id:?}: the refusal must name the workspace it looked for"
        );

        let updated = update(
            &amp,
            region,
            workspace_id,
            &serde_json::json!({ "retentionPeriodInDays": 7 }),
        )
        .await;
        assert_eq!(
            updated.status, 404,
            "case {workspace_id:?}: update must answer 404, got {updated:?}"
        );
        assert_eq!(
            updated.error_code(),
            "ResourceNotFoundException",
            "case {workspace_id:?}: update must answer ResourceNotFoundException, got {updated:?}"
        );
    }
}

/// `WorkspaceConfigurationStatusCode`: "Update is disallowed until workspace
/// configuration is ACTIVE and workspace status is ACTIVE." One row per way that does
/// not hold.
#[tokio::test]
async fn update_is_refused_until_the_workspace_and_its_configuration_are_active() {
    let amp = Amp::new();
    let region = "us-west-1";
    let request = serde_json::json!({ "retentionPeriodInDays": 20 });

    // The workspace is still CREATING: the clock has not made it ACTIVE yet.
    let creating = amp
        .call("POST", "/workspaces", region, &serde_json::json!({}).to_string())
        .await;
    assert_eq!(creating.status, 202, "create must answer 202, got {creating:?}");
    let creating_id = text(&creating.body["workspaceId"]);
    let refused = update(&amp, region, &creating_id, &request).await;
    assert_eq!(
        refused.status, 409,
        "case \"workspace creating\": update must answer 409, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ConflictException",
        "case \"workspace creating\": update must answer ConflictException, got {refused:?}"
    );

    // The previous update has not settled: the configuration is still UPDATING.
    let workspace_id = amp.active_workspace(region, "conflicted").await;
    let accepted = update(&amp, region, &workspace_id, &request).await;
    assert_eq!(
        accepted.status, 202,
        "case \"configuration updating\": the first update must answer 202, got {accepted:?}"
    );
    let second = update(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({ "retentionPeriodInDays": 21 }),
    )
    .await;
    assert_eq!(
        second.status, 409,
        "case \"configuration updating\": a second update must answer 409, got {second:?}"
    );
    assert_eq!(
        second.error_code(),
        "ConflictException",
        "case \"configuration updating\": a second update must answer ConflictException, got {second:?}"
    );

    // Once the clock settles the configuration the same request is accepted, and the
    // refused one wrote nothing.
    wait_for_due_transition();
    let settled = describe(&amp, region, &workspace_id).await;
    assert_eq!(
        config_of(&settled).retention_period_in_days,
        Some(20),
        "case \"configuration updating\": the refused update must have written nothing, got {settled:?}"
    );
    let retried = update(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({ "retentionPeriodInDays": 21 }),
    )
    .await;
    assert_eq!(
        retried.status, 202,
        "case \"configuration active\": the retry must answer 202, got {retried:?}"
    );
}

/// `clientToken` carries `@idempotencyToken`: replaying the same token answers with the
/// update it already started instead of starting a second one.
#[tokio::test]
async fn replaying_a_client_token_does_not_start_a_second_update() {
    let amp = Amp::new();
    let region = "ca-central-1";
    let workspace_id = amp.active_workspace(region, "idempotent").await;
    let request = serde_json::json!({
        "clientToken": "token-1",
        "retentionPeriodInDays": 45,
    });

    let first = update(&amp, region, &workspace_id, &request).await;
    assert_eq!(first.status, 202, "the first update must answer 202, got {first:?}");

    // While the update is pending, the replay is NOT a conflict.
    let replay = update(&amp, region, &workspace_id, &request).await;
    assert_eq!(
        replay.status, 202,
        "case \"replay while updating\": must answer 202, got {replay:?}"
    );
    assert_eq!(
        text(&replay.body["status"]["statusCode"]),
        CONFIG_STATUS_UPDATING,
        "case \"replay while updating\": must report the update already in flight, got {replay:?}"
    );

    // A DIFFERENT token on a pending update is the conflict the status documents.
    let other = update(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({ "clientToken": "token-2", "retentionPeriodInDays": 46 }),
    )
    .await;
    assert_eq!(
        other.status, 409,
        "case \"another token while updating\": must answer 409, got {other:?}"
    );

    // Once the update has settled the token no longer stands for one in flight: the
    // same call is a NEW request, accepted like any other, and a call carrying members
    // of its own is applied rather than dropped behind the old status.
    wait_for_due_transition();
    let settled_replay = update(&amp, region, &workspace_id, &request).await;
    assert_eq!(
        settled_replay.status, 202,
        "case \"replay once settled\": must answer 202, got {settled_replay:?}"
    );
    assert_eq!(
        text(&settled_replay.body["status"]["statusCode"]),
        CONFIG_STATUS_UPDATING,
        "case \"replay once settled\": a new request starts a new update, got {settled_replay:?}"
    );
    wait_for_due_transition();
    let described = describe(&amp, region, &workspace_id).await;
    assert_eq!(
        config_of(&described),
        Config {
            status: CONFIG_STATUS_ACTIVE.to_string(),
            retention_period_in_days: Some(45),
            limits: Vec::new(),
            out_of_order_time_window_in_seconds: default_out_of_order_window(),
            rule_query_offset_in_seconds: default_rule_query_offset(),
        },
        "case \"replay once settled\": applying the same members again changes nothing"
    );

    // And a settled token reused for DIFFERENT members writes those members instead of
    // answering with the update it once named.
    let reused = update(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({ "clientToken": "token-1", "retentionPeriodInDays": 47 }),
    )
    .await;
    assert_eq!(
        reused.status, 202,
        "case \"settled token with new members\": must answer 202, got {reused:?}"
    );
    wait_for_due_transition();
    let rewritten = describe(&amp, region, &workspace_id).await;
    assert_eq!(
        config_of(&rewritten).retention_period_in_days,
        Some(47),
        "case \"settled token with new members\": the new members must be stored, got {rewritten:?}"
    );
}

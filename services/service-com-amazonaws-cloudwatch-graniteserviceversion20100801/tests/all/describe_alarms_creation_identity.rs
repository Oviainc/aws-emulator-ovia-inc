// SPDX-License-Identifier: CC0-1.0

//! The creation identity `DescribeAlarms` reports, over every path that SELECTS the alarms it
//! reports.
//!
//! One rule, asserted over a TABLE whose rows differ along the filter the read selected the
//! alarm with — no filter, an exact name list, a name prefix, the alarm type, the state value,
//! an action prefix, and each page of a paged read — rather than along one recorded request: a
//! read that reports a metric alarm in full reports the identity of the write that created it,
//! `<uuid>/<epoch-millis>`, on EVERY alarm it returns; and that identity belongs to the alarm,
//! not to the request that asked for it, so every path and every later read reports the same
//! one for the same alarm and a different one for every other alarm.
//!
//! An alarm the clock has re-evaluated since it was written is still the alarm that write
//! created, so the identity a read reports is also unchanged by the evaluations a read settles.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{
    build_router, creation_identity_millis, described_alarm, merged, metric_alarm_request, ok,
    reported_creation_id, wait_for_evaluation, AlarmKind as Kind,
};

/// The account the harness signs its requests for, as an action ARN names it.
const ACTION_PREFIX: &str = "arn:aws:sns:us-east-1:000000000000:";

/// The alarms every row below selects from: three metric alarms, each identifying the series it
/// watches its own way and each with its own alarm action, so one name prefix, one state and one
/// action prefix each select a different subset of them.
///
/// Returned in the order a read reports them, which is by alarm name
/// (`DescribeAlarmsOutput$MetricAlarms`).
fn alarms() -> Vec<(&'static str, Value)> {
    vec![
        ("ident-average", json!({})),
        (
            "ident-percentile",
            json!({"Statistic": null, "ExtendedStatistic": "p90"}),
        ),
        ("other-sum", json!({"Statistic": "Sum"})),
    ]
}

/// Write the alarms of [`alarms`], and answer the creation identity each one reports, read back
/// through `DescribeAlarms`.
///
/// Every identity is checked for the form here, so each row below asserts only that the path it
/// uses reports the identity the alarm has.
async fn given_alarms(
    router: &smithy_server_runtime::MultiServiceRouter,
) -> BTreeMap<String, String> {
    let mut identities: BTreeMap<String, String> = BTreeMap::new();
    for (name, patch) in alarms() {
        let request = merged(
            metric_alarm_request(name),
            &merged(
                patch,
                &json!({"AlarmActions": [format!("{ACTION_PREFIX}{name}")]}),
            ),
        );
        ok(router, "PutMetricAlarm", &request).await;

        let alarm = described_alarm(router, Kind::Metric, name)
            .await
            .unwrap_or(Value::Null);
        let reported = reported_creation_id(&alarm).unwrap_or_default().to_string();
        assert!(
            creation_identity_millis(&reported).is_some(),
            "case {name:?}: expected a CreationId of the form <uuid>/<epoch-millis>, got {alarm}"
        );
        assert!(
            identities.values().all(|other| other != &reported),
            "case {name:?}: every alarm carries its own creation identity, got the repeated \
             {reported:?} among {identities:?}"
        );
        identities.insert(name.to_string(), reported);
    }
    identities
}

/// The alarm name and reported creation identity of every metric alarm in `described`.
fn reported_identities(described: &Value) -> Vec<(String, Option<String>)> {
    described
        .get("MetricAlarms")
        .and_then(Value::as_array)
        .map(|alarms| {
            alarms
                .iter()
                .map(|alarm| {
                    (
                        alarm
                            .get("AlarmName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        reported_creation_id(alarm).map(str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The pairs a row expects: each selected alarm, with the identity [`given_alarms`] recorded for
/// it.
fn expected_identities(
    identities: &BTreeMap<String, String>,
    selected: &[&str],
) -> Vec<(String, Option<String>)> {
    selected
        .iter()
        .map(|name| ((*name).to_string(), identities.get(*name).cloned()))
        .collect()
}

#[tokio::test]
async fn every_selection_path_reports_the_creation_identity_of_the_alarms_it_selects() {
    let router = build_router();
    let identities = given_alarms(&router).await;

    // One alarm is forced into a state no other alarm is in, so the row that selects on the
    // state value selects exactly that alarm whatever the clock has evaluated meanwhile.
    ok(
        &router,
        "SetAlarmState",
        &json!({
            "AlarmName": "ident-percentile",
            "StateValue": "ALARM",
            "StateReason": "selected by state",
        }),
    )
    .await;

    // Rows differ in the filter the read selects with, and in how many of the alarms that filter
    // selects — "You can filter the results by specifying a prefix for the alarm name, the alarm
    // state, or a prefix for any action" (`DescribeAlarms`).
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter at all",
            json!({}),
            vec!["ident-average", "ident-percentile", "other-sum"],
        ),
        (
            "an exact name list",
            json!({"AlarmNames": ["other-sum", "ident-percentile"]}),
            vec!["ident-percentile", "other-sum"],
        ),
        (
            "a name prefix",
            json!({"AlarmNamePrefix": "ident-"}),
            vec!["ident-average", "ident-percentile"],
        ),
        (
            "the metric alarm type, named",
            json!({"AlarmTypes": ["MetricAlarm"]}),
            vec!["ident-average", "ident-percentile", "other-sum"],
        ),
        (
            "every alarm type there is",
            json!({"AlarmTypes": ["MetricAlarm", "CompositeAlarm", "LogAlarm"]}),
            vec!["ident-average", "ident-percentile", "other-sum"],
        ),
        (
            "a state value",
            json!({"StateValue": "ALARM"}),
            vec!["ident-percentile"],
        ),
        (
            "an action prefix",
            json!({"ActionPrefix": format!("{ACTION_PREFIX}other-")}),
            vec!["other-sum"],
        ),
        (
            "the first page of a paged read",
            json!({"MaxRecords": 1}),
            vec!["ident-average"],
        ),
        (
            "a page size larger than the store holds",
            json!({"MaxRecords": 100}),
            vec!["ident-average", "ident-percentile", "other-sum"],
        ),
    ];

    for (case, filter, selected) in cases {
        let described = ok(&router, "DescribeAlarms", &filter).await;
        assert_eq!(
            reported_identities(&described),
            expected_identities(&identities, &selected),
            "case {case:?}: every alarm this path selects must report the creation identity of \
             the write that created it ({identities:?}), got {described}"
        );
    }
}

#[tokio::test]
async fn a_paged_read_reports_a_creation_identity_on_every_page() {
    let router = build_router();
    let identities = given_alarms(&router).await;

    // "The maximum number of alarm descriptions to retrieve" (`DescribeAlarmsInput$MaxRecords`):
    // rows differ in the page size, so the same alarms are split over a different number of
    // pages — and the identity each page reports is the alarm's, not the page's.
    for page_size in [1, 2, 3] {
        let mut walked: BTreeMap<String, String> = BTreeMap::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"MaxRecords": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = ok(&router, "DescribeAlarms", &request).await;
            pages += 1;
            let reported = reported_identities(&page);
            assert!(
                reported.len() <= usize::try_from(page_size).expect("a page size fits a usize"),
                "case page size {page_size}: a page holds at most the alarms asked for, got \
                 {page}"
            );
            for (name, identity) in reported {
                assert_eq!(
                    identity.as_deref(),
                    identities.get(&name).map(String::as_str),
                    "case page size {page_size}, page {pages}: expected the creation identity \
                     {:?} for {name:?}, got {identity:?}",
                    identities.get(&name)
                );
                walked.insert(name, identity.unwrap_or_default());
            }
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert!(
                pages <= identities.len() + 1,
                "case page size {page_size}: a paged walk must terminate, got {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, identities,
            "case page size {page_size}: the walk must report every alarm with its own creation \
             identity"
        );
    }
}

#[tokio::test]
async fn the_creation_identity_outlives_the_evaluations_a_read_settles() {
    let router = build_router();
    let identities = given_alarms(&router).await;

    // Rows differ in what has happened to the alarm since it was written: the evaluation the
    // clock owed a new alarm has come due, and then a caller has forced its state. Neither is
    // the write that CREATED the alarm, so neither changes the identity a read reports.
    for stage in ["once the evaluation is due", "after a forced state"] {
        if stage == "after a forced state" {
            for (name, _) in alarms() {
                ok(
                    &router,
                    "SetAlarmState",
                    &json!({
                        "AlarmName": name,
                        "StateValue": "ALARM",
                        "StateReason": "forced after creation",
                    }),
                )
                .await;
            }
        }
        wait_for_evaluation();

        let described = ok(&router, "DescribeAlarms", &json!({})).await;
        let reported: BTreeMap<String, String> = reported_identities(&described)
            .into_iter()
            .map(|(name, identity)| (name, identity.unwrap_or_default()))
            .collect();
        assert_eq!(
            reported, identities,
            "case {stage:?}: a read reports the identity of the write that created each alarm, \
             unchanged, got {described}"
        );
    }
}

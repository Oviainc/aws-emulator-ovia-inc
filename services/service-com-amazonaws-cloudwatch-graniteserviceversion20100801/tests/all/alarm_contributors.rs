// SPDX-License-Identifier: CC0-1.0

//! What `DescribeAlarmContributors` reports: "the current alarm contributors that are in `ALARM`
//! state", read off the alarm this store holds rather than recorded beside it.
//!
//! The rule under test: an alarm's contributor is the time series the alarm evaluates, and it is
//! reported exactly while the alarm's own state is `ALARM` — so an alarm of a kind that watches
//! no time series of its own has no contributor whatever state it is in, and an alarm that is not
//! in `ALARM` has none either.
//!
//! Every table row is a distinct input along what the rule generalises over (which kind of alarm,
//! how many dimensions identify its series, which state the alarm is in, and how a requested name
//! misses), so one recorded call is one row among many.

use crate::harness::{
    build_router, call, described_alarm, merged, metric_alarm_request, error_code, error_message,
    ok, AlarmKind,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The contributors `DescribeAlarmContributors` reports for `name`, with the whole answer beside
/// them so an assertion can name what it saw.
async fn contributors(router: &MultiServiceRouter, name: &str) -> (Vec<Value>, Value) {
    let answered = ok(
        router,
        "DescribeAlarmContributors",
        &json!({"AlarmName": name}),
    )
    .await;
    let reported = answered
        .get("AlarmContributors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (reported, answered)
}

/// A string member of a body.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Force `name` into `state`, the one seam that puts an alarm of ANY kind into a state a reader
/// can then be asked about without waiting for a metric to be published and graded.
async fn force_state(router: &MultiServiceRouter, name: &str, state: &str) {
    ok(
        router,
        "SetAlarmState",
        &json!({"AlarmName": name, "StateValue": state, "StateReason": format!("{state} for {name}")}),
    )
    .await;
}

#[tokio::test]
async fn an_alarm_in_alarm_state_reports_the_series_that_contributes_to_it() {
    let router = build_router();

    // (case, the alarm's kind, its name, the dimensions identifying its series, how many
    //  contributors it owes while it is in ALARM).
    //
    // A composite alarm's state comes from other alarms' states and a log alarm's from a
    // scheduled query, so neither watches a time series of its own and neither owes a
    // contributor; a metric alarm watches one series whatever dimensions identify it.
    let cases: Vec<(&str, AlarmKind, &str, Value, usize)> = vec![
        (
            "a metric alarm on an undimensioned series",
            AlarmKind::Metric,
            "undimensioned-alarm",
            json!([]),
            1,
        ),
        (
            "a metric alarm on a series with one dimension",
            AlarmKind::Metric,
            "one-dimension-alarm",
            json!([{"Name": "InstanceId", "Value": "i-0123456789abcdef0"}]),
            1,
        ),
        (
            "a metric alarm on a series with three dimensions",
            AlarmKind::Metric,
            "three-dimension-alarm",
            json!([
                {"Name": "InstanceId", "Value": "i-0123456789abcdef0"},
                {"Name": "AutoScalingGroupName", "Value": "web-asg"},
                {"Name": "Stage", "Value": "prod"},
            ]),
            1,
        ),
        (
            "a composite alarm, whose state comes from other alarms",
            AlarmKind::Composite,
            "composite-alarm",
            json!([]),
            0,
        ),
        (
            "a log alarm, whose state comes from a scheduled query",
            AlarmKind::Log,
            "log-alarm",
            json!([]),
            0,
        ),
    ];

    let mut ids: Vec<String> = vec![];
    for (case, kind, name, dimensions, owed) in cases {
        match kind {
            AlarmKind::Metric => {
                let request = merged(
                    metric_alarm_request(name),
                    &json!({"Dimensions": dimensions.clone()}),
                );
                ok(&router, "PutMetricAlarm", &request).await;
            }
            _ => kind.create(&router, name, None).await,
        }

        // Before anything puts the alarm in ALARM there is no contributor in ALARM state to
        // report, whatever kind of alarm it is.
        let (reported, answered) = contributors(&router, name).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: an alarm that is not in ALARM owes no contributor, got {answered}"
        );

        // A log alarm's state is not one `SetAlarmState` can force ("Temporarily sets the state
        // of an alarm for testing purposes" refuses a name no metric or composite alarm carries),
        // so that row stays in the state its write left it in — which is the state its own row
        // asserts about.
        let forceable = !matches!(kind, AlarmKind::Log);
        if forceable {
            force_state(&router, name, "ALARM").await;
        }
        let (reported, answered) = contributors(&router, name).await;
        let owed = if forceable { owed } else { 0 };
        assert_eq!(
            reported.len(),
            owed,
            "case {case:?}: expected {owed} contributor(s), got {answered}"
        );
        assert!(
            answered.get("NextToken").is_none(),
            "case {case:?}: every contributor of the alarm is in this page, so no token is owed, \
             got {answered}"
        );

        if let Some(contributor) = reported.first() {
            let alarm = described_alarm(&router, kind, name)
                .await
                .unwrap_or_default();
            // The series is described by its own dimensions and by nothing this service made up.
            let expected_attributes: Value = Value::Object(
                dimensions
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|dimension| {
                        Some((
                            text(dimension, "Name")?,
                            Value::from(text(dimension, "Value")?),
                        ))
                    })
                    .collect(),
            );
            assert_eq!(
                contributor.get("ContributorAttributes"),
                Some(&expected_attributes),
                "case {case:?}: expected the series' own dimensions {expected_attributes}, \
                 got {contributor}"
            );
            // Why that series is in ALARM, and when it got there, is what the alarm itself
            // reports: the series IS what the alarm evaluates.
            assert_eq!(
                (
                    text(contributor, "StateReason"),
                    contributor.get("StateTransitionedTimestamp"),
                ),
                (
                    text(&alarm, "StateReason"),
                    alarm.get("StateTransitionedTimestamp"),
                ),
                "case {case:?}: expected the alarm's own reason and transition instant, \
                 got {contributor} beside {alarm}"
            );
            // The identifier fits the member's modelled 1..16 length and is stable across reads.
            let id = text(contributor, "ContributorId").unwrap_or_default();
            assert!(
                (1..=16).contains(&id.chars().count()),
                "case {case:?}: expected an id of 1 to 16 characters, got {id:?}"
            );
            let (again, _) = contributors(&router, name).await;
            assert_eq!(
                again.first().and_then(|entry| text(entry, "ContributorId")),
                Some(id.clone()),
                "case {case:?}: a contributor keeps its id across reads, got {again:?}"
            );
            ids.push(id);
        }

        // Back in OK, the alarm has no contributor in ALARM state again: the read reports the
        // state the alarm is in now rather than one it was in.
        if forceable {
            force_state(&router, name, "OK").await;
        }
        let (reported, answered) = contributors(&router, name).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: an alarm that is not in ALARM owes no contributor, got {answered}"
        );
    }

    // Two alarms' contributors are two contributors: the identifier distinguishes the series it
    // names rather than being one fixed value.
    let distinct: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "the contributors of different alarms must carry different ids, got {ids:?}"
    );
}

#[tokio::test]
async fn a_contributor_read_refuses_a_name_no_alarm_carries_and_a_token_it_did_not_issue() {
    let router = build_router();
    ok(&router, "PutMetricAlarm", &metric_alarm_request("present-alarm")).await;
    ok(&router, "PutMetricAlarm", &metric_alarm_request("deleted-alarm")).await;
    ok(
        &router,
        "DeleteAlarms",
        &json!({"AlarmNames": ["deleted-alarm"]}),
    )
    .await;
    force_state(&router, "present-alarm", "ALARM").await;

    // Rows differ along HOW the requested name misses the alarms the store holds.
    for (case, name) in [
        ("a name nothing was created under", "absent-alarm"),
        ("an alarm created and then deleted", "deleted-alarm"),
        ("a stored name in another case", "PRESENT-ALARM"),
        ("a stored name with a suffix", "present-alarm-2"),
        ("a stored name with a prefix", "my-present-alarm"),
    ] {
        let (status, body) = call(
            &router,
            "DescribeAlarmContributors",
            &json!({"AlarmName": name}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} with {body}"
        );
        let got = error_message(&body);
        assert_eq!(
            got,
            format!("No alarm for name {name} was found"),
            "case {case:?}: expected the service's own sentence, got {got:?} in {body}"
        );
    }

    // No call of this operation issues a token, so every token a request can carry is one this
    // service did not issue. Rows differ along the form of the token, and along whether the alarm
    // the request names exists at all — the token is judged on either path.
    for (case, name, token) in [
        ("text that is no token", "present-alarm", "not-a-token"),
        ("a token-looking hex string", "present-alarm", "6162636465"),
        ("an empty token", "present-alarm", ""),
        ("a token with a name no alarm carries", "absent-alarm", "not-a-token"),
    ] {
        let (status, body) = call(
            &router,
            "DescribeAlarmContributors",
            &json!({"AlarmName": name, "NextToken": token}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidNextToken"),
            "case {case:?}: expected HTTP 400 InvalidNextToken, got {status} with {body}"
        );
        let got = error_message(&body);
        assert_eq!(
            got, "Invalid NextToken",
            "case {case:?}: expected the service's own sentence, got {got:?} in {body}"
        );
    }

    // The refusals changed nothing: the alarm the store holds still reports its contributor.
    let (reported, answered) = contributors(&router, "present-alarm").await;
    assert_eq!(
        reported.len(),
        1,
        "case \"after the refusals\": the alarm in ALARM must still report its contributor, \
         got {answered}"
    );
}

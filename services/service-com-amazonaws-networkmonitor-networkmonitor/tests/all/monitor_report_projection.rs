// SPDX-License-Identifier: CC0-1.0

//! Every monitor read path reports ONE stored record the same way.
//!
//! `CreateMonitor`, `UpdateMonitor`, the `monitors[]` summary `ListMonitors`
//! vends and `GetMonitor` each answer from the stored monitor record. The rule:
//! the members an operation reports for a monitor, and their values, come from
//! that record alone, so every one of those answers reports a given stored
//! monitor identically — `GetMonitor` over the members its superset output
//! shares with the others, and the only member that may differ between two
//! answers is one the record itself changed in between (`state`, which the
//! clock moves from `PENDING` to `ACTIVE`, and `aggregationPeriod`, which an
//! update replaces).
//!
//! The table spans the class the rule covers: how many tags the monitor carries
//! (none, one, three), each documented `aggregationPeriod` and the documented
//! default left unnamed, and whether the monitor holds a probe at all — which
//! is what decides the `state` the record settles into (`INACTIVE` with
//! nothing to aggregate, `ACTIVE` once the clock has made the activation of a
//! monitor with a probe due). Each row is driven through all four operations,
//! both with an update that changes nothing and with one that replaces the
//! stored period.

use std::collections::BTreeSet;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, get_monitor, list_monitors, source_arn_of, string_at, PAST_ACTIVATION,
};

/// The `aggregationPeriod` values the model documents for this service
/// (`smithy_query(query="doc",shape="CreateMonitorInput",member="aggregationPeriod")`
/// -> "Valid values are either 30 or 60. 60 is the default if no period is
/// chosen."), so a row's stored period and the period an update replaces it
/// with are both drawn from this catalog.
const DOCUMENTED_PERIODS: &[i64] = &[30, 60];

/// The period a monitor is stored with when its create names none, from the
/// same documented sentence as [`DOCUMENTED_PERIODS`].
const DEFAULT_PERIOD: i64 = 60;

/// The members the three structurally identical monitor response shapes model
/// (`smithy_query(query="shape",
/// shape="CreateMonitorOutput,UpdateMonitorOutput,MonitorSummary")`), which are
/// also the members `GetMonitorOutput` shares with them.
const SHARED_MEMBERS: &[&str] = &[
    "monitorArn",
    "monitorName",
    "state",
    "aggregationPeriod",
    "tags",
];

/// The members `GetMonitorOutput` models on top of [`SHARED_MEMBERS`]
/// (`smithy_query(query="shape",shape="GetMonitorOutput")`).
const GET_MONITOR_EXTRA_MEMBERS: &[&str] = &["probes", "createdAt", "modifiedAt"];

/// One row of the table: a monitor to create, and what the record it stores
/// reports back.
struct Row {
    /// What this row varies, named in every assertion of it.
    case: &'static str,
    /// The `aggregationPeriod` the create names, or `None` to leave it unnamed
    /// and take the documented default.
    period: Option<i64>,
    /// The `tags` the create assigns the monitor, as JSON body text (`{}` for a
    /// monitor carrying none).
    tags: &'static str,
    /// Whether the create gives the monitor a probe, which is what decides the
    /// state it settles into.
    with_probe: bool,
    /// The `state` the stored record reports once the clock has made every
    /// transition it owes due.
    settled_state: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "the documented default period, no tags, no probe",
        period: None,
        tags: "{}",
        with_probe: false,
        settled_state: "INACTIVE",
    },
    Row {
        case: "period 30, one tag, no probe",
        period: Some(30),
        tags: r#"{"team":"net"}"#,
        with_probe: false,
        settled_state: "INACTIVE",
    },
    Row {
        case: "period 60, three tags, one probe",
        period: Some(60),
        tags: r#"{"a":"1","b":"2","c":"3"}"#,
        with_probe: true,
        settled_state: "ACTIVE",
    },
    Row {
        case: "period 30, one tag, one probe",
        period: Some(30),
        tags: r#"{"team":"net"}"#,
        with_probe: true,
        settled_state: "ACTIVE",
    },
];

/// The period a row's stored monitor is updated to: the documented value that
/// is NOT the one it was stored with, so the update really replaces it.
fn other_period(stored: i64) -> i64 {
    *DOCUMENTED_PERIODS
        .iter()
        .find(|period| **period != stored)
        .expect("the documented catalog holds more than one period")
}

async fn update_monitor(router: &MultiServiceRouter, name: &str, period: i64) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("/monitors/{name}"),
        Some(&format!(r#"{{"aggregationPeriod":{period}}}"#)),
    )
    .await
}

/// The members a response reports, so two answers can be compared on the SET of
/// members and not only on the values of the ones both carry.
fn members_of(body: &Value) -> BTreeSet<String> {
    body.as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

/// The value a response reports for `member`, with an absent member read as
/// `Null`.
fn member_of(body: &Value, member: &str) -> Value {
    body.get(member).cloned().unwrap_or(Value::Null)
}

/// The `monitors[]` summary a `ListMonitors` page reports for `name`.
fn listed_monitor(page: &Value, name: &str) -> Value {
    page.get("monitors")
        .and_then(Value::as_array)
        .and_then(|monitors| {
            monitors
                .iter()
                .find(|monitor| string_at(monitor, "monitorName") == name)
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// Assert that every answer in `answers` reports the [`SHARED_MEMBERS`] exactly
/// as `expected` does.
fn assert_shared_members_agree(case: &str, expected: &Value, answers: &[(&str, &Value)]) {
    for member in SHARED_MEMBERS {
        let want = member_of(expected, member);
        for (operation, body) in answers {
            let actual = member_of(body, member);
            assert_eq!(
                actual, want,
                "case {case:?}: {operation} must report the {member} the record holds, \
                 expected {want:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_monitor_read_path_reports_the_same_stored_record() {
    let (router, created) = monitors_of_every_row().await;
    // Every monitor was created before this wait, so by now the clock owes each
    // of them whatever transition it had due and every read below sees a
    // settled record that no longer moves under it.
    std::thread::sleep(PAST_ACTIVATION);

    // An update that names the period the record already holds changes nothing,
    // so its answer and the reads around it are four reports of ONE unchanged
    // record.
    let mut unchanged: Vec<(Value, Value)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let name = monitor_name(index);
        let stored_period = row.period.unwrap_or(DEFAULT_PERIOD);
        let (status, updated) = update_monitor(&router, &name, stored_period).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateMonitor expected 200, got {status} {updated:?}",
            row.case
        );
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetMonitor expected 200, got {status} {got:?}",
            row.case
        );
        unchanged.push((updated, got));
    }
    // One page holds the whole table (the model bounds `maxResults` at 25), so
    // every row's summary comes from a single listing.
    let (status, page) = list_monitors(&router, "").await;
    assert_eq!(
        status, 200,
        "ListMonitors expected 200, got {status} {page:?}"
    );

    for (index, row) in ROWS.iter().enumerate() {
        let name = monitor_name(index);
        let (updated, got) = &unchanged[index];
        let listed = listed_monitor(&page, &name);
        let create = &created[index];

        // `UpdateMonitor` and the `ListMonitors` summary model the same members
        // as `CreateMonitor`: a member one path carries and another drops would
        // be a monitor reported differently depending on which operation was
        // asked.
        let expected_members: BTreeSet<String> =
            SHARED_MEMBERS.iter().map(|m| (*m).to_string()).collect();
        for (operation, body) in [
            ("CreateMonitor", create),
            ("UpdateMonitor", updated),
            ("ListMonitors monitors[]", &listed),
        ] {
            let actual = members_of(body);
            assert_eq!(
                actual, expected_members,
                "case {:?}: {operation} must report the modelled monitor members, \
                 expected {expected_members:?}, got {actual:?}",
                row.case
            );
        }
        // `GetMonitor` reports that same set plus the members its superset
        // output adds, and nothing else.
        let expected_get: BTreeSet<String> = SHARED_MEMBERS
            .iter()
            .chain(GET_MONITOR_EXTRA_MEMBERS.iter())
            .map(|m| (*m).to_string())
            .collect();
        let actual_get = members_of(got);
        assert_eq!(
            actual_get, expected_get,
            "case {:?}: GetMonitor must report the shared members and its own, \
             expected {expected_get:?}, got {actual_get:?}",
            row.case
        );

        // The values: one stored record, reported identically by all four.
        assert_shared_members_agree(
            row.case,
            got,
            &[
                ("UpdateMonitor", updated),
                ("ListMonitors monitors[]", &listed),
            ],
        );
        // `state` is the one member the RECORD moved between the create and
        // these reads, so the create is compared on every other shared member.
        for member in SHARED_MEMBERS.iter().filter(|member| **member != "state") {
            let want = member_of(got, member);
            let actual = member_of(create, member);
            assert_eq!(
                actual, want,
                "case {:?}: CreateMonitor must report the {member} the record holds, \
                 expected {want:?}, got {actual:?}",
                row.case
            );
        }

        // The values themselves are the stored record's, so the shared
        // projection is pinned to the request that created the monitor and not
        // merely to itself.
        let reported_arn = string_at(got, "monitorArn");
        assert!(
            reported_arn.ends_with(&format!("monitor/{name}")),
            "case {:?}: monitorArn must name the monitor's own name, got {reported_arn:?}",
            row.case
        );
        assert_eq!(
            string_at(got, "monitorName"),
            name,
            "case {:?}: monitorName must be the one stored, got {:?}",
            row.case,
            string_at(got, "monitorName")
        );
        assert_eq!(
            string_at(got, "state"),
            row.settled_state,
            "case {:?}: state must be the one the record settled into, got {:?}",
            row.case,
            string_at(got, "state")
        );
        let expected_period = Value::from(row.period.unwrap_or(DEFAULT_PERIOD));
        assert_eq!(
            member_of(got, "aggregationPeriod"),
            expected_period,
            "case {:?}: aggregationPeriod must be the one stored, expected \
             {expected_period:?}, got {:?}",
            row.case,
            member_of(got, "aggregationPeriod")
        );
        let expected_tags: Value =
            serde_json::from_str(row.tags).expect("a row's tags are well-formed JSON");
        assert_eq!(
            member_of(got, "tags"),
            expected_tags,
            "case {:?}: tags must be the pairs stored, expected {expected_tags:?}, got {:?}",
            row.case,
            member_of(got, "tags")
        );
    }

    // A write through one path is read back through the others: an update that
    // REPLACES the stored period is reported by the update itself, by
    // `GetMonitor` and by the listing's summary alike.
    let mut replaced: Vec<Value> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let name = monitor_name(index);
        let requested = other_period(row.period.unwrap_or(DEFAULT_PERIOD));
        let (status, updated) = update_monitor(&router, &name, requested).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateMonitor to {requested} expected 200, got {status} {updated:?}",
            row.case
        );
        let reported = member_of(&updated, "aggregationPeriod");
        assert_eq!(
            reported,
            Value::from(requested),
            "case {:?}: UpdateMonitor must report the period it stored, expected \
             {requested}, got {reported:?}",
            row.case
        );
        replaced.push(updated);
    }
    let (status, page) = list_monitors(&router, "").await;
    assert_eq!(
        status, 200,
        "ListMonitors after the updates expected 200, got {status} {page:?}"
    );
    for (index, row) in ROWS.iter().enumerate() {
        let name = monitor_name(index);
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetMonitor after the update expected 200, got {status} {got:?}",
            row.case
        );
        let listed = listed_monitor(&page, &name);
        assert_shared_members_agree(
            row.case,
            &replaced[index],
            &[
                ("GetMonitor", &got),
                ("ListMonitors monitors[]", &listed),
            ],
        );
    }
}

/// The name of the monitor one row stores.
fn monitor_name(index: usize) -> String {
    format!("monitor-projection-{index}")
}

/// Create one monitor per row and answer the router beside what each
/// `CreateMonitor` reported — built once, so the whole table shares a single
/// wait for the transitions the clock owes.
async fn monitors_of_every_row() -> (MultiServiceRouter, Vec<Value>) {
    let router = super::monitor_and_probe::router();
    let mut creates = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let period = row
            .period
            .map(|period| format!(r#","aggregationPeriod":{period}"#))
            .unwrap_or_default();
        // Each row's probe sits in a subnet of its own, so no row is refused
        // for the company its probes keep.
        let probes = if row.with_probe {
            format!(
                r#","probes":[{{"sourceArn":"{}","destination":"10.0.0.1","protocol":"ICMP"}}]"#,
                source_arn_of(index)
            )
        } else {
            String::new()
        };
        let body = format!(
            r#"{{"monitorName":"{}"{period},"tags":{}{probes}}}"#,
            monitor_name(index),
            row.tags
        );
        let (status, created) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateMonitor expected 200, got {status} {created:?}",
            row.case
        );
        creates.push(created);
    }
    (router, creates)
}

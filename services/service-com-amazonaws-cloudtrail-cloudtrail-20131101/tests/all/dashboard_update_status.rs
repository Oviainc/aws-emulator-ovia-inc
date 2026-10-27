// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `DashboardStatus` a write leaves a CloudTrail Lake
//! dashboard in.
//!
//! Every case goes over the wire through the shared harness
//! (`tests/all/emulator.rs`): an `awsJson1_1` `POST /` with this service's published
//! `X-Amz-Target` prefix, dispatched through the same `MultiServiceRouter` the
//! production router serves, so what is asserted here is what a client sees.
//!
//! The rules under test:
//!
//! * the status a dashboard reports is the one its last write left it in: a successful
//!   `UpdateDashboard` leaves it `UPDATED`, reported from the moment that call
//!   returns, whatever status the create path had settled;
//! * a write that is refused changes nothing about the dashboard, its status included;
//! * reading a dashboard never moves its status, and the create path's own clock never
//!   moves a dashboard an update has since written.
//!
//! `UpdateDashboardResponse` models no `Status` member
//! (`smithy_query(query="shape",shape="UpdateDashboardResponse")`), so the status a
//! write leaves is read back through `GetDashboard`, the one operation that reports it
//! (`DashboardDetail`, what `ListDashboards` pages, carries only `DashboardArn` and
//! `Type`).

use serde_json::{json, Value};
use std::time::Duration;

// An async test must not block its runtime worker, so the waits below are the
// runtime's own sleep.
use tokio::time::sleep;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

use super::emulator::{Emulator, Reply, HOME_REGION};

/// A margin on top of [`state::DASHBOARD_SETTLE_AFTER`], so a case that waits for the
/// create path's clock does not race the instant the handler reads.
fn past_settle() -> Duration {
    state::DASHBOARD_SETTLE_AFTER + Duration::from_millis(250)
}

/// One widget, so a row can differ in widget COUNT and CONTENT without repeating the
/// shape. `QueryStatement` and `ViewProperties` are `RequestWidget`'s required
/// members.
fn widget(index: usize) -> Value {
    json!({
        "QueryStatement": format!("SELECT eventName FROM EDS LIMIT {index}"),
        "ViewProperties": {"type": "bar", "colors": format!("{index}")},
    })
}

/// A refresh schedule as a request states it.
fn schedule(unit: &str, value: i32, status: &str) -> Value {
    json!({"Frequency": {"Unit": unit, "Value": value}, "Status": status})
}

/// The ARN a dashboard called `name` in `region` carries — the second of the two forms
/// `DashboardId` takes ("The name or ARN of the dashboard").
fn arn(region: &str, name: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:000000000000:dashboard/{name}")
}

/// The baseline every row starts from: one widget, an enabled 6-hourly schedule,
/// termination protection on.
///
/// These are free functions rather than inherent methods on the harness because an
/// inherent method may be defined only once per type across this test target, and the
/// neighbouring dashboard suites already name theirs.
async fn create_dashboard(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "CreateDashboard",
            &json!({
                "Name": name,
                "Widgets": [widget(0)],
                "RefreshSchedule": schedule(state::REFRESH_FREQUENCY_UNIT_HOURS, 6, "ENABLED"),
                "TerminationProtectionEnabled": true,
            }),
        )
        .await
}

/// Update the dashboard `dashboard_id` with `patch` merged into the request.
async fn update(emulator: &Emulator, dashboard_id: &str, patch: &Value) -> Reply {
    let mut request = json!({"DashboardId": dashboard_id});
    if let Some(members) = patch.as_object() {
        for (key, value) in members {
            request[key] = value.clone();
        }
    }
    emulator.call("UpdateDashboard", &request).await
}

/// Read the dashboard back.
async fn get(emulator: &Emulator, dashboard_id: &str) -> Reply {
    emulator
        .call("GetDashboard", &json!({"DashboardId": dashboard_id}))
        .await
}

/// The `Status` a successful `GetDashboard` reports.
fn status_of(reply: &Reply, case: &str) -> String {
    reply.expect_ok(case)["Status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: a successful `UpdateDashboard` leaves the dashboard reporting `UPDATED`, from
/// the moment that call returns and for as long as nothing else writes it.
///
/// The rows differ in WHICH members the update carries and in which of the two
/// documented identifier forms addresses the dashboard — what the rule generalises
/// over — including the request that carries no optional member at all: the status
/// follows the successful update, not any particular member of it.
#[tokio::test]
async fn an_update_leaves_the_dashboard_reporting_updated() {
    let emulator = Emulator::new();
    // (case, patch, address the dashboard by its ARN rather than its name)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "widgets replaced",
            json!({"Widgets": [widget(1), widget(2)]}),
            false,
        ),
        ("an empty widget list", json!({"Widgets": []}), false),
        (
            "a refresh schedule",
            json!({"RefreshSchedule": schedule(state::REFRESH_FREQUENCY_UNIT_DAYS, 1, "DISABLED")}),
            true,
        ),
        (
            "termination protection",
            json!({"TerminationProtectionEnabled": false}),
            true,
        ),
        ("no optional member at all", json!({}), false),
        (
            "every member at once",
            json!({
                "Widgets": [widget(3)],
                "RefreshSchedule": schedule(state::REFRESH_FREQUENCY_UNIT_HOURS, 12, "ENABLED"),
                "TerminationProtectionEnabled": false,
            }),
            true,
        ),
    ];

    for (index, (case, patch, by_arn)) in rows.into_iter().enumerate() {
        let name = format!("status-{index}");
        create_dashboard(&emulator, &name).await.expect_ok(case);
        let identifier = if by_arn {
            arn(HOME_REGION, &name)
        } else {
            name.clone()
        };

        // The create settles first, so what the update writes over is a settled
        // dashboard: the status it then reports is the update's, not the create's.
        sleep(past_settle()).await;
        let created = get(&emulator, &identifier).await;
        assert_eq!(
            status_of(&created, case),
            state::DASHBOARD_STATUS_CREATED,
            "case {case:?}: a created dashboard settles into CREATED, got {created:?}"
        );

        update(&emulator, &identifier, &patch).await.expect_ok(case);

        let updated = get(&emulator, &identifier).await;
        let updated = updated.expect_ok(case).clone();
        assert_eq!(
            updated["Status"],
            json!(state::DASHBOARD_STATUS_UPDATED),
            "case {case:?}: the read after a successful update must report UPDATED, got \
             {updated:?}"
        );
        assert!(
            updated["UpdatedTimestamp"].as_f64() > updated["CreatedTimestamp"].as_f64(),
            "case {case:?}: an update must stamp UpdatedTimestamp past CreatedTimestamp, got \
             {updated:?}"
        );

        // Reading is not a write: the status stays where the update left it, however
        // long after it and however often it is read.
        for attempt in 0..2 {
            let again = get(&emulator, &identifier).await;
            assert_eq!(
                again.expect_ok(case),
                &updated,
                "case {case:?} (read {attempt}): a read must report the same dashboard, got \
                 {again:?}"
            );
        }
        sleep(past_settle()).await;
        let later = get(&emulator, &identifier).await;
        assert_eq!(
            later.expect_ok(case),
            &updated,
            "case {case:?}: no clock may move a dashboard its update already wrote, got \
             {later:?}"
        );
    }
}

/// RULE: the status a dashboard reports is the one its LAST write left it in — so an
/// update of a dashboard the create path has not settled yet reports `UPDATED`, and the
/// create's own pending transition never writes `CREATED` over it afterwards.
#[tokio::test]
async fn an_update_inside_the_create_span_reports_updated() {
    let emulator = Emulator::new();
    let case = "an update before the create settles";
    create_dashboard(&emulator, "still-creating")
        .await
        .expect_ok(case);

    let fresh = get(&emulator, "still-creating").await;
    assert_eq!(
        status_of(&fresh, case),
        state::DASHBOARD_STATUS_CREATING,
        "case {case:?}: a new dashboard reads CREATING until the clock settles it, got {fresh:?}"
    );

    update(&emulator, "still-creating", &json!({"Widgets": [widget(4)]}))
        .await
        .expect_ok(case);
    let updated = get(&emulator, "still-creating").await;
    assert_eq!(
        status_of(&updated, case),
        state::DASHBOARD_STATUS_UPDATED,
        "case {case:?}: the update's own status replaces the create's, got {updated:?}"
    );

    // Past the instant the create's transition was due: it is owed to a dashboard
    // nothing has written since, and this one has been written.
    sleep(past_settle()).await;
    let settled = get(&emulator, "still-creating").await;
    let settled = settled.expect_ok(case).clone();
    assert_eq!(
        settled["Status"],
        json!(state::DASHBOARD_STATUS_UPDATED),
        "case {case:?}: the last write is what the dashboard reports, got {settled:?}"
    );
    assert_eq!(
        settled["Widgets"],
        json!([widget(4)]),
        "case {case:?}: the update must be what the dashboard now stores, got {settled:?}"
    );
}

/// RULE: a refused update changes nothing about the dashboard, including the status it
/// reports.
///
/// The rows differ in HOW the update is refused — the widget maximum, a frequency the
/// model does not publish, and a widget query CloudTrail Lake cannot run — which is
/// what the rule generalises over.
#[tokio::test]
async fn a_refused_update_leaves_the_status_alone() {
    let emulator = Emulator::new();
    let too_many: Vec<Value> = (0..=state::CUSTOM_DASHBOARD_WIDGET_LIMIT)
        .map(widget)
        .collect();
    // (case, patch, error code)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "one widget past the maximum",
            json!({"Widgets": too_many}),
            "ServiceQuotaExceededException",
        ),
        (
            "a frequency the model does not publish",
            json!({"RefreshSchedule": schedule(state::REFRESH_FREQUENCY_UNIT_HOURS, 5, "ENABLED")}),
            "ValidationException",
        ),
        (
            "a widget query that cannot run",
            json!({"Widgets": [{
                "QueryStatement": "show me every console login",
                "ViewProperties": {"type": "table"},
            }]}),
            "InvalidQueryStatementException",
        ),
    ];

    for (index, (case, patch, code)) in rows.into_iter().enumerate() {
        let name = format!("refused-{index}");
        create_dashboard(&emulator, &name).await.expect_ok(case);
        sleep(past_settle()).await;
        let before = get(&emulator, &name).await;
        let before = before.expect_ok(case).clone();

        update(&emulator, &name, &patch)
            .await
            .expect_error(case, 400, code);

        let after = get(&emulator, &name).await;
        assert_eq!(
            after.expect_ok(case),
            &before,
            "case {case:?}: a refused update must leave the dashboard exactly as it stood, got \
             {after:?}"
        );
        assert_eq!(
            status_of(&after, case),
            state::DASHBOARD_STATUS_CREATED,
            "case {case:?}: a refused update must not move the status, got {after:?}"
        );
    }

    // The CloudTrail-managed dashboard is refused the same way, and keeps the status
    // its create settled.
    let managed_schedule = schedule(
        state::REFRESH_FREQUENCY_UNIT_HOURS,
        state::HIGHLIGHTS_REFRESH_FREQUENCY_VALUE,
        state::REFRESH_SCHEDULE_ENABLED,
    );
    let case = "caller-composed widgets on the managed dashboard";
    emulator
        .call(
            "CreateDashboard",
            &json!({
                "Name": state::HIGHLIGHTS_DASHBOARD_NAME,
                "RefreshSchedule": managed_schedule,
            }),
        )
        .await
        .expect_ok(case);
    sleep(past_settle()).await;
    update(
        &emulator,
        state::HIGHLIGHTS_DASHBOARD_NAME,
        &json!({"Widgets": [widget(0)]}),
    )
    .await
    .expect_error(case, 400, "ValidationException");
    let after = get(&emulator, state::HIGHLIGHTS_DASHBOARD_NAME).await;
    assert_eq!(
        status_of(&after, case),
        state::DASHBOARD_STATUS_CREATED,
        "case {case:?}: a refused update must not move the status, got {after:?}"
    );

    // What that dashboard DOES accept leaves it updated, like any other dashboard.
    let accepted = "termination protection on the managed dashboard";
    update(
        &emulator,
        state::HIGHLIGHTS_DASHBOARD_NAME,
        &json!({"TerminationProtectionEnabled": true}),
    )
    .await
    .expect_ok(accepted);
    let managed = get(&emulator, state::HIGHLIGHTS_DASHBOARD_NAME).await;
    assert_eq!(
        status_of(&managed, accepted),
        state::DASHBOARD_STATUS_UPDATED,
        "case {accepted:?}: an accepted update leaves the managed dashboard UPDATED too, got \
         {managed:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateDashboard`.
//!
//! Every case goes over the wire through the shared harness
//! (`tests/all/emulator.rs`): an `awsJson1_1` `POST /` with this service's published
//! `X-Amz-Target` prefix, dispatched through the same `MultiServiceRouter` the
//! production router serves, so what is asserted here is what a client sees —
//! including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * an update applies exactly the optional members the request carries — a present
//!   member replaces the stored value, an absent one leaves it alone — and the update
//!   response and every later `GetDashboard` report those same stored values;
//! * `Widgets` is the whole list, not an addition to it: the model tells a caller to
//!   pass the existing widgets plus the new ones to add, and minus the ones to
//!   remove;
//! * an update stamps `UpdatedTimestamp` and never moves `CreatedTimestamp`;
//! * the dashboard is the one `DashboardId` names, in either documented form, and an
//!   identifier naming no stored dashboard is `ResourceNotFoundException`;
//! * the widget maximum and the published refresh frequencies bound an update exactly
//!   as they bound a create, and a refused update changes nothing;
//! * a widget's `QueryStatement` has to be a query CloudTrail Lake can run — one SQL
//!   `SELECT` — and one that is not is `InvalidQueryStatementException`;
//! * an update leaves the dashboard's settled `DashboardStatus` alone: the model
//!   declares `UPDATING`/`UPDATED`, but no recording shows CloudTrail putting a
//!   dashboard back into a transient status for an update, so none is invented.

use serde_json::{json, Value};
use std::time::Duration;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

// An async test must not block its runtime worker, so the waits below are the
// runtime's own sleep.
use tokio::time::sleep;

use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases that pin a dashboard to the Region it was made in.
const OTHER_REGION: &str = "us-west-2";

/// One widget, so a row can differ in widget COUNT and CONTENT without repeating the
/// shape. `QueryStatement` and `ViewProperties` are `RequestWidget`'s required
/// members, and the reported `Widget` carries those same members back.
fn widget(index: usize) -> Value {
    json!({
        "QueryStatement": format!("SELECT eventName FROM EDS LIMIT {index}"),
        "QueryParameters": ["$StartTime$"],
        "ViewProperties": {"type": "bar", "colors": format!("{index}")},
    })
}

/// A refresh schedule as it is STORED: the request's own, with `TimeOfDay` defaulted
/// (`RefreshSchedule$TimeOfDay`: "default is 00:00").
fn schedule(unit: &str, value: i32, status: &str) -> Value {
    json!({
        "Frequency": {"Unit": unit, "Value": value},
        "Status": status,
        "TimeOfDay": state::DEFAULT_REFRESH_TIME_OF_DAY,
    })
}

/// The ARN a dashboard called `name` in `region` carries.
fn arn(region: &str, name: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:000000000000:dashboard/{name}")
}

/// The dashboard calls these cases make, on the shared harness.
impl Emulator {
    /// The baseline every patch case starts from: one widget, an enabled 6-hourly
    /// schedule, termination protection on.
    async fn create_baseline(&self, name: &str) -> Reply {
        self.call(
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
    async fn update_dashboard(&self, dashboard_id: &str, patch: &Value) -> Reply {
        let mut request = json!({"DashboardId": dashboard_id});
        if let Some(members) = patch.as_object() {
            for (key, value) in members {
                request[key] = value.clone();
            }
        }
        self.call("UpdateDashboard", &request).await
    }

    /// Read a dashboard back by name or ARN. It carries a name of its own because an
    /// inherent method may be defined only once per type across this test target, and
    /// `dashboard_refresh` already defines its own `read_dashboard`.
    async fn fetch_dashboard(&self, dashboard_id: &str) -> Reply {
        self.call("GetDashboard", &json!({"DashboardId": dashboard_id}))
            .await
    }
}

/// RULE: an update applies exactly the members the request carries, and a later read
/// reports the same stored values.
///
/// The rows differ in WHICH members are present — which is what the rule generalises
/// over — including the empty widget list that removes every widget and the request
/// that carries no optional member at all.
#[tokio::test]
async fn an_update_applies_only_the_members_the_request_carries() {
    let emulator = Emulator::new();
    let baseline_widgets = json!([widget(0)]);
    let baseline_schedule = schedule(state::REFRESH_FREQUENCY_UNIT_HOURS, 6, "ENABLED");
    let replacement = json!([widget(1), widget(2)]);
    let other_schedule = schedule(state::REFRESH_FREQUENCY_UNIT_DAYS, 1, "DISABLED");

    // (case, patch, expected widgets, expected schedule, expected termination protection)
    let rows: Vec<(&str, Value, Value, Value, bool)> = vec![
        (
            "widgets only",
            json!({"Widgets": replacement.clone()}),
            replacement.clone(),
            baseline_schedule.clone(),
            true,
        ),
        (
            "refresh schedule only",
            json!({"RefreshSchedule": other_schedule.clone()}),
            baseline_widgets.clone(),
            other_schedule.clone(),
            true,
        ),
        (
            "termination protection only",
            json!({"TerminationProtectionEnabled": false}),
            baseline_widgets.clone(),
            baseline_schedule.clone(),
            false,
        ),
        (
            "an empty widget list removes every widget",
            json!({"Widgets": []}),
            json!([]),
            baseline_schedule.clone(),
            true,
        ),
        (
            "every member at once",
            json!({
                "Widgets": replacement.clone(),
                "RefreshSchedule": other_schedule.clone(),
                "TerminationProtectionEnabled": false,
            }),
            replacement.clone(),
            other_schedule.clone(),
            false,
        ),
        (
            "no optional member at all",
            json!({}),
            baseline_widgets.clone(),
            baseline_schedule.clone(),
            true,
        ),
    ];

    for (index, (case, patch, widgets, refresh_schedule, protection)) in
        rows.into_iter().enumerate()
    {
        let name = format!("patched-{index}");
        let created = emulator.create_baseline(&name).await;
        let created = created.expect_ok(case).clone();

        let updated = emulator.update_dashboard(&name, &patch).await;
        let updated = updated.expect_ok(case).clone();
        assert_eq!(
            updated["Widgets"], widgets,
            "case {case:?}: wrong stored widgets, got {updated:?}"
        );
        assert_eq!(
            updated["RefreshSchedule"], refresh_schedule,
            "case {case:?}: wrong stored refresh schedule, got {updated:?}"
        );
        assert_eq!(
            updated["TerminationProtectionEnabled"],
            json!(protection),
            "case {case:?}: wrong stored TerminationProtectionEnabled, got {updated:?}"
        );
        assert_eq!(
            (&updated["DashboardArn"], &updated["Name"], &updated["Type"]),
            (&created["DashboardArn"], &created["Name"], &created["Type"]),
            "case {case:?}: an update must not change what identifies the dashboard, got \
             {updated:?}"
        );

        // The response is the stored record, so a read reports the same values.
        let fetched = emulator.fetch_dashboard(&name).await;
        let fetched = fetched.expect_ok(case).clone();
        assert_eq!(
            (
                &fetched["Widgets"],
                &fetched["RefreshSchedule"],
                &fetched["TerminationProtectionEnabled"],
                &fetched["UpdatedTimestamp"],
            ),
            (
                &updated["Widgets"],
                &updated["RefreshSchedule"],
                &updated["TerminationProtectionEnabled"],
                &updated["UpdatedTimestamp"],
            ),
            "case {case:?}: the dashboard read back is not the one the update stored, got \
             {fetched:?}"
        );
    }
}

/// RULE: an update stamps `UpdatedTimestamp` and never moves `CreatedTimestamp`.
#[tokio::test]
async fn an_update_moves_only_the_updated_timestamp() {
    let emulator = Emulator::new();
    let case = "timestamps";
    emulator
        .create_baseline("timestamped")
        .await
        .expect_ok(case);
    // `CreateDashboardResponse` models no timestamps, so the baseline comes from the
    // reader.
    let created = emulator.fetch_dashboard("timestamped").await;
    let created = created.expect_ok(case).clone();
    let created_at = created["CreatedTimestamp"].as_f64();
    assert_eq!(
        created["UpdatedTimestamp"].as_f64(),
        created_at,
        "case {case}: a dashboard nobody updated was last updated when it was created, \
         got {created:?}"
    );

    let mut previous = created["CreatedTimestamp"]
        .as_f64()
        .expect("the dashboard reports a CreatedTimestamp");
    // Two updates in a row: each moves UpdatedTimestamp forward again.
    for round in 0..2 {
        sleep(Duration::from_millis(20)).await;
        let updated = emulator
            .update_dashboard(
                "timestamped",
                &json!({"TerminationProtectionEnabled": round % 2 == 0}),
            )
            .await;
        let updated = updated.expect_ok(case).clone();
        assert_eq!(
            updated["CreatedTimestamp"].as_f64(),
            created_at,
            "case {case} (round {round}): CreatedTimestamp must not move, got {updated:?}"
        );
        let updated_at = updated["UpdatedTimestamp"]
            .as_f64()
            .expect("the update reports an UpdatedTimestamp");
        assert!(
            updated_at > previous,
            "case {case} (round {round}): expected UpdatedTimestamp past {previous}, got \
             {updated_at}"
        );
        previous = updated_at;
    }
}

/// RULE: the dashboard an update changes is the one `DashboardId` names, in either
/// documented form, and only in the Region that ARN or that call names.
#[tokio::test]
async fn an_update_addresses_the_dashboard_by_name_or_by_arn() {
    let emulator = Emulator::new();
    let name = "addressed-dashboard";
    emulator
        .create_baseline(name)
        .await
        .expect_ok("the dashboard in the home Region");
    // The same name in another Region is a different dashboard, so an update here
    // must leave it alone.
    emulator
        .call_in(
            "CreateDashboard",
            OTHER_REGION,
            &json!({"Name": name, "Widgets": [widget(9)]}),
        )
        .await
        .expect_ok("the dashboard in the other Region");

    for (form, identifier) in [
        ("a bare name", name.to_string()),
        ("its own ARN", arn(HOME_REGION, name)),
    ] {
        let case = &format!("addressed by {form}");
        let updated = emulator
            .update_dashboard(&identifier, &json!({"Widgets": [widget(3)]}))
            .await;
        let updated = updated.expect_ok(case).clone();
        assert_eq!(
            updated["DashboardArn"],
            json!(arn(HOME_REGION, name)),
            "case {case}: both forms address the same dashboard, got {updated:?}"
        );
        let fetched = emulator.fetch_dashboard(&identifier).await;
        assert_eq!(
            fetched.expect_ok(case)["Widgets"],
            json!([widget(3)]),
            "case {case}: the update must be what the dashboard now stores, got {fetched:?}"
        );
        // Reset for the next form, so each starts from the same place.
        emulator
            .update_dashboard(&identifier, &json!({"Widgets": [widget(0)]}))
            .await
            .expect_ok(case);
    }

    let elsewhere = emulator
        .call_in(
            "GetDashboard",
            OTHER_REGION,
            &json!({"DashboardId": name}),
        )
        .await;
    assert_eq!(
        elsewhere.expect_ok("the other Region's dashboard")["Widgets"],
        json!([widget(9)]),
        "the other Region's dashboard of the same name must be untouched, got {elsewhere:?}"
    );
}

/// RULE: an identifier that names no stored dashboard is not found — whichever of the
/// two forms it is written in, and whether it could name one at all.
///
/// The rows differ in HOW they fail to resolve; `DashboardId`'s own pattern
/// (`^[a-zA-Z0-9._/\-:]+$`) admits every one of them, so each reaches the handler.
#[tokio::test]
async fn an_update_of_a_dashboard_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    emulator
        .create_baseline("present-dashboard")
        .await
        .expect_ok("the dashboard that does exist");

    let rows: Vec<(&str, String)> = vec![
        ("an unused name", "absent-dashboard".to_string()),
        ("an unused ARN", arn(HOME_REGION, "absent-dashboard")),
        (
            "an existing name in another Region's ARN",
            arn(OTHER_REGION, "present-dashboard"),
        ),
        ("a name too short to be one", "ab".to_string()),
        (
            "a name outside the DashboardName pattern",
            "present.dashboard".to_string(),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{HOME_REGION}:000000000000:dashboard/present-dashboard"),
        ),
        (
            "an ARN of another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/present-dashboard"),
        ),
        (
            "an ARN missing its account",
            format!("arn:aws:cloudtrail:{HOME_REGION}::dashboard/present-dashboard"),
        ),
    ];
    for (case, identifier) in rows {
        let refused = emulator
            .update_dashboard(&identifier, &json!({"Widgets": [widget(4)]}))
            .await;
        refused.expect_error(case, 400, "ResourceNotFoundException");
        // One sentence for the whole class, written out here rather than read from
        // the constant the handler uses, so a change to either is caught: the
        // refusal names no part of the request.
        assert_eq!(
            refused.body["Message"],
            json!("No matching dashboard found."),
            "case {case}: wrong refusal message, got {refused:?}"
        );
    }

    // None of those refusals touched the dashboard that does exist.
    let stored = emulator.fetch_dashboard("present-dashboard").await;
    assert_eq!(
        stored.expect_ok("the dashboard after the refusals")["Widgets"],
        json!([widget(0)]),
        "a refused update must change nothing, got {stored:?}"
    );
}

/// RULE: an update is bounded by the same published limits as a create — at most ten
/// widgets on a custom dashboard, and only a published refresh frequency at a real UTC
/// time of day — and a refused update changes nothing.
#[tokio::test]
async fn an_update_outside_the_published_bounds_is_refused() {
    let emulator = Emulator::new();
    let limit = state::CUSTOM_DASHBOARD_WIDGET_LIMIT;
    emulator
        .create_baseline("bounded-dashboard")
        .await
        .expect_ok("the dashboard to update");

    // Each count up to and including the limit is accepted.
    for count in [0, 1, limit - 1, limit] {
        let widgets: Vec<Value> = (0..count).map(widget).collect();
        let updated = emulator
            .update_dashboard("bounded-dashboard", &json!({"Widgets": widgets.clone()}))
            .await;
        assert_eq!(
            updated.expect_ok(&format!("{count} widgets"))["Widgets"],
            json!(widgets),
            "case {count} widgets: the stored widgets are not the ones requested, got \
             {updated:?}"
        );
    }
    // One past the limit, and well past it, are refused as quota failures.
    for count in [limit + 1, limit * 2] {
        let widgets: Vec<Value> = (0..count).map(widget).collect();
        emulator
            .update_dashboard("bounded-dashboard", &json!({"Widgets": widgets}))
            .await
            .expect_error(
                &format!("{count} widgets"),
                400,
                "ServiceQuotaExceededException",
            );
    }

    // Every published frequency is accepted on an update, including the entries no
    // other case here uses.
    let mut accepted: Vec<(String, Value)> = state::REFRESH_FREQUENCY_HOURS_VALUES
        .iter()
        .map(|value| {
            (
                format!("every {value} {}", state::REFRESH_FREQUENCY_UNIT_HOURS),
                schedule(state::REFRESH_FREQUENCY_UNIT_HOURS, *value, "ENABLED"),
            )
        })
        .collect();
    accepted.push((
        format!(
            "every {} {}",
            state::REFRESH_FREQUENCY_DAYS_VALUE,
            state::REFRESH_FREQUENCY_UNIT_DAYS
        ),
        schedule(
            state::REFRESH_FREQUENCY_UNIT_DAYS,
            state::REFRESH_FREQUENCY_DAYS_VALUE,
            "DISABLED",
        ),
    ));
    for (case, requested) in &accepted {
        let updated = emulator
            .update_dashboard("bounded-dashboard", &json!({"RefreshSchedule": requested}))
            .await;
        assert_eq!(
            updated.expect_ok(case)["RefreshSchedule"],
            *requested,
            "case {case:?}: the stored schedule must be the one requested, got {updated:?}"
        );
    }

    // One step outside the published set on each side, a value it does not publish,
    // and a time of day outside each of its two fields.
    let refused: Vec<(&str, Value)> = vec![
        (
            "zero hours, one step below the set",
            json!({"Frequency": {"Unit": "HOURS", "Value": 0}}),
        ),
        (
            "25 hours, one step above the set",
            json!({"Frequency": {"Unit": "HOURS", "Value": 25}}),
        ),
        (
            "5 hours, inside the range but unpublished",
            json!({"Frequency": {"Unit": "HOURS", "Value": 5}}),
        ),
        ("two days", json!({"Frequency": {"Unit": "DAYS", "Value": 2}})),
        (
            "hour 24",
            json!({"Frequency": {"Unit": "HOURS", "Value": 6}, "TimeOfDay": "24:00"}),
        ),
        (
            "minute 60",
            json!({"Frequency": {"Unit": "HOURS", "Value": 6}, "TimeOfDay": "00:60"}),
        ),
    ];
    for (case, requested) in refused {
        emulator
            .update_dashboard("bounded-dashboard", &json!({"RefreshSchedule": requested}))
            .await
            .expect_error(case, 400, "ValidationException");
    }

    // The last accepted update is still what the dashboard stores: nothing refused
    // changed it.
    let (_, last_accepted) = accepted.last().expect("the accepted rows are not empty");
    let stored = emulator.fetch_dashboard("bounded-dashboard").await;
    let stored = stored.expect_ok("the dashboard after the refusals").clone();
    assert_eq!(
        stored["RefreshSchedule"], *last_accepted,
        "a refused update must leave the stored schedule alone, got {stored:?}"
    );
    assert_eq!(
        stored["Widgets"],
        json!((0..limit).map(widget).collect::<Vec<Value>>()),
        "a refused update must leave the stored widgets alone, got {stored:?}"
    );
}

/// A widget carrying `statement` verbatim. `QueryStatement` and `ViewProperties` are
/// `RequestWidget`'s required members.
fn widget_querying(statement: &str) -> Value {
    json!({
        "QueryStatement": statement,
        "ViewProperties": {"type": "table"},
    })
}

/// An event data store id, in the form the modelled `UUID` shape pins
/// (`smithy_query(query="shape",shape="UUID")`: len 36..36, `^[a-f0-9\-]+$`), which is
/// what a widget's query names its table by.
const EVENT_DATA_STORE_ID: &str = "a7e5ba1c-9a2f-4c3b-8d6e-1f2a3b4c5d6e";
/// A second one, for the query that spans two stores.
const OTHER_EVENT_DATA_STORE_ID: &str = "b1c2d3e4-5a6b-4c7d-8e9f-0a1b2c3d4e5f";

/// RULE: a widget's `QueryStatement` has to be a query CloudTrail Lake can run — one
/// SQL `SELECT` over the event data stores it names — and a request carrying one that
/// is not is refused with `InvalidQueryStatementException`, storing nothing.
///
/// The rows differ in the FORM of the statement, which is what the rule generalises
/// over: how a runnable query names its store (a bare table, an event data store id,
/// an event data store ARN, two of them joined), what it substitutes into it (the
/// documented `$StartTime$` / `$EndTime$` / `$Period$` parameters and the quoted `?`,
/// both from the model doc for `RequestWidget`), and how an unrunnable one fails (it
/// does not parse, it is not SQL at all, it is more than one statement, or it is a
/// statement that is not a query).
#[tokio::test]
async fn an_update_whose_widget_query_cannot_run_is_refused() {
    let emulator = Emulator::new();
    emulator
        .create_baseline("queried-dashboard")
        .await
        .expect_ok("the dashboard to update");

    // Every one of these is ONE SELECT, so every one is runnable.
    let runnable: Vec<(&str, String)> = vec![
        (
            "a bare table name",
            "SELECT eventName FROM eds".to_string(),
        ),
        (
            "an event data store id as the table",
            format!("SELECT eventName, eventSource FROM {EVENT_DATA_STORE_ID}"),
        ),
        (
            "an event data store ARN as the table",
            format!(
                "SELECT eventSource, count(*) AS total FROM \
                 arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/\
                 {EVENT_DATA_STORE_ID} GROUP BY eventSource"
            ),
        ),
        (
            // "you can query across multiple event data stores as long as all event
            // data stores exist in your account" — the model doc for
            // `RequestWidget$QueryStatement`.
            "two event data stores joined",
            format!(
                "SELECT a.eventName FROM {EVENT_DATA_STORE_ID} AS a JOIN \
                 {OTHER_EVENT_DATA_STORE_ID} AS b ON a.eventID = b.eventID"
            ),
        ),
        (
            // "The following query parameters are valid: $StartTime$, $EndTime$, and
            // $Period$."
            "the documented query parameters",
            format!(
                "SELECT eventName FROM {EVENT_DATA_STORE_ID} WHERE eventTime > $StartTime$ \
                 AND eventTime < $EndTime$ AND period = $Period$"
            ),
        ),
        (
            // "When a query uses ? with eventTime, ? must be surrounded by single
            // quotes as follows: '?'."
            "a quoted placeholder",
            "SELECT * FROM eds WHERE eventTime > '?'".to_string(),
        ),
        (
            "a trailing semicolon",
            "SELECT eventName FROM eds;".to_string(),
        ),
        (
            // SQL escapes a quote inside a literal by doubling it, so the literal runs
            // past the doubled quote and the id after it is still read as a table.
            "a doubled-quote escape beside an event data store id",
            format!(
                "SELECT eventName FROM {EVENT_DATA_STORE_ID} WHERE userIdentity = 'it''s me'"
            ),
        ),
        (
            // A comment is not read as SQL, so neither the apostrophe nor the
            // id-shaped word inside one changes how the statement parses.
            "a line comment carrying an apostrophe",
            format!(
                "SELECT eventName FROM {EVENT_DATA_STORE_ID} -- it's the console logins\n"
            ),
        ),
        (
            "a block comment carrying an apostrophe",
            format!(
                "SELECT /* it's every event of {OTHER_EVENT_DATA_STORE_ID} */ eventName \
                 FROM {EVENT_DATA_STORE_ID}"
            ),
        ),
        (
            // 32 is the deepest bracketing `state::widget_query`'s own structural bound
            // admits (`MAX_QUERY_NESTING_DEPTH`); at the bound the statement is still
            // read and run.
            "bracketing at the structural bound",
            format!(
                "SELECT eventName FROM eds WHERE {}1 = 1{}",
                "(".repeat(32),
                ")".repeat(32)
            ),
        ),
        (
            // A chain of operators nests the parsed tree at bracket depth zero, so it
            // is the token bound, not the bracket bound, that keeps this one readable:
            // 2000 terms is about 4000 tokens, just inside `MAX_QUERY_TOKENS`.
            "a long unbracketed operator chain",
            format!(
                "SELECT eventName FROM eds WHERE 1 = {}1",
                "1 + ".repeat(2_000)
            ),
        ),
        (
            "a sub-query",
            "SELECT eventName FROM eds WHERE eventSource IN (SELECT eventSource FROM eds)"
                .to_string(),
        ),
    ];
    for (case, statement) in &runnable {
        let widgets = json!([widget_querying(statement)]);
        let updated = emulator
            .update_dashboard("queried-dashboard", &json!({"Widgets": widgets.clone()}))
            .await;
        assert_eq!(
            updated.expect_ok(case)["Widgets"][0]["QueryStatement"],
            json!(statement),
            "case {case:?}: a runnable query must be stored as written, got {updated:?}"
        );
    }

    let (_, last_runnable) = runnable.last().expect("the runnable rows are not empty");
    let unrunnable: Vec<(&str, String)> = vec![
        ("no statement at all", " ".to_string()),
        ("prose, not SQL", "show me every console login".to_string()),
        (
            "a WHERE with no predicate",
            "SELECT eventName FROM eds WHERE".to_string(),
        ),
        (
            "a statement cut off after FROM",
            "SELECT eventName FROM".to_string(),
        ),
        (
            "an unterminated string literal",
            "SELECT eventName FROM eds WHERE eventName = 'StartLogging".to_string(),
        ),
        (
            "an unterminated sub-query",
            "SELECT eventName FROM eds WHERE eventSource IN (SELECT eventSource FROM eds"
                .to_string(),
        ),
        (
            "an undocumented query parameter",
            "SELECT eventName FROM eds WHERE eventTime > $Whenever$".to_string(),
        ),
        ("two statements, not one", "SELECT 1; SELECT 2".to_string()),
        (
            "a write, not a query",
            "INSERT INTO eds VALUES ('StartLogging')".to_string(),
        ),
        (
            "a delete, not a query",
            "DELETE FROM eds WHERE eventName = 'StartLogging'".to_string(),
        ),
        ("a DDL statement, not a query", "DROP TABLE eds".to_string()),
        (
            // One bracket past that bound.
            "bracketing one level past the structural bound",
            format!(
                "SELECT eventName FROM eds WHERE {}1 = 1{}",
                "(".repeat(33),
                ")".repeat(33)
            ),
        ),
        (
            // Caller-controlled nesting, at nearly the whole of the modelled
            // `QueryStatement` maximum (`@length(1, 10000)`): the bound is checked on
            // the characters, so the statement is refused before any parse descends
            // with it.
            "4000 levels of nested parentheses",
            format!(
                "SELECT eventName FROM eds WHERE {}1{}",
                "(".repeat(4_000),
                ")".repeat(4_000)
            ),
        ),
        (
            "a deeply nested sub-query chain",
            format!(
                "SELECT eventName FROM eds WHERE eventSource IN {}SELECT eventSource FROM eds{}",
                "(SELECT eventSource FROM (".repeat(200),
                ")".repeat(400)
            ),
        ),
    ];
    for (case, statement) in &unrunnable {
        let refused = emulator
            .update_dashboard(
                "queried-dashboard",
                &json!({"Widgets": [widget_querying(statement)]}),
            )
            .await;
        refused.expect_error(case, 400, "InvalidQueryStatementException");
        // One sentence for the whole class, written out here rather than read from the
        // constant the handler uses, so a change to either is caught: the refusal names
        // no part of the statement.
        assert_eq!(
            refused.body["Message"],
            json!("Query is not valid."),
            "case {case:?}: wrong refusal message, got {refused:?}"
        );
    }

    // Nothing any of those refusals carried was stored: the dashboard still holds the
    // last update that was accepted.
    let stored = emulator.fetch_dashboard("queried-dashboard").await;
    let stored = stored.expect_ok("the dashboard after the refusals").clone();
    assert_eq!(
        stored["Widgets"],
        json!([widget_querying(last_runnable)]),
        "a refused update must leave the stored widgets alone, got {stored:?}"
    );

    // A list where only ONE statement cannot run is refused whole, so a partially
    // readable request stores none of its widgets.
    let mixed = emulator
        .update_dashboard(
            "queried-dashboard",
            &json!({
                "Widgets": [
                    widget_querying("SELECT eventName FROM eds"),
                    widget_querying("DROP TABLE eds"),
                ],
            }),
        )
        .await;
    mixed.expect_error(
        "one unrunnable statement among runnable ones",
        400,
        "InvalidQueryStatementException",
    );
    let after = emulator.fetch_dashboard("queried-dashboard").await;
    assert_eq!(
        after.expect_ok("the dashboard after the mixed list")["Widgets"],
        json!([widget_querying(last_runnable)]),
        "case one unrunnable statement among runnable ones: nothing may be stored, got \
         {after:?}"
    );
}

/// RULE: CloudTrail Lake manages the Highlights dashboard, so an update may not
/// compose its widgets or take it off its documented refresh schedule.
#[tokio::test]
async fn an_update_keeps_the_managed_dashboard_managed() {
    let emulator = Emulator::new();
    let highlights_schedule = schedule(
        state::REFRESH_FREQUENCY_UNIT_HOURS,
        state::HIGHLIGHTS_REFRESH_FREQUENCY_VALUE,
        state::REFRESH_SCHEDULE_ENABLED,
    );
    emulator
        .call(
            "CreateDashboard",
            &json!({
                "Name": state::HIGHLIGHTS_DASHBOARD_NAME,
                "RefreshSchedule": highlights_schedule,
            }),
        )
        .await
        .expect_ok("the Highlights dashboard");

    let refused: Vec<(&str, Value)> = vec![
        ("caller-composed widgets", json!({"Widgets": [widget(0)]})),
        (
            "a schedule at another frequency",
            json!({"RefreshSchedule": schedule(state::REFRESH_FREQUENCY_UNIT_HOURS, 12, "ENABLED")}),
        ),
        (
            "a schedule that is not enabled",
            json!({"RefreshSchedule": schedule(
                state::REFRESH_FREQUENCY_UNIT_HOURS,
                state::HIGHLIGHTS_REFRESH_FREQUENCY_VALUE,
                "DISABLED",
            )}),
        ),
    ];
    for (case, patch) in refused {
        emulator
            .update_dashboard(state::HIGHLIGHTS_DASHBOARD_NAME, &patch)
            .await
            .expect_error(case, 400, "ValidationException");
    }

    // An EMPTY widget list composes nothing, so it is the same no-op `CreateDashboard`
    // accepts for this dashboard.
    let empty = emulator
        .update_dashboard(state::HIGHLIGHTS_DASHBOARD_NAME, &json!({"Widgets": []}))
        .await;
    assert_eq!(
        empty.expect_ok("an empty widget list")["Widgets"],
        json!([]),
        "an empty widget list composes nothing on the managed dashboard, got {empty:?}"
    );

    // What the managed dashboard DOES accept: its own documented schedule, and
    // termination protection.
    let updated = emulator
        .update_dashboard(
            state::HIGHLIGHTS_DASHBOARD_NAME,
            &json!({"TerminationProtectionEnabled": true}),
        )
        .await;
    let updated = updated.expect_ok("termination protection").clone();
    assert_eq!(
        updated["Type"],
        json!(state::DASHBOARD_TYPE_MANAGED),
        "the managed dashboard must keep its type, got {updated:?}"
    );
    assert_eq!(
        updated["TerminationProtectionEnabled"],
        json!(true),
        "termination protection must be stored, got {updated:?}"
    );
    assert_eq!(
        updated["Widgets"],
        json!([]),
        "the managed dashboard carries no caller-composed widget, got {updated:?}"
    );
}

/// RULE: an update does not move the dashboard's `DashboardStatus` — the model
/// declares `UPDATING`/`UPDATED` but no recording shows CloudTrail putting a
/// dashboard back into a transient status for an update, so a settled dashboard stays
/// settled and keeps reporting the same resource.
#[tokio::test]
async fn an_update_leaves_the_settled_status_alone() {
    let emulator = Emulator::new();
    emulator
        .create_baseline("settled-dashboard")
        .await
        .expect_ok("the dashboard to update");
    sleep(state::DASHBOARD_SETTLE_AFTER + Duration::from_millis(250)).await;
    let created = emulator.fetch_dashboard("settled-dashboard").await;
    assert_eq!(
        created.expect_ok("after the create settles")["Status"],
        json!(state::DASHBOARD_STATUS_CREATED),
        "a created dashboard settles into CREATED, got {created:?}"
    );

    let updated = emulator
        .update_dashboard("settled-dashboard", &json!({"Widgets": [widget(5)]}))
        .await;
    assert_eq!(
        updated.expect_ok("the update")["Type"],
        json!(state::DASHBOARD_TYPE_CUSTOM),
        "the update must report the stored dashboard, got {updated:?}"
    );
    let after = emulator.fetch_dashboard("settled-dashboard").await;
    let after = after.expect_ok("after the update").clone();
    assert_eq!(
        after["Status"],
        json!(state::DASHBOARD_STATUS_CREATED),
        "an update must leave the settled status alone, got {after:?}"
    );
    assert_eq!(
        after["Widgets"],
        json!([widget(5)]),
        "the update must be what the dashboard now stores, got {after:?}"
    );
    let again = emulator.fetch_dashboard("settled-dashboard").await;
    assert_eq!(
        again.expect_ok("a repeated read"),
        &after,
        "a second read must report the same dashboard, got {again:?}"
    );
}

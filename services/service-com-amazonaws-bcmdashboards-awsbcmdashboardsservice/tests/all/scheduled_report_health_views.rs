// SPDX-License-Identifier: CC0-1.0

//! The health of a scheduled report, as every view of the same record reports it.
//!
//! The rule under test: a status a service DERIVES from the resources a stored record
//! points at is recomputed from the stored state every time it is reported, so every view
//! of one record reports the same value for it — whether that member is `@required` in one
//! projection and optional in another — and none reports a status the store itself
//! contradicts. Here the status is `healthStatus`: `GetScheduledReport`'s
//! `ScheduledReport` (optional member) and `ListScheduledReports`' `ScheduledReportSummary`
//! (`@required` member) must carry the IDENTICAL health for one report; a report whose
//! configuration resolves reports `HEALTHY` in both — the model documents `HEALTHY` as
//! "the scheduled report is configured properly and has all required permissions to
//! execute" — with no `statusReasons` (the model documents those as present only when the
//! status is `UNHEALTHY`) and no `lastRefreshedAt` until something has refreshed it; and a
//! report whose dashboard is no longer stored, or whose `widgetIds` name a widget that
//! dashboard no longer holds, reports `UNHEALTHY` with `DASHBOARD_NOT_FOUND` /
//! `WIDGET_ID_NOT_FOUND` whether or not anything has ever executed it.
//!
//! The table spans the record states the health can be in rather than one request: never
//! refreshed (with no `widgetIds`, with an empty list, with one id, and with the schedule
//! `DISABLED`), refreshed by an execution while every resource resolves, refreshed while
//! the dashboard or the named widget no longer resolves, and — with nothing refreshing it
//! at all — the dashboard deleted, its widgets replaced, the report re-pointed at a
//! dashboard that does not hold its widget, the widget ids cleared again after the widget
//! went away, and the dashboard deleted only AFTER a refresh had found the report healthy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{graph_config, ok_call, router};
// The `HealthStatusCode` and `StatusReason` values, and the two readers, declared once
// for the crate's suites in the execution suite beside the health they assert on.
use super::scheduled_report_execution::{
    create_report, listed_summary, stored_report, DASHBOARD_NOT_FOUND, HEALTHY, UNHEALTHY,
    WIDGET_ID_NOT_FOUND,
};
use super::scheduled_report_lifecycle::source_dashboard;

/// What happens to the report's resources before its health is read.
#[derive(Debug, Clone, Copy)]
enum Prepare {
    /// Nothing refreshes the health: the record keeps the health it was created with.
    NothingRefreshesIt,
    /// An execution refreshes the health with every resource still resolving.
    ExecuteIt,
    /// The dashboard the report renders is deleted, then the report is executed.
    DeleteTheDashboardThenExecute,
    /// The dashboard's widgets are replaced (so the id the report names is gone), then
    /// the report is executed.
    ReplaceTheWidgetThenExecute,
    /// The dashboard the report renders is deleted, and nothing refreshes the health.
    DeleteTheDashboard,
    /// The dashboard's widgets are replaced, and nothing refreshes the health.
    ReplaceTheWidget,
    /// The report is re-pointed at a second stored dashboard, which does not hold the
    /// widget the report names.
    RepointAtAnotherDashboard,
    /// The widgets are replaced and then the report's `widgetIds` are cleared, so the
    /// configuration resolves again.
    ReplaceTheWidgetThenClearTheWidgetIds,
    /// The report is executed while everything resolves, and only THEN is the dashboard
    /// deleted: the record carries a refresh that found the report healthy.
    ExecuteItThenDeleteTheDashboard,
}

/// One row: a report to build, what to do to it, and the health BOTH views must report.
#[derive(Debug, Clone, Copy)]
struct HealthCase {
    label: &'static str,
    name: &'static str,
    /// `None` leaves `scheduleConfig.state` out of the create request.
    schedule_state: Option<&'static str>,
    /// `None` leaves `widgetIds` out, `Some(false)` sends an empty list, `Some(true)`
    /// names the dashboard's real widget.
    names_widget: Option<bool>,
    prepare: Prepare,
    expected_code: &'static str,
    /// `None` means the member must be absent.
    expected_reasons: Option<&'static [&'static str]>,
    /// Whether the health must carry a refresh instant.
    expect_refreshed: bool,
}

const CASES: &[HealthCase] = &[
    HealthCase {
        label: "never refreshed, no widgetIds",
        name: "Fresh No Widgets",
        schedule_state: None,
        names_widget: None,
        prepare: Prepare::NothingRefreshesIt,
        expected_code: HEALTHY,
        expected_reasons: None,
        expect_refreshed: false,
    },
    HealthCase {
        label: "never refreshed, empty widgetIds",
        name: "Fresh Empty Widgets",
        schedule_state: Some("ENABLED"),
        names_widget: Some(false),
        prepare: Prepare::NothingRefreshesIt,
        expected_code: HEALTHY,
        expected_reasons: None,
        expect_refreshed: false,
    },
    HealthCase {
        label: "never refreshed, one widget id",
        name: "Fresh One Widget",
        schedule_state: None,
        names_widget: Some(true),
        prepare: Prepare::NothingRefreshesIt,
        expected_code: HEALTHY,
        expected_reasons: None,
        expect_refreshed: false,
    },
    HealthCase {
        label: "never refreshed, schedule DISABLED",
        name: "Fresh Disabled",
        schedule_state: Some("DISABLED"),
        names_widget: Some(true),
        prepare: Prepare::NothingRefreshesIt,
        expected_code: HEALTHY,
        expected_reasons: None,
        expect_refreshed: false,
    },
    HealthCase {
        label: "refreshed while every resource resolves",
        name: "Executed Healthy",
        schedule_state: None,
        names_widget: Some(true),
        prepare: Prepare::ExecuteIt,
        expected_code: HEALTHY,
        expected_reasons: None,
        expect_refreshed: true,
    },
    HealthCase {
        label: "refreshed after the dashboard was deleted",
        name: "Executed No Dashboard",
        schedule_state: Some("ENABLED"),
        names_widget: None,
        prepare: Prepare::DeleteTheDashboardThenExecute,
        expected_code: UNHEALTHY,
        expected_reasons: Some(&[DASHBOARD_NOT_FOUND]),
        expect_refreshed: true,
    },
    HealthCase {
        label: "refreshed after the named widget was replaced",
        name: "Executed No Widget",
        schedule_state: None,
        names_widget: Some(true),
        prepare: Prepare::ReplaceTheWidgetThenExecute,
        expected_code: UNHEALTHY,
        expected_reasons: Some(&[WIDGET_ID_NOT_FOUND]),
        expect_refreshed: true,
    },
    HealthCase {
        label: "dashboard deleted, never refreshed, no widgetIds",
        name: "Deleted No Widgets",
        schedule_state: None,
        names_widget: None,
        prepare: Prepare::DeleteTheDashboard,
        expected_code: UNHEALTHY,
        expected_reasons: Some(&[DASHBOARD_NOT_FOUND]),
        expect_refreshed: false,
    },
    HealthCase {
        label: "dashboard deleted, never refreshed, empty widgetIds",
        name: "Deleted Empty Widgets",
        schedule_state: Some("DISABLED"),
        names_widget: Some(false),
        prepare: Prepare::DeleteTheDashboard,
        expected_code: UNHEALTHY,
        expected_reasons: Some(&[DASHBOARD_NOT_FOUND]),
        expect_refreshed: false,
    },
    HealthCase {
        label: "dashboard deleted, never refreshed, one widget id",
        name: "Deleted One Widget",
        schedule_state: Some("ENABLED"),
        names_widget: Some(true),
        prepare: Prepare::DeleteTheDashboard,
        expected_code: UNHEALTHY,
        expected_reasons: Some(&[DASHBOARD_NOT_FOUND]),
        expect_refreshed: false,
    },
    HealthCase {
        label: "named widget replaced, never refreshed",
        name: "Replaced One Widget",
        schedule_state: None,
        names_widget: Some(true),
        prepare: Prepare::ReplaceTheWidget,
        expected_code: UNHEALTHY,
        expected_reasons: Some(&[WIDGET_ID_NOT_FOUND]),
        expect_refreshed: false,
    },
    HealthCase {
        label: "widgets replaced while the report names none",
        name: "Replaced No Ids",
        schedule_state: None,
        names_widget: Some(false),
        prepare: Prepare::ReplaceTheWidget,
        expected_code: HEALTHY,
        expected_reasons: None,
        expect_refreshed: false,
    },
    HealthCase {
        label: "re-pointed at a dashboard without the widget it names",
        name: "Repointed Report",
        schedule_state: None,
        names_widget: Some(true),
        prepare: Prepare::RepointAtAnotherDashboard,
        expected_code: UNHEALTHY,
        expected_reasons: Some(&[WIDGET_ID_NOT_FOUND]),
        expect_refreshed: false,
    },
    HealthCase {
        label: "widget ids cleared again after the widget went away",
        name: "Repaired Report",
        schedule_state: None,
        names_widget: Some(true),
        prepare: Prepare::ReplaceTheWidgetThenClearTheWidgetIds,
        expected_code: HEALTHY,
        expected_reasons: None,
        expect_refreshed: false,
    },
    HealthCase {
        label: "dashboard deleted after a refresh found the report healthy",
        name: "Stale Refresh",
        schedule_state: Some("ENABLED"),
        names_widget: Some(true),
        prepare: Prepare::ExecuteItThenDeleteTheDashboard,
        expected_code: UNHEALTHY,
        expected_reasons: Some(&[DASHBOARD_NOT_FOUND]),
        expect_refreshed: true,
    },
];

/// The name the source dashboard of every row is created under, needed again by the
/// update that replaces its widgets (`name` is required on `UpdateDashboard`).
const SOURCE_DASHBOARD_NAME: &str = "HealthViewSource";

/// Replace the source dashboard's widgets, so the id a report names is gone and the
/// replacement carries a freshly minted one: `UpdateDashboard`'s `widgets` replaces all
/// existing widgets.
async fn replace_the_widget(router: &MultiServiceRouter, dashboard_arn: &str) {
    ok_call(
        router,
        "UpdateDashboard",
        &json!({
            "arn": dashboard_arn,
            "name": SOURCE_DASHBOARD_NAME,
            "widgets": [{
                "title": "Replacement",
                "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
            }]
        }),
    )
    .await;
}

/// Build the row's report in a fresh store, carry out its `prepare`, and answer its ARN.
async fn build_case(router: &MultiServiceRouter, case: &HealthCase) -> String {
    let (dashboard_arn, widget_id) = source_dashboard(router, SOURCE_DASHBOARD_NAME).await;
    let widget_ids = case.names_widget.map(|names| {
        if names {
            vec![widget_id.clone()]
        } else {
            Vec::new()
        }
    });
    let arn = create_report(
        router,
        &dashboard_arn,
        case.name,
        case.schedule_state,
        widget_ids,
    )
    .await;

    match case.prepare {
        Prepare::NothingRefreshesIt => {}
        Prepare::ExecuteIt => {
            ok_call(router, "ExecuteScheduledReport", &json!({"arn": arn})).await;
        }
        Prepare::DeleteTheDashboardThenExecute => {
            ok_call(router, "DeleteDashboard", &json!({"arn": dashboard_arn})).await;
            ok_call(router, "ExecuteScheduledReport", &json!({"arn": arn})).await;
        }
        Prepare::ReplaceTheWidgetThenExecute => {
            replace_the_widget(router, &dashboard_arn).await;
            ok_call(router, "ExecuteScheduledReport", &json!({"arn": arn})).await;
        }
        Prepare::DeleteTheDashboard => {
            ok_call(router, "DeleteDashboard", &json!({"arn": dashboard_arn})).await;
        }
        Prepare::ReplaceTheWidget => replace_the_widget(router, &dashboard_arn).await,
        Prepare::RepointAtAnotherDashboard => {
            let (other_arn, _) = source_dashboard(router, "HealthViewOtherSource").await;
            ok_call(
                router,
                "UpdateScheduledReport",
                &json!({"arn": arn, "dashboardArn": other_arn}),
            )
            .await;
        }
        Prepare::ReplaceTheWidgetThenClearTheWidgetIds => {
            replace_the_widget(router, &dashboard_arn).await;
            ok_call(
                router,
                "UpdateScheduledReport",
                &json!({"arn": arn, "clearWidgetIds": true}),
            )
            .await;
        }
        Prepare::ExecuteItThenDeleteTheDashboard => {
            ok_call(router, "ExecuteScheduledReport", &json!({"arn": arn})).await;
            ok_call(router, "DeleteDashboard", &json!({"arn": dashboard_arn})).await;
        }
    }
    arn
}

/// Both views of one stored report report the SAME health — member for member — and that
/// health is the one the record's state calls for.
#[tokio::test]
async fn both_views_of_one_report_report_the_same_health() {
    for case in CASES {
        let label = case.label;
        // A fresh store per row, so one row's execution cannot be read as another's.
        let router = router();
        let arn = build_case(&router, case).await;

        let detail = stored_report(&router, &arn).await;
        let summary = listed_summary(&router, &arn).await;

        let detail_health = detail.get("healthStatus").unwrap_or_else(|| {
            panic!("case {label}: the detail view reports no healthStatus: {detail}")
        });
        let summary_health = summary.get("healthStatus").unwrap_or_else(|| {
            panic!("case {label}: the summary reports no healthStatus: {summary}")
        });
        assert_eq!(
            detail_health, summary_health,
            "case {label}: the two views of one record must report one health, got {detail_health} vs {summary_health}"
        );

        for (view, health) in [("detail", detail_health), ("summary", summary_health)] {
            assert_eq!(
                health.get("statusCode").and_then(Value::as_str),
                Some(case.expected_code),
                "case {label} ({view}): expected statusCode {:?}, got {health}",
                case.expected_code
            );
            match case.expected_reasons {
                Some(expected) => assert_eq!(
                    health.get("statusReasons"),
                    Some(&json!(expected)),
                    "case {label} ({view}): expected statusReasons {expected:?}, got {health}"
                ),
                None => assert!(
                    health.get("statusReasons").is_none(),
                    "case {label} ({view}): a healthy report carries no statusReasons, got {health}"
                ),
            }
            assert_eq!(
                health.get("lastRefreshedAt").is_some(),
                case.expect_refreshed,
                "case {label} ({view}): expected lastRefreshedAt present={}, got {health}",
                case.expect_refreshed
            );
        }

        // Reading either view again answers the same health: neither read refreshed it.
        assert_eq!(
            stored_report(&router, &arn).await.get("healthStatus"),
            Some(detail_health),
            "case {label}: a second detail read must answer the same health"
        );
        assert_eq!(
            listed_summary(&router, &arn).await.get("healthStatus"),
            Some(summary_health),
            "case {label}: a second listing must answer the same health"
        );
    }
}

/// One row of the unrefreshed table: a report to create, in a store it shares with the
/// other rows.
struct UnrefreshedRow {
    label: &'static str,
    /// `None` leaves `scheduleConfig.state` out of the create request.
    schedule_state: Option<&'static str>,
    /// `None` leaves `widgetIds` out, otherwise the list the request submits.
    widget_ids: Option<Vec<String>>,
}

/// An unrefreshed report reports exactly `{"statusCode": "HEALTHY"}` in both views: the
/// resolution invents no refresh instant and no reasons for a refresh that never happened.
#[tokio::test]
async fn an_unrefreshed_report_reports_healthy_and_nothing_else() {
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "UnrefreshedSource").await;

    // Rows differing in widget-id count and schedule state, all in one store, so the
    // listing projects several records at once.
    let rows = vec![
        UnrefreshedRow {
            label: "no widgetIds, state omitted",
            schedule_state: None,
            widget_ids: None,
        },
        UnrefreshedRow {
            label: "empty widgetIds, ENABLED",
            schedule_state: Some("ENABLED"),
            widget_ids: Some(Vec::new()),
        },
        UnrefreshedRow {
            label: "one widget id, DISABLED",
            schedule_state: Some("DISABLED"),
            widget_ids: Some(vec![widget_id]),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let label = row.label;
        let arn = create_report(
            &router,
            &dashboard_arn,
            &format!("Unrefreshed {index}"),
            row.schedule_state,
            row.widget_ids,
        )
        .await;
        let expected = json!({"statusCode": HEALTHY});
        assert_eq!(
            stored_report(&router, &arn).await.get("healthStatus"),
            Some(&expected),
            "case {label}: the detail view reports HEALTHY and nothing else"
        );
        assert_eq!(
            listed_summary(&router, &arn).await.get("healthStatus"),
            Some(&expected),
            "case {label}: the summary reports HEALTHY and nothing else"
        );
    }
}

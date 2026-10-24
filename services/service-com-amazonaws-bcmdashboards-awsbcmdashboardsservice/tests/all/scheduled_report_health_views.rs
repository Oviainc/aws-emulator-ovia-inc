// SPDX-License-Identifier: CC0-1.0

//! The health of a scheduled report, as every view of the same record reports it.
//!
//! The rule under test: a member that reports a property of a stored resource is
//! resolved from the record, so every view of one record reports the same value for it —
//! whether that member is `@required` in one projection and optional in another. Here
//! the property is `healthStatus`: `GetScheduledReport`'s `ScheduledReport` (optional
//! member) and `ListScheduledReports`' `ScheduledReportSummary` (`@required` member)
//! must carry the IDENTICAL health for one report, and a report nothing has refreshed
//! reports `HEALTHY` in both — the model documents `HEALTHY` as "the scheduled report is
//! configured properly and has all required permissions to execute", which is what a
//! report this service accepted at create time is — with no `statusReasons` (the model
//! documents those as present only when the status is `UNHEALTHY`) and no
//! `lastRefreshedAt` (nothing has refreshed it).
//!
//! The table spans the record states the health can be in rather than one request: never
//! refreshed (with no `widgetIds`, with an empty list, with one id, and with the schedule
//! `DISABLED`), refreshed by an execution while every resource resolves, and refreshed
//! while the dashboard or the named widget no longer resolves.

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
];

/// Build the row's report in a fresh store, carry out its `prepare`, and answer its ARN.
async fn build_case(router: &MultiServiceRouter, case: &HealthCase) -> String {
    let (dashboard_arn, widget_id) = source_dashboard(router, "HealthViewSource").await;
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
            // `UpdateDashboard`'s `widgets` replaces all existing widgets, so the id the
            // report names is gone and the replacement carries a new one.
            ok_call(
                router,
                "UpdateDashboard",
                &json!({
                    "arn": dashboard_arn,
                    "name": "HealthViewSource",
                    "widgets": [{
                        "title": "Replacement",
                        "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                    }]
                }),
            )
            .await;
            ok_call(router, "ExecuteScheduledReport", &json!({"arn": arn})).await;
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

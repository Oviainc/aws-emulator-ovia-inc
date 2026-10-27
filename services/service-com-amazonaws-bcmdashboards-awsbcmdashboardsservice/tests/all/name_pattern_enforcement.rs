// SPDX-License-Identifier: CC0-1.0

//! The `@pattern` of a constrained string this service's boundary validator cannot
//! apply, enforced end to end over the wire.
//!
//! The rule under test: a value outside the `@pattern` its member's shape declares is
//! refused with `ValidationException` (HTTP 400) and changes no state, for EVERY
//! `@pattern` the model declares — including the ones whose expression carries a
//! construct the boundary validator's regex engine cannot compile, which it then treats
//! as matching. Three shapes of this service are in that class, and all three carry the
//! negative lookahead `(?!.* {2})` (no two consecutive spaces):
//!
//! | shape                                 | member                                      |
//! |---------------------------------------|---------------------------------------------|
//! | `DashboardName` / `ScheduledReportName` | `CreateDashboard.name`, `UpdateDashboard.name`, `CreateScheduledReport.scheduledReport.name`, `UpdateScheduledReport.name` |
//! | `Description`                         | the `description` of each of those, and of a widget |
//! | `WidgetTitle`                         | `widgets[].title`                           |
//!
//! Source for every expression below:
//! `smithy_query(query="shape",shape="DashboardName,ScheduledReportName,Description,WidgetTitle")`.
//! The refusal states the expression WITHOUT the `^`/`$` the model wraps it in, which is
//! how this service publishes a pattern requirement
//! (`tests/all/validation_pattern_rendering.rs` pins that rendering for the patterns the
//! boundary itself judges) — so a client cannot tell which layer applied the pattern.
//!
//! The rows span what each grammar generalises over: the lookahead the recorded case
//! broke (a double space), each end of a two-ended expression, a character outside the
//! admitted class, the nesting of a member inside a list element and inside a structure,
//! and a request breaking two of them at once. The acceptance table spans the same
//! members with values each grammar admits, including both published `@length` bounds.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, graph_config, ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// `DashboardName`'s and `ScheduledReportName`'s expression, as this service publishes
/// it: the modelled `@pattern` without its anchors.
const NAME_EXPRESSION: &str = r"(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]";

/// `Description`'s expression, published the same way.
const DESCRIPTION_EXPRESSION: &str = r"(?!.* {2})[ a-zA-Z0-9.,!?;:@#$%&\-_/\\]*";

/// `WidgetTitle`'s expression, published the same way.
const WIDGET_TITLE_EXPRESSION: &str = r"(?!.* {2})[a-zA-Z0-9_-][ a-zA-Z0-9_-]*[a-zA-Z0-9_-]";

/// The clause a `@pattern` violation states.
fn pattern_clause(value: &str, path: &str, expression: &str) -> String {
    format!(
        "Value '{value}' at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {expression}"
    )
}

/// The envelope over the clauses of one refusal.
fn envelope(clauses: &[String]) -> String {
    match clauses {
        [single] => format!("1 validation error detected: {single}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The `message` of an error body.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A widget whose title and configs are well formed, so a row that breaks another
/// member's pattern breaks only that one.
fn valid_widget() -> Value {
    json!({"title": "Spend", "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]})
}

/// A well-formed `DashboardArn` for a dashboard nothing is stored under, and a
/// well-formed `ScheduledReportArn` likewise: a refusal of a pattern is answered before
/// any resource is resolved, so a refused row needs no stored resource.
const UNKNOWN_DASHBOARD_ARN: &str = "arn:aws:bcm-dashboards::000000000000:dashboard/never-created";
const UNKNOWN_REPORT_ARN: &str =
    "arn:aws:bcm-dashboards::000000000000:scheduled-report/never-created";

/// One row of the refusal table.
struct RefusalCase {
    label: &'static str,
    operation: &'static str,
    request: Value,
    /// The clauses the refusal must state, in the order the request submits the members.
    clauses: Vec<String>,
}

fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            label: "a dashboard name holding two consecutive spaces (the lookahead)",
            operation: "CreateDashboard",
            request: json!({"name": "Bad  Name", "widgets": [valid_widget()]}),
            clauses: vec![pattern_clause("Bad  Name", "name", NAME_EXPRESSION)],
        },
        RefusalCase {
            label: "a dashboard name opening with a digit, which its first class excludes",
            operation: "CreateDashboard",
            request: json!({"name": "2026 Spend", "widgets": []}),
            clauses: vec![pattern_clause("2026 Spend", "name", NAME_EXPRESSION)],
        },
        RefusalCase {
            label: "a dashboard name ending in a space, which its last class excludes",
            operation: "CreateDashboard",
            request: json!({"name": "Monthly Spend ", "widgets": []}),
            clauses: vec![pattern_clause("Monthly Spend ", "name", NAME_EXPRESSION)],
        },
        RefusalCase {
            label: "a dashboard name holding a character outside the admitted class",
            operation: "CreateDashboard",
            request: json!({"name": "Spend (EU)", "widgets": []}),
            clauses: vec![pattern_clause("Spend (EU)", "name", NAME_EXPRESSION)],
        },
        RefusalCase {
            label: "a dashboard description holding two consecutive spaces",
            operation: "CreateDashboard",
            request: json!({
                "name": "Described Dashboard",
                "description": "Monthly  spend",
                "widgets": []
            }),
            clauses: vec![pattern_clause(
                "Monthly  spend",
                "description",
                DESCRIPTION_EXPRESSION,
            )],
        },
        RefusalCase {
            label: "a dashboard description holding a character outside the admitted class",
            operation: "CreateDashboard",
            request: json!({
                "name": "Starred Dashboard",
                "description": "Monthly spend *",
                "widgets": []
            }),
            clauses: vec![pattern_clause(
                "Monthly spend *",
                "description",
                DESCRIPTION_EXPRESSION,
            )],
        },
        RefusalCase {
            label: "a widget title holding two consecutive spaces, nested in a list",
            operation: "CreateDashboard",
            request: json!({
                "name": "Widget Titles",
                "widgets": [
                    valid_widget(),
                    {"title": "Total  Cost", "configs": [graph_config("Cost", "BAR", "DAILY")]}
                ]
            }),
            clauses: vec![pattern_clause(
                "Total  Cost",
                "widgets.2.member.title",
                WIDGET_TITLE_EXPRESSION,
            )],
        },
        RefusalCase {
            label: "a widget title opening with a space, which its first class excludes",
            operation: "CreateDashboard",
            request: json!({
                "name": "Leading Space Title",
                "widgets": [{"title": " Spend", "configs": [graph_config("Cost", "BAR", "DAILY")]}]
            }),
            clauses: vec![pattern_clause(
                " Spend",
                "widgets.1.member.title",
                WIDGET_TITLE_EXPRESSION,
            )],
        },
        RefusalCase {
            label: "a widget title holding a period, which its class excludes",
            operation: "CreateDashboard",
            request: json!({
                "name": "Punctuated Title",
                "widgets": [{"title": "Spend.", "configs": [graph_config("Cost", "BAR", "DAILY")]}]
            }),
            clauses: vec![pattern_clause(
                "Spend.",
                "widgets.1.member.title",
                WIDGET_TITLE_EXPRESSION,
            )],
        },
        RefusalCase {
            label: "a widget description outside its class, nested in a list element",
            operation: "CreateDashboard",
            request: json!({
                "name": "Widget Descriptions",
                "widgets": [{
                    "title": "Spend",
                    "description": "Cost per <service>",
                    "configs": [graph_config("Cost", "BAR", "DAILY")]
                }]
            }),
            clauses: vec![pattern_clause(
                "Cost per <service>",
                "widgets.1.member.description",
                DESCRIPTION_EXPRESSION,
            )],
        },
        RefusalCase {
            label: "a create breaking the name's and a widget title's patterns at once",
            operation: "CreateDashboard",
            request: json!({
                "name": "Two  Faults",
                "widgets": [{"title": "Total  Cost", "configs": [graph_config("Cost", "BAR", "DAILY")]}]
            }),
            clauses: vec![
                pattern_clause("Two  Faults", "name", NAME_EXPRESSION),
                pattern_clause(
                    "Total  Cost",
                    "widgets.1.member.title",
                    WIDGET_TITLE_EXPRESSION,
                ),
            ],
        },
        RefusalCase {
            label: "an update renaming a dashboard to a name holding two spaces",
            operation: "UpdateDashboard",
            request: json!({"arn": UNKNOWN_DASHBOARD_ARN, "name": "Bad  Rename"}),
            clauses: vec![pattern_clause("Bad  Rename", "name", NAME_EXPRESSION)],
        },
        RefusalCase {
            label: "an update supplying a widget title outside its class",
            operation: "UpdateDashboard",
            request: json!({
                "arn": UNKNOWN_DASHBOARD_ARN,
                "name": "Good Rename",
                "widgets": [{"title": "Cost+Usage", "configs": [graph_config("Cost", "BAR", "DAILY")]}]
            }),
            clauses: vec![pattern_clause(
                "Cost+Usage",
                "widgets.1.member.title",
                WIDGET_TITLE_EXPRESSION,
            )],
        },
        RefusalCase {
            label: "a report name holding two consecutive spaces, nested in a structure",
            operation: "CreateScheduledReport",
            request: json!({
                "scheduledReport": {
                    "name": "Bad  Report",
                    "dashboardArn": UNKNOWN_DASHBOARD_ARN,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
                }
            }),
            clauses: vec![pattern_clause(
                "Bad  Report",
                "scheduledReport.name",
                NAME_EXPRESSION,
            )],
        },
        RefusalCase {
            label: "a report description outside its class, nested in a structure",
            operation: "CreateScheduledReport",
            request: json!({
                "scheduledReport": {
                    "name": "Good Report",
                    "description": "Sent to finance <weekly>",
                    "dashboardArn": UNKNOWN_DASHBOARD_ARN,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
                }
            }),
            clauses: vec![pattern_clause(
                "Sent to finance <weekly>",
                "scheduledReport.description",
                DESCRIPTION_EXPRESSION,
            )],
        },
        RefusalCase {
            label: "an update renaming a report to a name ending in a hyphenated space",
            operation: "UpdateScheduledReport",
            request: json!({"arn": UNKNOWN_REPORT_ARN, "name": "Report  Rename"}),
            clauses: vec![pattern_clause("Report  Rename", "name", NAME_EXPRESSION)],
        },
    ]
}

/// Every row is refused with HTTP 400 `ValidationException` and the clauses its broken
/// patterns state, under the envelope that counts them.
#[tokio::test]
async fn a_value_outside_an_unapplied_pattern_is_refused() {
    for case in refusal_cases() {
        let router = router();
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 from {}, got {status}: {body}",
            case.label, case.operation
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {:?}: expected ValidationException, got {:?}: {body}",
            case.label,
            error_code(&body)
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            envelope(&case.clauses),
            "case {:?}: expected the refusal to state its clauses, got {message:?}",
            case.label
        );
    }
}

/// A refused create stores nothing: the account still holds no dashboard of its own, so
/// the refusal happened before the record was written and before an ARN was minted.
#[tokio::test]
async fn a_refused_write_changes_no_state() {
    let router = router();

    for case in refusal_cases() {
        let (status, _body) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: the row must be refused for this test to mean anything",
            case.label
        );
    }

    // Nothing of the account's own was created by any of them: every dashboard the store
    // holds is one of the AWS-managed catalog's, and no scheduled report exists.
    let listed = ok_call(&router, "ListDashboards", &json!({"maxResults": 100})).await;
    let own: Vec<&Value> = listed["dashboards"]
        .as_array()
        .expect("dashboards")
        .iter()
        .filter(|item| item["type"].as_str() == Some("CUSTOM"))
        .collect();
    assert!(
        own.is_empty(),
        "the refused creates must have stored no dashboard, got {own:?}"
    );
    let reports = ok_call(&router, "ListScheduledReports", &json!({"maxResults": 100})).await;
    assert_eq!(
        reports["scheduledReports"].as_array().map(Vec::len),
        Some(0),
        "the refused creates must have stored no scheduled report, got {reports}"
    );

    // And a refused UPDATE leaves the whole stored record as it was.
    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "Untouched Dashboard",
            "description": "Before the refused update",
            "widgets": [valid_widget()]
        }),
    )
    .await;
    let arn = created["arn"].as_str().expect("an arn").to_string();
    let before = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
    let (status, body) = call(
        &router,
        "UpdateDashboard",
        &json!({
            "arn": arn,
            "name": "Refused  Rename",
            "description": "After the refused update",
            "widgets": []
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a refused rename: expected HTTP 400, got {status}: {body}"
    );
    let after = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
    assert_eq!(
        after, before,
        "case a refused rename: the whole record must be untouched, got {after} after {before}"
    );
}

/// The refusals above are about the VALUE submitted: a value each grammar admits is
/// accepted on the same members and reads back from the stored record.
#[tokio::test]
async fn the_same_members_still_accept_a_value_their_pattern_admits() {
    let router = router();

    // Names spanning the admitted classes and both published `@length` bounds (2..50).
    let names = [
        ("at the 2-character minimum", "Em".to_string()),
        ("a single space between words", "Monthly Spend".to_string()),
        ("every admitted character class", "A_b-C 1".to_string()),
        (
            "at the 50-character maximum",
            format!("A{}z", "b".repeat(48)),
        ),
    ];
    // Descriptions spanning the punctuation its class lists, at both `@length` bounds
    // (1..200), including the backslash and the characters a URL-ish value carries.
    let descriptions = [
        ("at the 1-character minimum", "a".to_string()),
        (
            "punctuation the class lists",
            "50% of cost, e.g. a/b\\c: spend @ 2026; up? yes!".to_string(),
        ),
        ("at the 200-character maximum", "d".repeat(200)),
    ];
    // Titles spanning the admitted classes at both `@length` bounds (2..50).
    let titles = [
        ("at the 2-character minimum", "Em".to_string()),
        ("digits, hyphen and underscore", "Spend-By_Service 2026".to_string()),
        ("at the 50-character maximum", format!("A{}z", "b".repeat(48))),
    ];

    for (index, (name_label, name)) in names.iter().enumerate() {
        let (description_label, description) = &descriptions[index % descriptions.len()];
        let (title_label, title) = &titles[index % titles.len()];
        let label = format!("name {name_label}, description {description_label}, title {title_label}");

        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": name,
                "description": description,
                "widgets": [{
                    "title": title,
                    "description": description,
                    "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
                }]
            }),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: CreateDashboard must answer an arn: {created}"))
            .to_string();

        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            fetched["name"].as_str(),
            Some(name.as_str()),
            "case {label}: the stored name must be the submitted one: {fetched}"
        );
        assert_eq!(
            fetched["description"].as_str(),
            Some(description.as_str()),
            "case {label}: the stored description must be the submitted one: {fetched}"
        );
        assert_eq!(
            fetched["widgets"][0]["title"].as_str(),
            Some(title.as_str()),
            "case {label}: the stored widget title must be the submitted one: {fetched}"
        );
    }

    // A scheduled report carries the same two grammars, on a structure member.
    let (dashboard_arn, _widget) = source_dashboard(&router, "Pattern Report Source").await;
    accepted_report(&router, &dashboard_arn, "Weekly Report", Some("Sent on Mondays")).await;
    accepted_report(&router, &dashboard_arn, "Em", None).await;
}

/// Create a report whose name and description each grammar admits, rename it to another
/// such name, and read the stored record back.
async fn accepted_report(
    router: &MultiServiceRouter,
    dashboard_arn: &str,
    name: &str,
    description: Option<&str>,
) {
    let mut report = json!({
        "name": name,
        "dashboardArn": dashboard_arn,
        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
        "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
    });
    if let Some(description) = description {
        report["description"] = json!(description);
    }
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &json!({"scheduledReport": report}),
    )
    .await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case report {name:?}: no arn in {created}"))
        .to_string();

    let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
    assert_eq!(
        fetched["scheduledReport"]["name"].as_str(),
        Some(name),
        "case report {name:?}: the stored name must be the submitted one: {fetched}"
    );

    let renamed = format!("{name} Renamed");
    ok_call(
        router,
        "UpdateScheduledReport",
        &json!({"arn": arn, "name": renamed}),
    )
    .await;
    let after = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
    assert_eq!(
        after["scheduledReport"]["name"].as_str(),
        Some(renamed.as_str()),
        "case report {name:?}: the accepted rename must be stored: {after}"
    );
}

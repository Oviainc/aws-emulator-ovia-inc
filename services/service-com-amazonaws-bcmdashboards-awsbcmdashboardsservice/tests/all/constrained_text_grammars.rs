// SPDX-License-Identifier: CC0-1.0

//! Every constrained string of this service is judged by the modelled `@pattern`
//! EXPRESSION its refusal publishes, on every write that submits one.
//!
//! The rule under test: a value of a constrained string is admitted exactly when the
//! `@pattern` the member's shape declares matches it, and refused with the clause that
//! quotes that same expression otherwise — one statement of each rule, applied by every
//! write that carries such a member. The three shapes whose expression the boundary
//! validator cannot apply (each opens with the negative lookahead `(?!.* {2})`) are
//! `DashboardName`/`ScheduledReportName`, `Description` and `WidgetTitle`; source for all
//! three:
//! `smithy_query(query="shape",shape="DashboardName,ScheduledReportName,Description,WidgetTitle")`.
//!
//! The expressions are read from the one place this service states them
//! (`state::RESOURCE_NAME_PATTERN`, `state::DESCRIPTION_PATTERN`,
//! `state::WIDGET_TITLE_PATTERN` — the constants a refusal quotes), so a row below states
//! only the VERDICT: whether the service admits the value or refuses it. The rows are
//! values the sibling suites do not cite, picked where a character-by-character reading
//! and the expression itself could disagree — each end of a two-ended expression, the
//! characters just outside a published class, a line break (the one character a lookahead
//! written over `.` cannot see) both inside a value and at its very END, where a
//! Perl-style `$` would differ from an end-of-text one, a non-ASCII letter, and the
//! longest interior the name expression's `{0,48}` admits.
//!
//! The four writes that submit one of these members all go through the shared check:
//! `CreateDashboard` and `UpdateDashboard` (a dashboard's name and description, and each
//! widget's title and description), `CreateScheduledReport` and `UpdateScheduledReport` (a
//! report's name and description). Every call goes through the real router, so what these
//! tests assert is what an SDK client sees, and an admitted value is read back from the
//! stored record.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::{
    DESCRIPTION_PATTERN, RESOURCE_NAME_PATTERN, WIDGET_TITLE_PATTERN,
};

use super::dashboard_lifecycle::{call, error_code, graph_config, ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// A well-formed `DashboardArn` and `ScheduledReportArn` nothing is stored under: a
/// pattern refusal is answered before the ARN is resolved, so a refused row needs no
/// stored resource.
const UNKNOWN_DASHBOARD_ARN: &str = "arn:aws:bcm-dashboards::000000000000:dashboard/never-created";
const UNKNOWN_REPORT_ARN: &str =
    "arn:aws:bcm-dashboards::000000000000:scheduled-report/never-created";

/// The `scheduleExpression` a report under test is cronned with — well formed, so a row
/// breaks only the grammar it is about.
const SCHEDULE_EXPRESSION: &str = "cron(0 8 1 * ? *)";

/// The name an `UpdateDashboard` that is about another member still has to submit: the
/// member is required on that request, and this value every grammar admits.
const UPDATED_NAME: &str = "Renamed Board";

/// The three grammars a write of this service submits a value of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grammar {
    /// `DashboardName` / `ScheduledReportName`.
    Name,
    /// `Description`, on a dashboard, a widget and a report alike.
    Description,
    /// `WidgetTitle`.
    Title,
}

impl Grammar {
    /// The expression a refusal of this grammar publishes: the modelled `@pattern`
    /// without the `^`/`$` Smithy wraps it in, rendered the way this service renders
    /// every pattern requirement (`tests/all/validation_pattern_rendering.rs` pins that
    /// rendering).
    fn expression(self) -> String {
        let pattern = match self {
            Self::Name => RESOURCE_NAME_PATTERN,
            Self::Description => DESCRIPTION_PATTERN,
            Self::Title => WIDGET_TITLE_PATTERN,
        };
        service_common::validation::unanchored(pattern)
    }
}

/// One row: a value, the grammar that judges it, and whether the service admits it.
struct TextCase {
    label: &'static str,
    grammar: Grammar,
    value: String,
    admitted: bool,
}

/// The table. Every value is inside its member's `@length` bounds (2..50 for a name and a
/// title, 1..200 for a description), so the only constraint in play is the `@pattern`.
fn cases() -> Vec<TextCase> {
    // An interior of single-spaced letters filling the 48 characters the name
    // expression's `{0,48}` admits, between its first and last class.
    let spaced_interior = format!("A{}z", "a ".repeat(24));
    let spaced_title = format!("9{}z", " a".repeat(24));
    vec![
        // `DashboardName` / `ScheduledReportName`:
        // `(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]`.
        TextCase {
            label: "a name of a letter then a digit, at the 2-character minimum",
            grammar: Grammar::Name,
            value: "Q7".to_string(),
            admitted: true,
        },
        TextCase {
            label: "a name ending in an underscore, which its last class lists",
            grammar: Grammar::Name,
            value: "Q_".to_string(),
            admitted: true,
        },
        TextCase {
            label: "a name holding every class its interior lists",
            grammar: Grammar::Name,
            value: "Z 1-2_3".to_string(),
            admitted: true,
        },
        TextCase {
            label: "a name whose interior fills the 48 characters the expression admits",
            grammar: Grammar::Name,
            value: spaced_interior,
            admitted: true,
        },
        TextCase {
            label: "a name holding a line break",
            grammar: Grammar::Name,
            value: "Ab\ncd".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a name ENDING in a line break",
            grammar: Grammar::Name,
            value: "Spend\n".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a name holding a tab",
            grammar: Grammar::Name,
            value: "Ab\tcd".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a name opening with a non-ASCII letter",
            grammar: Grammar::Name,
            value: "Ålesund".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a name opening with a space",
            grammar: Grammar::Name,
            value: " Ab".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a name ending in two spaces",
            grammar: Grammar::Name,
            value: "Ab  ".to_string(),
            admitted: false,
        },
        // `Description`: `(?!.* {2})[ a-zA-Z0-9.,!?;:@#$%&\-_/\\]*`.
        TextCase {
            label: "a description of one listed punctuation character",
            grammar: Grammar::Description,
            value: "#".to_string(),
            admitted: true,
        },
        TextCase {
            label: "a description opening and closing with a single space",
            grammar: Grammar::Description,
            value: " spend ".to_string(),
            admitted: true,
        },
        TextCase {
            label: "a description holding every punctuation its class lists",
            grammar: Grammar::Description,
            value: r"a.b,c!d?e;f:g@h#i$j%k&l-m_n/o\p".to_string(),
            admitted: true,
        },
        TextCase {
            label: "a description holding a line break",
            grammar: Grammar::Description,
            value: "spend\nreport".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a description ENDING in a line break",
            grammar: Grammar::Description,
            value: "spend\n".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a description holding a tilde, just outside its class",
            grammar: Grammar::Description,
            value: "~spend".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a description holding a double quote",
            grammar: Grammar::Description,
            value: "the \"spend\" report".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a description holding a plus sign",
            grammar: Grammar::Description,
            value: "cost+usage".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a description holding a non-ASCII letter",
            grammar: Grammar::Description,
            value: "coût".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a description holding a tab",
            grammar: Grammar::Description,
            value: "cost\tusage".to_string(),
            admitted: false,
        },
        // `WidgetTitle`: `(?!.* {2})[a-zA-Z0-9_-][ a-zA-Z0-9_-]*[a-zA-Z0-9_-]`.
        TextCase {
            label: "a title of a hyphen and an underscore, at the 2-character minimum",
            grammar: Grammar::Title,
            value: "-_".to_string(),
            admitted: true,
        },
        TextCase {
            label: "a title opening with a digit",
            grammar: Grammar::Title,
            value: "9 to 5 spend".to_string(),
            admitted: true,
        },
        TextCase {
            label: "a title of single-spaced words at the 50-character maximum",
            grammar: Grammar::Title,
            value: spaced_title,
            admitted: true,
        },
        TextCase {
            label: "a title holding an at sign, which only the description class lists",
            grammar: Grammar::Title,
            value: "cost@usage".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a title holding a comma",
            grammar: Grammar::Title,
            value: "cost, usage".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a title holding a line break",
            grammar: Grammar::Title,
            value: "cost\nusage".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a title ENDING in a line break",
            grammar: Grammar::Title,
            value: "cost\n".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a title ending in a space",
            grammar: Grammar::Title,
            value: "cost usage ".to_string(),
            admitted: false,
        },
        TextCase {
            label: "a title holding a non-ASCII letter",
            grammar: Grammar::Title,
            value: "coût".to_string(),
            admitted: false,
        },
    ]
}

/// A widget whose title and configs are well formed.
fn valid_widget() -> Value {
    json!({"title": "Spend", "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]})
}

/// A widget carrying `title` and `description`, each as submitted.
fn widget(title: &str, description: Option<&str>) -> Value {
    let mut widget = valid_widget();
    widget["title"] = json!(title);
    if let Some(description) = description {
        widget["description"] = json!(description);
    }
    widget
}

/// The `scheduledReport` a create submits for `name`, pointing at `dashboard_arn`.
fn report_input(name: &str, dashboard_arn: &str, description: Option<&str>) -> Value {
    let mut report = json!({
        "name": name,
        "dashboardArn": dashboard_arn,
        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
        "scheduleConfig": {"scheduleExpression": SCHEDULE_EXPRESSION},
    });
    if let Some(description) = description {
        report["description"] = json!(description);
    }
    report
}

/// One place a refused value is submitted: the operation, the request and the member path
/// the refusal states it at.
struct Site {
    operation: &'static str,
    request: Value,
    path: &'static str,
}

/// Every write that submits a value of `grammar`, carrying `value` and nothing else that
/// breaks a rule.
fn sites(grammar: Grammar, value: &str) -> Vec<Site> {
    match grammar {
        Grammar::Name => vec![
            Site {
                operation: "CreateDashboard",
                request: json!({"name": value, "widgets": [valid_widget()]}),
                path: "name",
            },
            Site {
                operation: "UpdateDashboard",
                request: json!({"arn": UNKNOWN_DASHBOARD_ARN, "name": value}),
                path: "name",
            },
            Site {
                operation: "CreateScheduledReport",
                request: json!({
                    "scheduledReport": report_input(value, UNKNOWN_DASHBOARD_ARN, None)
                }),
                path: "scheduledReport.name",
            },
            Site {
                operation: "UpdateScheduledReport",
                request: json!({"arn": UNKNOWN_REPORT_ARN, "name": value}),
                path: "name",
            },
        ],
        Grammar::Description => vec![
            Site {
                operation: "CreateDashboard",
                request: json!({
                    "name": "Described Board",
                    "description": value,
                    "widgets": [valid_widget()]
                }),
                path: "description",
            },
            Site {
                operation: "CreateDashboard",
                request: json!({
                    "name": "Described Widget",
                    "widgets": [widget("Spend", Some(value))]
                }),
                path: "widgets.1.member.description",
            },
            Site {
                operation: "UpdateDashboard",
                request: json!({
                    "arn": UNKNOWN_DASHBOARD_ARN,
                    "name": UPDATED_NAME,
                    "description": value
                }),
                path: "description",
            },
            Site {
                operation: "UpdateDashboard",
                request: json!({
                    "arn": UNKNOWN_DASHBOARD_ARN,
                    "name": UPDATED_NAME,
                    "widgets": [widget("Spend", Some(value))]
                }),
                path: "widgets.1.member.description",
            },
            Site {
                operation: "CreateScheduledReport",
                request: json!({
                    "scheduledReport": report_input(
                        "Described Report",
                        UNKNOWN_DASHBOARD_ARN,
                        Some(value),
                    )
                }),
                path: "scheduledReport.description",
            },
            Site {
                operation: "UpdateScheduledReport",
                request: json!({"arn": UNKNOWN_REPORT_ARN, "description": value}),
                path: "description",
            },
        ],
        Grammar::Title => vec![
            Site {
                operation: "CreateDashboard",
                request: json!({"name": "Titled Board", "widgets": [widget(value, None)]}),
                path: "widgets.1.member.title",
            },
            Site {
                operation: "UpdateDashboard",
                request: json!({
                    "arn": UNKNOWN_DASHBOARD_ARN,
                    "name": UPDATED_NAME,
                    "widgets": [widget(value, None)]
                }),
                path: "widgets.1.member.title",
            },
        ],
    }
}

/// The one-clause refusal a broken pattern states.
fn expected_message(value: &str, path: &str, expression: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {expression}"
    )
}

/// The `message` of an error body.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A value the expression does not match is refused with HTTP 400 `ValidationException`
/// and the clause quoting that expression — at every member of every write that submits
/// one.
#[tokio::test]
async fn a_value_the_expression_rejects_is_refused_at_every_write() {
    for case in cases().into_iter().filter(|case| !case.admitted) {
        let expression = case.grammar.expression();
        for site in sites(case.grammar, &case.value) {
            let router = router();
            let (status, body) = call(&router, site.operation, &site.request).await;
            assert_eq!(
                status, 400,
                "case {:?} at {} {}: expected HTTP 400, got {status}: {body}",
                case.label, site.operation, site.path
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {:?} at {} {}: expected ValidationException, got {:?}: {body}",
                case.label,
                site.operation,
                site.path,
                error_code(&body)
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                expected_message(&case.value, site.path, &expression),
                "case {:?} at {} {}: expected the clause quoting the published expression, \
                 got {message:?}",
                case.label,
                site.operation,
                site.path
            );
        }
    }
}

/// A value the expression matches is accepted at every member of every write that submits
/// one, and reads back from the stored record.
#[tokio::test]
async fn a_value_the_expression_matches_is_served_at_every_write() {
    let router = router();
    let (source_arn, _widget_id) = source_dashboard(&router, "Grammar Report Source").await;

    for (index, case) in cases()
        .into_iter()
        .filter(|case| case.admitted)
        .enumerate()
    {
        match case.grammar {
            Grammar::Name => accepted_name(&router, &source_arn, index, &case).await,
            Grammar::Description => accepted_description(&router, &source_arn, index, &case).await,
            Grammar::Title => accepted_title(&router, index, &case).await,
        }
    }
}

/// A name the expression admits: created under it on both resources, and renamed to it by
/// both updates.
async fn accepted_name(
    router: &MultiServiceRouter,
    source_arn: &str,
    index: usize,
    case: &TextCase,
) {
    // CreateDashboard, then free the name again for the rename below (a name is unique to
    // the account).
    let created = ok_call(
        router,
        "CreateDashboard",
        &json!({"name": case.value, "widgets": [valid_widget()]}),
    )
    .await;
    let arn = arn_of(&created, case.label, "CreateDashboard");
    let fetched = ok_call(router, "GetDashboard", &json!({"arn": arn})).await;
    assert_eq!(
        fetched["name"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: GetDashboard must report the created name, got {fetched}",
        case.label
    );
    ok_call(router, "DeleteDashboard", &json!({"arn": arn})).await;

    // UpdateDashboard renaming a dashboard to it.
    let scratch = ok_call(
        router,
        "CreateDashboard",
        &json!({"name": format!("Scratch Board {index}"), "widgets": [valid_widget()]}),
    )
    .await;
    let scratch_arn = arn_of(&scratch, case.label, "CreateDashboard");
    ok_call(
        router,
        "UpdateDashboard",
        &json!({"arn": scratch_arn, "name": case.value}),
    )
    .await;
    let renamed = ok_call(router, "GetDashboard", &json!({"arn": scratch_arn})).await;
    assert_eq!(
        renamed["name"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: UpdateDashboard must store the new name, got {renamed}",
        case.label
    );
    ok_call(router, "DeleteDashboard", &json!({"arn": scratch_arn})).await;

    // CreateScheduledReport under the same name, then free it.
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &json!({"scheduledReport": report_input(&case.value, source_arn, None)}),
    )
    .await;
    let report_arn = arn_of(&created, case.label, "CreateScheduledReport");
    let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": report_arn})).await;
    assert_eq!(
        fetched["scheduledReport"]["name"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: GetScheduledReport must report the created name, got {fetched}",
        case.label
    );
    ok_call(router, "DeleteScheduledReport", &json!({"arn": report_arn})).await;

    // UpdateScheduledReport renaming a report to it.
    let scratch = ok_call(
        router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": report_input(
                &format!("Scratch Report {index}"),
                source_arn,
                None,
            )
        }),
    )
    .await;
    let scratch_arn = arn_of(&scratch, case.label, "CreateScheduledReport");
    ok_call(
        router,
        "UpdateScheduledReport",
        &json!({"arn": scratch_arn, "name": case.value}),
    )
    .await;
    let renamed = ok_call(router, "GetScheduledReport", &json!({"arn": scratch_arn})).await;
    assert_eq!(
        renamed["scheduledReport"]["name"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: UpdateScheduledReport must store the new name, got {renamed}",
        case.label
    );
    ok_call(router, "DeleteScheduledReport", &json!({"arn": scratch_arn})).await;
}

/// A description the expression admits: on a dashboard, on its widget and on a report,
/// through the create and through the update.
async fn accepted_description(
    router: &MultiServiceRouter,
    source_arn: &str,
    index: usize,
    case: &TextCase,
) {
    let created = ok_call(
        router,
        "CreateDashboard",
        &json!({
            "name": format!("Described Create {index}"),
            "description": case.value,
            "widgets": [widget("Spend", Some(&case.value))]
        }),
    )
    .await;
    let arn = arn_of(&created, case.label, "CreateDashboard");
    let fetched = ok_call(router, "GetDashboard", &json!({"arn": arn})).await;
    assert_eq!(
        fetched["description"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: GetDashboard must report the created description, got {fetched}",
        case.label
    );
    assert_eq!(
        fetched["widgets"][0]["description"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: GetDashboard must report the created widget description, got {fetched}",
        case.label
    );

    // The same value through the UPDATE, onto a dashboard that carried another one.
    let plain = ok_call(
        router,
        "CreateDashboard",
        &json!({
            "name": format!("Described Update {index}"),
            "description": "before the rewrite",
            "widgets": [widget("Spend", Some("before the rewrite"))]
        }),
    )
    .await;
    let plain_arn = arn_of(&plain, case.label, "CreateDashboard");
    ok_call(
        router,
        "UpdateDashboard",
        &json!({
            "arn": plain_arn,
            "name": format!("Described Rewritten {index}"),
            "description": case.value,
            "widgets": [widget("Spend", Some(&case.value))]
        }),
    )
    .await;
    let updated = ok_call(router, "GetDashboard", &json!({"arn": plain_arn})).await;
    assert_eq!(
        updated["description"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: UpdateDashboard must store the new description, got {updated}",
        case.label
    );
    assert_eq!(
        updated["widgets"][0]["description"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: UpdateDashboard must store the new widget description, got {updated}",
        case.label
    );

    // And on a report, created with it and updated to it.
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": report_input(
                &format!("Report Desc Create {index}"),
                source_arn,
                Some(&case.value),
            )
        }),
    )
    .await;
    let report_arn = arn_of(&created, case.label, "CreateScheduledReport");
    let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": report_arn})).await;
    assert_eq!(
        fetched["scheduledReport"]["description"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: GetScheduledReport must report the created description, got {fetched}",
        case.label
    );

    let plain = ok_call(
        router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": report_input(
                &format!("Report Desc Update {index}"),
                source_arn,
                Some("before the rewrite"),
            )
        }),
    )
    .await;
    let plain_report_arn = arn_of(&plain, case.label, "CreateScheduledReport");
    ok_call(
        router,
        "UpdateScheduledReport",
        &json!({"arn": plain_report_arn, "description": case.value}),
    )
    .await;
    let updated = ok_call(
        router,
        "GetScheduledReport",
        &json!({"arn": plain_report_arn}),
    )
    .await;
    assert_eq!(
        updated["scheduledReport"]["description"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: UpdateScheduledReport must store the new description, got {updated}",
        case.label
    );
}

/// A widget title the expression admits: through the create and through the update.
async fn accepted_title(router: &MultiServiceRouter, index: usize, case: &TextCase) {
    let created = ok_call(
        router,
        "CreateDashboard",
        &json!({
            "name": format!("Titled Create {index}"),
            "widgets": [widget(&case.value, None)]
        }),
    )
    .await;
    let arn = arn_of(&created, case.label, "CreateDashboard");
    let fetched = ok_call(router, "GetDashboard", &json!({"arn": arn})).await;
    assert_eq!(
        fetched["widgets"][0]["title"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: GetDashboard must report the created widget title, got {fetched}",
        case.label
    );

    let plain = ok_call(
        router,
        "CreateDashboard",
        &json!({
            "name": format!("Titled Update {index}"),
            "widgets": [valid_widget()]
        }),
    )
    .await;
    let plain_arn = arn_of(&plain, case.label, "CreateDashboard");
    ok_call(
        router,
        "UpdateDashboard",
        &json!({
            "arn": plain_arn,
            "name": format!("Titled Rewritten {index}"),
            "widgets": [widget(&case.value, None)]
        }),
    )
    .await;
    let updated = ok_call(router, "GetDashboard", &json!({"arn": plain_arn})).await;
    assert_eq!(
        updated["widgets"][0]["title"].as_str(),
        Some(case.value.as_str()),
        "case {:?}: UpdateDashboard must store the new widget title, got {updated}",
        case.label
    );
}

/// The `arn` a write answered.
fn arn_of(body: &Value, label: &str, operation: &str) -> String {
    body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: {operation} must answer an arn: {body}"))
        .to_string()
}

// SPDX-License-Identifier: CC0-1.0

//! The bound a refusal that quotes a submitted `DateTimeValue$value` stays inside, over
//! the wire.
//!
//! The rule under test: a refusal message is a MODELLED member, so a message built by
//! quoting a value the request carried quotes that value within what the message's own
//! fixed text leaves of the maximum the member carrying it declares — and the fixed text
//! (the member path and the rule the caller has to read) is what survives, not the quoted
//! value. `DateTimeValue$value` and every error shape's `message` target the SAME
//! `GenericString`, `@length min 0, max 1024` (source:
//! `smithy_query(query="shape",shape="DateTimeValue,ValidationException,GenericString")`),
//! so one maximal value plus the clause quoting it would otherwise put a message on the
//! wire some two hundred characters past the maximum a client was told could occur.
//!
//! The rows span that class rather than one request: every member of this surface that
//! submits a `DateTimeRange` (a widget query's `timeRange` on both dashboard writes,
//! including the deepest member path the model admits and a second member of the
//! `QueryParameters` union, and a scheduled report's `widgetDateRangeOverride` on both
//! report writes), both bounds of the range, both `DateTimeType` values — each with its
//! own wording and its own fixed length — and value lengths from short through exactly the
//! room the clause leaves, one character past it, and the modelled maximum of 1024. The
//! ordering refusal, which quotes BOTH bounds, is driven over the same lengths.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::MAX_MESSAGE_LENGTH;

use super::dashboard_lifecycle::{call, error_code, graph_config, ok_call, router};
use super::scheduled_report_lifecycle::EXECUTION_ROLE_ARN;

/// The modelled maximum length of a `DateTimeValue$value` (`GenericString`, `@length
/// 0..1024`, source: `smithy_query(query="shape",shape="DateTimeValue,GenericString")`) —
/// the same maximum the member carrying the refusal message declares.
const MAX_DATE_TIME_VALUE_LENGTH: usize = 1024;

/// A well-formed `DashboardArn` and `ScheduledReportArn` nothing is stored under: a
/// malformed range is refused before the resource it names is resolved, so a refused row
/// needs no stored resource.
const UNKNOWN_DASHBOARD_ARN: &str =
    "arn:aws:bcm-dashboards::000000000000:dashboard/never-created-here";
const UNKNOWN_REPORT_ARN: &str =
    "arn:aws:bcm-dashboards::000000000000:scheduled-report/never-created-here";

/// A `DateTimeValue`.
fn point(kind: &str, value: &str) -> Value {
    json!({"type": kind, "value": value})
}

/// A `DateTimeRange` from `start` to `end`.
fn range(start: Value, end: Value) -> Value {
    json!({"startTime": start, "endTime": end})
}

/// A `GenericString` of exactly `length` characters that names no point in time in either
/// form: numbered segments, so a PREFIX of it is recognisable as a prefix of what was
/// submitted and not as text a refusal invented.
fn malformed_value(length: usize) -> String {
    let mut text = String::new();
    let mut segment = 0usize;
    while text.chars().count() < length {
        text.push_str(&format!("not-a-date-{segment:04}-"));
        segment += 1;
    }
    text.chars().take(length).collect()
}

/// An absolute date/time of exactly `length` characters: the same instant throughout, with
/// the RFC 3339 fraction of a second carrying the padding, so every row of the ordering
/// table names the same point in time at a different length.
fn padded_absolute(length: usize) -> String {
    let head = "2025-07-01T00:00:00.";
    let digits = length
        .checked_sub(head.chars().count() + 1)
        .filter(|digits| *digits >= 1)
        .unwrap_or_else(|| panic!("an absolute date/time cannot be padded to {length} characters"));
    format!("{head}{}Z", "0".repeat(digits))
}

/// The form a value of `kind` has to take, as the refusal words it. An independent oracle:
/// the wording the service publishes, written out here rather than imported.
fn requirement(kind: &str) -> &'static str {
    match kind {
        "ABSOLUTE" => {
            "the value of an ABSOLUTE date/time value must be an absolute date, for example \
             '2025-07-01'"
        }
        "RELATIVE" => {
            "the value of a RELATIVE date/time value must be a relative time period in the \
             ISO 8601 duration format, for example '-P3M' for three months ago"
        }
        other => panic!("no DateTimeType named {other:?}"),
    }
}

/// How a refusal words a `DateTimeValue` at `path` whose text is not in the form `kind`
/// names, quoting `quoted` where the submitted value goes.
fn malformed_form_message(path: &str, kind: &str, quoted: &str) -> String {
    format!(
        "{path}.value: '{quoted}' is not a valid {kind} date/time value; {}",
        requirement(kind)
    )
}

/// How a refusal words a range at `path` whose end comes before its start.
fn order_message(path: &str, start: &str, end: &str) -> String {
    format!(
        "{path}: endTime '{end}' is earlier than startTime '{start}'; a date/time range \
         defines a time period with explicit start and end times"
    )
}

/// A valid value of `kind`, for the bound of a range that is not the one under test.
fn well_formed(kind: &str) -> &'static str {
    match kind {
        "ABSOLUTE" => "2025-07-01",
        "RELATIVE" => "-P3M",
        other => panic!("no DateTimeType named {other:?}"),
    }
}

/// A cost-and-usage widget config over `time_range`.
fn cost_and_usage_config(time_range: Value) -> Value {
    json!({
        "queryParameters": {
            "costAndUsage": {
                "metrics": ["UnblendedCost"],
                "timeRange": time_range,
                "granularity": "MONTHLY"
            }
        },
        "displayConfig": {"graph": {"UnblendedCost": {"visualType": "LINE"}}}
    })
}

/// A reservation-coverage widget config over `time_range`: another member of the
/// `QueryParameters` union, which carries a `@required` `timeRange` of its own.
fn reservation_coverage_config(time_range: Value) -> Value {
    json!({
        "queryParameters": {
            "reservationCoverage": {
                "metrics": ["Hour"],
                "timeRange": time_range,
                "granularity": "MONTHLY"
            }
        },
        "displayConfig": {"table": {}}
    })
}

/// The path of the `timeRange` of config `config_position` of widget `widget_position`
/// under the union member `query`, as the framework spells a member path.
fn time_range_path(widget_position: usize, config_position: usize, query: &str) -> String {
    format!(
        "widgets.{widget_position}.member.configs.{config_position}.member.\
         queryParameters.{query}.timeRange"
    )
}

/// One member of this surface that submits a `DateTimeRange`: the label of the row, the
/// operation that submits it, the path a refusal states the range at, and the request
/// carrying a given range.
struct Target {
    label: &'static str,
    operation: &'static str,
    path: String,
    request: Box<dyn Fn(Value) -> Value>,
}

/// Every member that submits a `DateTimeRange`, over both dashboard writes and both
/// scheduled-report writes — including the deepest path a widget query's range sits at
/// (the second config of the second widget) and a second member of the `QueryParameters`
/// union.
fn targets() -> Vec<Target> {
    vec![
        Target {
            label: "CreateDashboard, a cost-and-usage widget query",
            operation: "CreateDashboard",
            path: time_range_path(1, 1, "costAndUsage"),
            request: Box::new(|time_range| {
                json!({
                    "name": "Value Bound Board",
                    "widgets": [{
                        "title": "Spend",
                        "configs": [cost_and_usage_config(time_range)]
                    }]
                })
            }),
        },
        Target {
            label: "CreateDashboard, the second config of the second widget",
            operation: "CreateDashboard",
            path: time_range_path(2, 2, "reservationCoverage"),
            request: Box::new(|time_range| {
                json!({
                    "name": "Deep Value Bound Board",
                    "widgets": [
                        {"title": "First", "configs": [graph_config("Cost", "BAR", "DAILY")]},
                        {
                            "title": "Second",
                            "configs": [
                                graph_config("Cost", "BAR", "DAILY"),
                                reservation_coverage_config(time_range)
                            ]
                        }
                    ]
                })
            }),
        },
        Target {
            label: "UpdateDashboard, a cost-and-usage widget query",
            operation: "UpdateDashboard",
            path: time_range_path(1, 1, "costAndUsage"),
            request: Box::new(|time_range| {
                json!({
                    "arn": UNKNOWN_DASHBOARD_ARN,
                    "name": "Value Bound Board",
                    "widgets": [{
                        "title": "Spend",
                        "configs": [cost_and_usage_config(time_range)]
                    }]
                })
            }),
        },
        Target {
            label: "CreateScheduledReport, the widget date range override",
            operation: "CreateScheduledReport",
            path: "scheduledReport.widgetDateRangeOverride".to_string(),
            request: Box::new(|time_range| {
                json!({
                    "scheduledReport": {
                        "name": "Value Bound Report",
                        "dashboardArn": UNKNOWN_DASHBOARD_ARN,
                        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                        "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
                        "widgetDateRangeOverride": time_range
                    }
                })
            }),
        },
        Target {
            label: "UpdateScheduledReport, the widget date range override",
            operation: "UpdateScheduledReport",
            path: "widgetDateRangeOverride".to_string(),
            request: Box::new(|time_range| {
                json!({
                    "arn": UNKNOWN_REPORT_ARN,
                    "widgetDateRangeOverride": time_range
                })
            }),
        },
    ]
}

/// The refusal `operation` answers `request` with: HTTP 400 `ValidationException`, and its
/// message, which is within the maximum the member carrying it declares.
async fn refusal_message(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} with {body}"
    );
    let code = error_code(&body);
    assert!(
        code.ends_with("ValidationException"),
        "case {case:?}: expected ValidationException from {operation}, got {code:?} with {body}"
    );
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the error body carries no message: {body}"))
        .to_string();
    let length = message.chars().count();
    assert!(
        length <= MAX_MESSAGE_LENGTH,
        "case {case:?}: a message member holds at most {MAX_MESSAGE_LENGTH} characters, got \
         {length}: {message:?}"
    );
    message
}

/// Every member that submits a `DateTimeRange` refuses a malformed bound with a message
/// inside the modelled maximum, however long the submitted value is — quoting the whole
/// value while the clause has room for it, and otherwise a PREFIX of it, with the member
/// path and the rule intact either way.
#[tokio::test]
async fn a_malformed_date_time_value_is_quoted_within_the_message_bound() {
    let router = router();

    for target in targets() {
        for kind in ["ABSOLUTE", "RELATIVE"] {
            for bound in ["startTime", "endTime"] {
                let path = format!("{}.{bound}", target.path);
                // What the clause itself costs, which is the room the submitted value has
                // before the bound takes over.
                let fixed = malformed_form_message(&path, kind, "").chars().count();
                let fitting = MAX_MESSAGE_LENGTH - fixed;
                for length in [
                    12,
                    200,
                    fitting,
                    fitting + 1,
                    MAX_DATE_TIME_VALUE_LENGTH,
                ] {
                    let value = malformed_value(length);
                    let time_range = if bound == "startTime" {
                        range(point(kind, &value), point(kind, well_formed(kind)))
                    } else {
                        range(point(kind, well_formed(kind)), point(kind, &value))
                    };
                    let case = format!(
                        "{}: a {kind} {bound} holding a {length}-character value",
                        target.label
                    );
                    let message = refusal_message(
                        &router,
                        &case,
                        target.operation,
                        &(target.request)(time_range),
                    )
                    .await;

                    if fixed + length <= MAX_MESSAGE_LENGTH {
                        // The clause has room for the whole value, so the refusal quotes
                        // it whole and the message is exactly the published wording.
                        let expected = malformed_form_message(&path, kind, &value);
                        assert_eq!(
                            message, expected,
                            "case {case:?}: expected the refusal to quote the whole value, got \
                             {message:?}"
                        );
                        continue;
                    }

                    // Past that room the value gives way and nothing else does: the
                    // message still opens at the member path and still closes with the
                    // rule, and what it quotes is a prefix of what was submitted.
                    let opening = format!("{path}.value: '");
                    let closing = format!(
                        "' is not a valid {kind} date/time value; {}",
                        requirement(kind)
                    );
                    let quoted = message
                        .strip_prefix(&opening)
                        .and_then(|rest| rest.strip_suffix(&closing))
                        .unwrap_or_else(|| {
                            panic!(
                                "case {case:?}: expected a message naming {opening:?} and \
                                 stating the rule, got {message:?}"
                            )
                        });
                    let head: String = value.chars().take(32).collect();
                    assert!(
                        quoted.starts_with(&head),
                        "case {case:?}: expected the refusal to quote the submitted value from \
                         its start ({head:?}), got {quoted:?}"
                    );
                    assert!(
                        !message.contains(value.as_str()),
                        "case {case:?}: the clause has no room for the whole value, so the \
                         message cannot carry it, got {message:?}"
                    );
                }
            }
        }
    }

    // A bounded refusal is still a refusal: not one of those rows stored anything.
    let listed = ok_call(&router, "ListDashboards", &json!({"maxResults": 100})).await;
    let own: Vec<&Value> = listed["dashboards"]
        .as_array()
        .expect("ListDashboards answers a dashboards list")
        .iter()
        .filter(|item| item["type"].as_str() == Some("CUSTOM"))
        .collect();
    assert!(
        own.is_empty(),
        "case the refused writes: expected no stored dashboard, got {own:?}"
    );
    let reports = ok_call(&router, "ListScheduledReports", &json!({"maxResults": 100})).await;
    let stored = reports["scheduledReports"]
        .as_array()
        .expect("ListScheduledReports answers a scheduledReports list");
    assert!(
        stored.is_empty(),
        "case the refused writes: expected no stored scheduled report, got {stored:?}"
    );
}

/// Whether this service reads `value` as a point in time at all, established through the
/// service itself rather than assumed here: `UpdateScheduledReport` applies the form rule
/// before it resolves the ARN it was given, so a range whose bounds are in the form their
/// `type` names is answered with the not-found of the ARN and one in no form at all with
/// `ValidationException`.
async fn names_a_point_in_time(router: &MultiServiceRouter, value: &str) -> bool {
    let (_, body) = call(
        router,
        "UpdateScheduledReport",
        &json!({
            "arn": UNKNOWN_REPORT_ARN,
            "widgetDateRangeOverride": range(
                point("ABSOLUTE", value),
                point("ABSOLUTE", "2030-01-01"),
            )
        }),
    )
    .await;
    let code = error_code(&body);
    assert!(
        code.ends_with("ValidationException") || code.ends_with("ResourceNotFoundException"),
        "case the probe for a {}-character value: expected the form rule or the not-found \
         answer, got {code:?} with {body}",
        value.chars().count()
    );
    code.ends_with("ResourceNotFoundException")
}

/// The ordering refusal quotes BOTH bounds, so it is held to the same maximum however long
/// they are: a range whose bounds are padded absolute date/times is refused with a message
/// inside the bound that still names its member path and states its rule — whichever of
/// the two rules the padded value breaks.
#[tokio::test]
async fn an_inverted_range_is_quoted_within_the_message_bound() {
    let router = router();
    // Later than every padded value below, so each pair runs backwards.
    let later = "2030-01-01";

    for length in [40usize, 200, MAX_DATE_TIME_VALUE_LENGTH] {
        let padded = padded_absolute(length);
        // Which of the two rules a value of this length breaks follows from whether the
        // service reads it as a point in time; both refusals are held to the same bound.
        let ordered = names_a_point_in_time(&router, &padded).await;

        for target in targets() {
            let case = format!(
                "{}: an inverted range whose endTime is {length} characters long",
                target.label
            );
            let message = refusal_message(
                &router,
                &case,
                target.operation,
                &(target.request)(range(point("ABSOLUTE", later), point("ABSOLUTE", &padded))),
            )
            .await;

            if !ordered {
                // The padded value is in no form at all, so the refusal is the
                // malformed-value one — bounded the same way, at the bound that carried it.
                let opening = format!("{}.endTime.value: '", target.path);
                assert!(
                    message.starts_with(&opening) && message.ends_with(requirement("ABSOLUTE")),
                    "case {case:?}: expected a bounded malformed-value refusal naming \
                     {opening:?} and stating the rule, got {message:?}"
                );
                continue;
            }

            // The ordering rule, stated at the range's own path. The whole wording is
            // asserted while the clause has room for both bounds.
            let expected = order_message(&target.path, later, &padded);
            if expected.chars().count() <= MAX_MESSAGE_LENGTH {
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected the ordering refusal to quote both bounds, got \
                     {message:?}"
                );
                continue;
            }
            let tail = "; a date/time range defines a time period with explicit start and end \
                        times";
            assert!(
                message.starts_with(&format!("{}: endTime '", target.path))
                    && message.ends_with(tail),
                "case {case:?}: expected a bounded ordering refusal naming its path and stating \
                 its rule, got {message:?}"
            );
            // The bound that fits is quoted whole, and the one that does not is quoted
            // from its start.
            let head: String = padded.chars().take(32).collect();
            assert!(
                message.contains(&format!("startTime '{later}'")) && message.contains(&head),
                "case {case:?}: expected both bounds quoted from their start, got {message:?}"
            );
        }
    }
}

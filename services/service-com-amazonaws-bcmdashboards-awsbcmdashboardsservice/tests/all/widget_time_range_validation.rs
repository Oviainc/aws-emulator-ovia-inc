// SPDX-License-Identifier: CC0-1.0

//! What this service accepts as a submitted `DateTimeRange`, and what it refuses, end to
//! end over the wire.
//!
//! Three rules are under test, each applied wherever the member it is about is submitted —
//! a widget's `configs[].queryParameters.<query>.timeRange` on `CreateDashboard` and
//! `UpdateDashboard`, and a scheduled report's `widgetDateRangeOverride` on
//! `CreateScheduledReport` and `UpdateScheduledReport`:
//!
//! 1. a `DateTimeValue` names a point in time in the form its `type` declares — an
//!    `ABSOLUTE` value is an absolute date, a `RELATIVE` one is an ISO 8601 duration
//!    (`smithy_query(query="shape",shape="DateTimeValue")`: "a point in time that can be
//!    specified as either an absolute date (for example, \"2025-07-01\") or a relative time
//!    period using ISO 8601 duration format (for example, \"-P3M\" for three months
//!    ago)"). `value` targets an unconstrained `GenericString`, so nothing at the boundary
//!    can state this;
//! 2. a range whose two bounds are both `ABSOLUTE` runs forwards: its `endTime` is not
//!    earlier than its `startTime`, because a `DateTimeRange` "defines a time period with
//!    explicit start and end times" (`smithy_query(query="shape",shape="DateTimeRange")`);
//! 3. the ids in ONE `widgets` list are distinct: `Widget$id` is "the unique identifier for
//!    the widget" (`smithy_query(query="shape",shape="Widget")`) and a client may supply
//!    it.
//!
//! A request breaking any of them is refused with `ValidationException` (HTTP 400) and
//! stores nothing; a request satisfying them is stored and reads back verbatim.
//!
//! The refusal rows differ along what each rule generalises over: which bound carries the
//! value, which `type` it declares, which shape of malformed text it holds, which member of
//! the `QueryParameters` union carries the query, how deep in the two nested lists the
//! member sits, how far apart the inverted bounds are (months, one day, hours), whether the
//! bounds carry a time of day and a zone offset at all, and which of the four operations
//! submits the range. The acceptance rows span the same members with values each form
//! admits, including a zero-length period, a leap day, every RFC 3339 zone form and a
//! mixed-`type` range no ordering rule applies to.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use serde_json::{json, Value};

use super::dashboard_lifecycle::{call, error_code, graph_config, ok_call, router};
use super::scheduled_report_lifecycle::EXECUTION_ROLE_ARN;

/// A well-formed `DashboardArn` and `ScheduledReportArn` nothing is stored under: a
/// malformed range is refused before the resource it names is resolved, so a refused row
/// needs no stored resource.
const UNKNOWN_DASHBOARD_ARN: &str = "arn:aws:bcm-dashboards::000000000000:dashboard/never-created";
const UNKNOWN_REPORT_ARN: &str =
    "arn:aws:bcm-dashboards::000000000000:scheduled-report/never-created";

/// A 32-hex `WidgetId` (the shape's `^[0-9a-f]{32}$`), `seed` repeated.
fn widget_id(seed: char) -> String {
    std::iter::repeat_n(seed, 32).collect()
}

/// A `DateTimeValue`.
fn point(kind: &str, value: &str) -> Value {
    json!({"type": kind, "value": value})
}

/// A `DateTimeRange` from `start` to `end`.
fn range(start: Value, end: Value) -> Value {
    json!({"startTime": start, "endTime": end})
}

/// A cost-and-usage widget config over `time_range`, displayed as a graph — the same shape
/// `dashboard_lifecycle::graph_config` builds, with the range under test.
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

/// A savings-plans-utilization widget config over `time_range`: another member of the
/// `QueryParameters` union, which carries its own `@required` `timeRange`.
fn savings_plans_utilization_config(time_range: Value) -> Value {
    json!({
        "queryParameters": {
            "savingsPlansUtilization": {
                "timeRange": time_range,
                "granularity": "MONTHLY"
            }
        },
        "displayConfig": {"table": {}}
    })
}

/// One widget whose single config queries `time_range`.
fn widget_over(time_range: Value) -> Value {
    json!({"title": "Spend", "configs": [cost_and_usage_config(time_range)]})
}

/// The path of the `timeRange` of config `config_position` of widget `widget_position`,
/// under the union member `query`, as the framework spells a member path.
fn time_range_path(widget_position: usize, config_position: usize, query: &str) -> String {
    format!(
        "widgets.{widget_position}.member.configs.{config_position}.member.\
         queryParameters.{query}.timeRange"
    )
}

/// How a refusal words a `DateTimeValue` whose text is not an absolute date. An
/// independent oracle: the wording the service publishes, written out here rather than
/// imported.
fn absolute_form_message(path: &str, value: &str) -> String {
    format!(
        "{path}.value: '{value}' is not a valid ABSOLUTE date/time value; the value of an \
         ABSOLUTE date/time value must be an absolute date, for example '2025-07-01'"
    )
}

/// How a refusal words a `DateTimeValue` whose text is not an ISO 8601 duration.
fn relative_form_message(path: &str, value: &str) -> String {
    format!(
        "{path}.value: '{value}' is not a valid RELATIVE date/time value; the value of a \
         RELATIVE date/time value must be a relative time period in the ISO 8601 duration \
         format, for example '-P3M' for three months ago"
    )
}

/// How a refusal words a range whose end comes before its start.
fn order_message(path: &str, start: &str, end: &str) -> String {
    format!(
        "{path}: endTime '{end}' is earlier than startTime '{start}'; a date/time range \
         defines a time period with explicit start and end times"
    )
}

/// How a refusal words a widget id the list already used.
fn duplicate_id_message(widget_position: usize, id: &str) -> String {
    format!(
        "widgets.{widget_position}.member.id: '{id}' is already the id of another widget in \
         this dashboard; the identifier of a widget must be unique within the dashboard"
    )
}

/// One row of the refusal table.
struct RefusalCase {
    label: String,
    operation: &'static str,
    request: Value,
    /// The whole `message` the refusal must carry.
    message: String,
}

/// A `CreateDashboard` row whose single widget queries `time_range`.
fn create_row(label: &str, name: &str, time_range: Value, message: String) -> RefusalCase {
    RefusalCase {
        label: label.to_string(),
        operation: "CreateDashboard",
        request: json!({"name": name, "widgets": [widget_over(time_range)]}),
        message,
    }
}

fn refusal_cases() -> Vec<RefusalCase> {
    let mut cases = Vec::new();

    // Rule 1, a RELATIVE bound: the value has to be an ISO 8601 duration. The rows span
    // prose, an absolute date submitted as a relative one, the empty string, lowercase
    // designators and a duration with trailing text, on both bounds.
    let relative_faults: [(&str, &str, &str); 6] = [
        ("prose", "last quarter", "startTime"),
        ("an absolute date", "2025-07-01", "endTime"),
        ("the empty string", "", "startTime"),
        ("lowercase designators", "-p3m", "startTime"),
        ("a bare number of months", "3M", "endTime"),
        ("a duration with trailing text", "P3M and a half", "startTime"),
    ];
    for (index, (shape, value, bound)) in relative_faults.into_iter().enumerate() {
        let time_range = if bound == "startTime" {
            range(point("RELATIVE", value), point("RELATIVE", "P0D"))
        } else {
            range(point("RELATIVE", "-P3M"), point("RELATIVE", value))
        };
        let path = format!("{}.{bound}", time_range_path(1, 1, "costAndUsage"));
        cases.push(create_row(
            &format!("a RELATIVE {bound} holding {shape}"),
            &format!("Relative Fault {index}"),
            time_range,
            relative_form_message(&path, value),
        ));
    }

    // Rule 1, an ABSOLUTE bound: the value has to name a date. The rows span prose, a
    // calendar date that does not exist, another field order, a month with no day, epoch
    // seconds and an ISO 8601 duration submitted as an absolute value.
    let absolute_faults: [(&str, &str, &str); 6] = [
        ("prose", "not-a-date", "startTime"),
        ("a day the calendar does not have", "2025-02-30", "startTime"),
        ("the day and month the other way round", "01-07-2025", "endTime"),
        ("a month with no day", "2025-07", "startTime"),
        ("epoch seconds", "1767225600", "endTime"),
        ("an ISO 8601 duration", "-P3M", "startTime"),
    ];
    for (index, (shape, value, bound)) in absolute_faults.into_iter().enumerate() {
        let time_range = if bound == "startTime" {
            range(point("ABSOLUTE", value), point("ABSOLUTE", "2025-07-01"))
        } else {
            range(point("ABSOLUTE", "2025-01-01"), point("ABSOLUTE", value))
        };
        let path = format!("{}.{bound}", time_range_path(1, 1, "costAndUsage"));
        cases.push(create_row(
            &format!("an ABSOLUTE {bound} holding {shape}"),
            &format!("Absolute Fault {index}"),
            time_range,
            absolute_form_message(&path, value),
        ));
    }

    // Rule 2: an ABSOLUTE period that runs backwards. The rows span months apart, one day
    // apart, hours apart within one date, and bounds whose ORDER only the zone offsets
    // decide (02:00+02:00 is 00:00Z, which is earlier than 01:00Z, so the pair that reads
    // as inverted character by character is accepted while this one is refused).
    let inverted: [(&str, &str, &str); 4] = [
        ("months apart", "2025-07-01", "2025-01-01"),
        ("one day apart", "2025-07-02", "2025-07-01"),
        (
            "hours apart within one date",
            "2025-07-01T12:00:00Z",
            "2025-07-01T06:00:00Z",
        ),
        (
            "inverted only once the zone offsets are read",
            "2025-07-01T12:00:00+00:00",
            "2025-07-01T06:00:00+02:00",
        ),
    ];
    for (index, (shape, start, end)) in inverted.into_iter().enumerate() {
        let path = time_range_path(1, 1, "costAndUsage");
        cases.push(create_row(
            &format!("an ABSOLUTE range {shape}"),
            &format!("Inverted Range {index}"),
            range(point("ABSOLUTE", start), point("ABSOLUTE", end)),
            order_message(&path, start, end),
        ));
    }

    // The same two rules reach a range however deep it sits and whichever query carries
    // it: the SECOND config of a widget, the SECOND widget of the list, and a query that
    // is not the cost-and-usage one.
    cases.push(RefusalCase {
        label: "a malformed bound in the second config of a widget".to_string(),
        operation: "CreateDashboard",
        request: json!({
            "name": "Second Config",
            "widgets": [{
                "title": "Spend",
                "configs": [
                    cost_and_usage_config(range(
                        point("RELATIVE", "-P3M"),
                        point("RELATIVE", "P0D"),
                    )),
                    cost_and_usage_config(range(
                        point("ABSOLUTE", "yesterday"),
                        point("ABSOLUTE", "2025-07-01"),
                    ))
                ]
            }]
        }),
        message: absolute_form_message(
            &format!("{}.startTime", time_range_path(1, 2, "costAndUsage")),
            "yesterday",
        ),
    });
    cases.push(RefusalCase {
        label: "an inverted range on the second widget of the list".to_string(),
        operation: "CreateDashboard",
        request: json!({
            "name": "Second Widget",
            "widgets": [
                json!({"title": "Spend", "configs": [graph_config("BlendedCost", "BAR", "DAILY")]}),
                widget_over(range(
                    point("ABSOLUTE", "2026-01-01"),
                    point("ABSOLUTE", "2025-12-31"),
                ))
            ]
        }),
        message: order_message(
            &time_range_path(2, 1, "costAndUsage"),
            "2026-01-01",
            "2025-12-31",
        ),
    });
    cases.push(RefusalCase {
        label: "a malformed bound on a savings-plans-utilization query".to_string(),
        operation: "CreateDashboard",
        request: json!({
            "name": "Other Query Member",
            "widgets": [{
                "title": "Coverage",
                "configs": [savings_plans_utilization_config(range(
                    point("RELATIVE", "-P3M"),
                    point("RELATIVE", "the end of the month"),
                ))]
            }]
        }),
        message: relative_form_message(
            &format!(
                "{}.endTime",
                time_range_path(1, 1, "savingsPlansUtilization")
            ),
            "the end of the month",
        ),
    });

    // Rule 3: two widgets of one dashboard may not share an id.
    cases.push(RefusalCase {
        label: "two widgets carrying one client-supplied id".to_string(),
        operation: "CreateDashboard",
        request: json!({
            "name": "Duplicate Widget Ids",
            "widgets": [
                {
                    "id": widget_id('f'),
                    "title": "First",
                    "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                },
                {
                    "id": widget_id('f'),
                    "title": "Second",
                    "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                }
            ]
        }),
        message: duplicate_id_message(2, &widget_id('f')),
    });
    cases.push(RefusalCase {
        label: "the first and third widget of three carrying one id".to_string(),
        operation: "CreateDashboard",
        request: json!({
            "name": "Duplicate Across Three",
            "widgets": [
                {
                    "id": widget_id('a'),
                    "title": "First",
                    "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                },
                {
                    "id": widget_id('b'),
                    "title": "Second",
                    "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                },
                {
                    "id": widget_id('a'),
                    "title": "Third",
                    "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                }
            ]
        }),
        message: duplicate_id_message(3, &widget_id('a')),
    });

    // The same rules on the UPDATE of a dashboard, which submits the same widget list.
    cases.push(RefusalCase {
        label: "an update submitting an inverted range".to_string(),
        operation: "UpdateDashboard",
        request: json!({
            "arn": UNKNOWN_DASHBOARD_ARN,
            "name": "Updated Dashboard",
            "widgets": [widget_over(range(
                point("ABSOLUTE", "2025-07-01T00:00:00Z"),
                point("ABSOLUTE", "2025-06-30"),
            ))]
        }),
        message: order_message(
            &time_range_path(1, 1, "costAndUsage"),
            "2025-07-01T00:00:00Z",
            "2025-06-30",
        ),
    });
    cases.push(RefusalCase {
        label: "an update submitting two widgets with one id".to_string(),
        operation: "UpdateDashboard",
        request: json!({
            "arn": UNKNOWN_DASHBOARD_ARN,
            "name": "Updated Dashboard",
            "widgets": [
                {
                    "id": widget_id('c'),
                    "title": "First",
                    "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                },
                {
                    "id": widget_id('c'),
                    "title": "Second",
                    "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                }
            ]
        }),
        message: duplicate_id_message(2, &widget_id('c')),
    });

    // And on the scheduled-report member that submits a `DateTimeRange` of its own.
    cases.push(RefusalCase {
        label: "a create whose date range override holds prose".to_string(),
        operation: "CreateScheduledReport",
        request: json!({
            "scheduledReport": {
                "name": "Override Report",
                "dashboardArn": UNKNOWN_DASHBOARD_ARN,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
                "widgetDateRangeOverride": range(
                    point("RELATIVE", "since january"),
                    point("RELATIVE", "P0D"),
                )
            }
        }),
        message: relative_form_message(
            "scheduledReport.widgetDateRangeOverride.startTime",
            "since january",
        ),
    });
    cases.push(RefusalCase {
        label: "a create whose date range override runs backwards".to_string(),
        operation: "CreateScheduledReport",
        request: json!({
            "scheduledReport": {
                "name": "Inverted Override Report",
                "dashboardArn": UNKNOWN_DASHBOARD_ARN,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
                "widgetDateRangeOverride": range(
                    point("ABSOLUTE", "2025-07-01"),
                    point("ABSOLUTE", "2025-01-01"),
                )
            }
        }),
        message: order_message(
            "scheduledReport.widgetDateRangeOverride",
            "2025-07-01",
            "2025-01-01",
        ),
    });
    cases.push(RefusalCase {
        label: "an update whose date range override holds a malformed date".to_string(),
        operation: "UpdateScheduledReport",
        request: json!({
            "arn": UNKNOWN_REPORT_ARN,
            "widgetDateRangeOverride": range(
                point("ABSOLUTE", "2025-13-01"),
                point("ABSOLUTE", "2025-12-01"),
            )
        }),
        message: absolute_form_message(
            "widgetDateRangeOverride.startTime",
            "2025-13-01",
        ),
    });

    cases
}

/// Every row is refused with HTTP 400 `ValidationException`, carrying the message its
/// broken rule states.
#[tokio::test]
async fn a_submitted_range_outside_the_rules_is_refused() {
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
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>");
        assert_eq!(
            message, case.message,
            "case {:?}: expected the refusal to state its rule, got {message:?}",
            case.label
        );
    }
}

/// A refused write stores nothing: no dashboard of the account's own is created, no
/// scheduled report is, and a refused update leaves the whole stored record as it was.
#[tokio::test]
async fn a_refused_write_changes_no_state() {
    let router = router();

    for case in refusal_cases() {
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: the row must be refused for this test to mean anything, got \
             {status}: {body}",
            case.label
        );
    }

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

    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "Untouched By Ranges",
            "widgets": [widget_over(range(
                point("RELATIVE", "-P3M"),
                point("RELATIVE", "P0D"),
            ))]
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
            "name": "Untouched By Ranges",
            "widgets": [widget_over(range(
                point("ABSOLUTE", "2025-07-01"),
                point("ABSOLUTE", "2025-01-01"),
            ))]
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a refused update: expected HTTP 400, got {status}: {body}"
    );
    let after = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
    assert_eq!(
        after, before,
        "case a refused update: the whole record must be untouched, got {after} after \
         {before}"
    );
}

/// One row of the acceptance table: a range every rule admits.
struct AcceptedRange {
    label: &'static str,
    time_range: Value,
}

fn accepted_ranges() -> Vec<AcceptedRange> {
    vec![
        AcceptedRange {
            label: "two relative offsets, the later bound the present",
            time_range: range(point("RELATIVE", "-P3M"), point("RELATIVE", "P0D")),
        },
        AcceptedRange {
            label: "two relative offsets a year and a month back",
            time_range: range(point("RELATIVE", "-P1Y"), point("RELATIVE", "-P1M")),
        },
        AcceptedRange {
            label: "relative offsets carrying a time component and an explicit sign",
            time_range: range(point("RELATIVE", "-P1DT12H"), point("RELATIVE", "+PT0S")),
        },
        AcceptedRange {
            label: "two absolute dates months apart",
            time_range: range(point("ABSOLUTE", "2025-01-01"), point("ABSOLUTE", "2025-07-01")),
        },
        AcceptedRange {
            label: "one absolute date on both bounds, a period of no length",
            time_range: range(point("ABSOLUTE", "2025-07-01"), point("ABSOLUTE", "2025-07-01")),
        },
        AcceptedRange {
            label: "a leap day the calendar does have",
            time_range: range(point("ABSOLUTE", "2024-02-29"), point("ABSOLUTE", "2024-03-01")),
        },
        AcceptedRange {
            label: "absolute instants carrying a time of day",
            time_range: range(
                point("ABSOLUTE", "2025-07-01T06:00:00Z"),
                point("ABSOLUTE", "2025-07-01T18:30:00Z"),
            ),
        },
        AcceptedRange {
            label: "absolute instants whose order only the zone offsets establish",
            time_range: range(
                point("ABSOLUTE", "2025-07-01T00:00:00+02:00"),
                point("ABSOLUTE", "2025-07-01T00:00:00Z"),
            ),
        },
        AcceptedRange {
            label: "an absolute start with no zone at all",
            time_range: range(
                point("ABSOLUTE", "2025-07-01T00:00:00"),
                point("ABSOLUTE", "2025-07-02T00:00:00-05:00"),
            ),
        },
        AcceptedRange {
            // No ordering rule applies to a pair whose bounds are not both fixed points.
            label: "an absolute start with a relative end",
            time_range: range(point("ABSOLUTE", "2025-01-01"), point("RELATIVE", "P0D")),
        },
        AcceptedRange {
            label: "a relative start with an absolute end",
            time_range: range(point("RELATIVE", "-P6M"), point("ABSOLUTE", "2025-07-01")),
        },
    ]
}

/// A range each form admits is stored and reads back verbatim through `GetDashboard`, on
/// the create path and on the update path.
#[tokio::test]
async fn a_submitted_range_the_rules_admit_round_trips() {
    let router = router();

    for (index, case) in accepted_ranges().into_iter().enumerate() {
        let name = format!("Accepted Range {index}");
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({"name": name, "widgets": [widget_over(case.time_range.clone())]}),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: CreateDashboard answered no arn: {created}", case.label))
            .to_string();

        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        let stored = &fetched["widgets"][0]["configs"][0]["queryParameters"]["costAndUsage"]
            ["timeRange"];
        assert_eq!(
            stored, &case.time_range,
            "case {:?}: the stored range must be the submitted one, got {stored}",
            case.label
        );

        // The same range on the update path, replacing the widget list.
        ok_call(
            &router,
            "UpdateDashboard",
            &json!({
                "arn": arn,
                "name": name,
                "widgets": [{
                    "title": "Updated Spend",
                    "configs": [savings_plans_utilization_config(case.time_range.clone())]
                }]
            }),
        )
        .await;
        let updated = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        let stored = &updated["widgets"][0]["configs"][0]["queryParameters"]
            ["savingsPlansUtilization"]["timeRange"];
        assert_eq!(
            stored, &case.time_range,
            "case {:?}: the updated range must be the submitted one, got {stored}",
            case.label
        );
    }
}

/// A minted id is distinct from every id the SAME request supplied, including one a
/// client set to the value the minter would have reached next: the rule is that no two
/// widgets of one dashboard share an id, however each id arrived.
#[tokio::test]
async fn a_minted_widget_id_never_repeats_a_supplied_one() {
    // The minter renders a widget id as 32 hex characters of a counter
    // (`WidgetId`'s `^[0-9a-f]{32}$`), so these are the ids it reaches first — the values
    // a client can supply to collide with it.
    for next in 1..=3_u64 {
        let router = router();
        let supplied = format!("{next:032x}");
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": "Colliding Widget Ids",
                "widgets": [
                    {
                        "id": supplied,
                        "title": "Supplied Id",
                        "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                    },
                    {
                        "title": "Minted Id",
                        "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                    }
                ]
            }),
        )
        .await;
        let arn = created["arn"].as_str().expect("an arn").to_string();

        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        let first = fetched["widgets"][0]["id"].as_str().unwrap_or_default();
        let second = fetched["widgets"][1]["id"].as_str().unwrap_or_default();
        assert_eq!(
            first, supplied,
            "case the id {supplied:?} supplied: it must survive, got {fetched}"
        );
        assert_ne!(
            second, supplied,
            "case the id {supplied:?} supplied: the minted id must not repeat it, got \
             {fetched}"
        );
        assert_eq!(
            second.len(),
            32,
            "case the id {supplied:?} supplied: the minted id keeps its modelled form, got \
             {fetched}"
        );
    }
}

/// about repetition inside one list, not about client-supplied ids — and a widget that
/// arrives without one still gets a minted id that collides with neither.
#[tokio::test]
async fn distinct_widget_ids_are_stored_as_submitted() {
    let router = router();

    let first = widget_id('1');
    let second = widget_id('2');
    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "Distinct Widget Ids",
            "widgets": [
                {
                    "id": first,
                    "title": "First",
                    "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                },
                {
                    "id": second,
                    "title": "Second",
                    "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                },
                {
                    "title": "No Id Of Its Own",
                    "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                }
            ]
        }),
    )
    .await;
    let arn = created["arn"].as_str().expect("an arn").to_string();

    let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
    let ids: Vec<String> = fetched["widgets"]
        .as_array()
        .expect("widgets")
        .iter()
        .map(|widget| {
            widget["id"]
                .as_str()
                .unwrap_or_else(|| panic!("every stored widget carries an id: {fetched}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        ids.len(),
        3,
        "case three widgets: all three must be stored, got {ids:?}"
    );
    assert_eq!(
        (ids[0].as_str(), ids[1].as_str()),
        (first.as_str(), second.as_str()),
        "case three widgets: the supplied ids must survive, got {ids:?}"
    );
    let unique: std::collections::BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "case three widgets: the stored ids must be distinct, got {ids:?}"
    );

    // One dashboard's id is not the next one's problem: the same id is free on another
    // dashboard, because the rule is about one widget list.
    let other = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "Another Dashboard",
            "widgets": [{
                "id": first,
                "title": "First",
                "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
            }]
        }),
    )
    .await;
    let other_arn = other["arn"].as_str().expect("an arn").to_string();
    let fetched = ok_call(&router, "GetDashboard", &json!({"arn": other_arn})).await;
    assert_eq!(
        fetched["widgets"][0]["id"].as_str(),
        Some(first.as_str()),
        "case a second dashboard: the id of a widget of another dashboard is free: {fetched}"
    );
}

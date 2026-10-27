// SPDX-License-Identifier: CC0-1.0

//! How this service QUOTES the requirement a `@pattern`-constrained member broke, over
//! the wire.
//!
//! The rule: a boundary refusal states a `@pattern` requirement as the expression a value
//! has to satisfy — the model's pattern text WITHOUT the `^` / `$` the model wraps it in
//! to turn a find into a whole-value match — so a client that lifts the expression out of
//! the message and re-validates locally gets the one the service advertises. Everything
//! else about the refusal is unchanged: HTTP 400, `ValidationException`, the
//! `N validation error(s) detected: ` envelope, the member path (including a list index),
//! the echoed value, and every other requirement class.
//!
//! The suite is a TABLE over the `@pattern`-constrained members of this service's
//! surface, spanning what the rule generalises over: five different patterns on seven
//! different operations, a member read from the request's top level and one nested under
//! a list element, and a pattern whose expression carries a group, an alternation, an
//! escaped metacharacter and a `\u`-escaped character range. The ARN row the issue cited
//! is ONE of those rows.
//!
//! Three neighbouring tests pin the SCOPE of the rule: a `@length` refusal keeps its own
//! wording, a value that breaks both `@length` and `@pattern` still states both clauses
//! under the plural envelope, and a COLLECTION over its `@length` is stated with no value
//! half, since a list has no scalar rendering to quote.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use serde_json::{json, Value};

use super::dashboard_lifecycle::{call, error_code, graph_config, ok_call, router};

/// The `message` of an error body.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The clause a `@pattern` violation states: the value submitted, the member path it was
/// submitted at, and the expression it has to satisfy.
fn pattern_clause(value: &str, path: &str, expression: &str) -> String {
    format!(
        "Value '{value}' at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {expression}"
    )
}

/// The envelope a single-clause refusal carries.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// One row of the table: a request that breaks exactly one member's `@pattern`, and the
/// refusal it must be answered with.
struct PatternCase {
    label: &'static str,
    operation: &'static str,
    request: Value,
    /// The dotted member path the clause states.
    path: &'static str,
    /// The value the clause echoes, which is the one the request submitted there.
    value: &'static str,
    /// The expression the clause must quote: the member's modelled `@pattern` as the
    /// service publishes it, with no `^` / `$`. Written out rather than derived, so the
    /// expected text is the model's and not this service's own rendering.
    expression: &'static str,
}

/// The patterns these rows break, as the model declares them
/// (`smithy_query(query="shape", shape="DashboardArn,ScheduledReportArn,ResourceArn,
/// ClientToken,WidgetId")`), each paired with the expression a refusal publishes — the
/// same text with the leading `^` and trailing `$` gone:
///
/// | shape                | modelled `@pattern`                                                        |
/// |----------------------|----------------------------------------------------------------------------|
/// | `DashboardArn`       | `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*¦[-a-z0-9]+)$`   |
/// | `ScheduledReportArn` | `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*¦[-a-z0-9]+)$` |
/// | `ResourceArn`        | `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:(dashboard¦scheduled-report)/(\*¦[-a-z0-9]+)$` |
/// | `ClientToken`        | `^[\u0021-\u007E]+$`                                                        |
/// | `WidgetId`           | `^[0-9a-f]{32}$`                                                            |
///
/// (`¦` stands for the alternation bar, which a doc comment's table cannot carry.)
const DASHBOARD_ARN_EXPRESSION: &str =
    r"arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)";
const SCHEDULED_REPORT_ARN_EXPRESSION: &str =
    r"arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*|[-a-z0-9]+)";
const RESOURCE_ARN_EXPRESSION: &str =
    r"arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:(dashboard|scheduled-report)/(\*|[-a-z0-9]+)";
const CLIENT_TOKEN_EXPRESSION: &str = r"[\u0021-\u007E]+";
const WIDGET_ID_EXPRESSION: &str = "[0-9a-f]{32}";

/// A well-formed report ARN, so a row that breaks another member's pattern breaks only
/// that one. It names no stored report: the boundary refusal is answered before any
/// handler looks one up.
const WELL_FORMED_REPORT_ARN: &str =
    "arn:aws:bcm-dashboards::000000000000:scheduled-report/never-created";

fn pattern_cases() -> Vec<PatternCase> {
    vec![
        PatternCase {
            label: "an account-id position that is not 12 digits, on a dashboard ARN",
            operation: "GetResourcePolicy",
            request: json!({
                "resourceArn": "arn:aws:bcm-dashboards::123:dashboard/not-a-real-resource"
            }),
            path: "resourceArn",
            value: "arn:aws:bcm-dashboards::123:dashboard/not-a-real-resource",
            expression: DASHBOARD_ARN_EXPRESSION,
        },
        PatternCase {
            label: "an upper-case resource id, which the ARN's resource segment excludes",
            operation: "GetDashboard",
            request: json!({
                "arn": "arn:aws:bcm-dashboards::000000000000:dashboard/NOT-LOWERCASE"
            }),
            path: "arn",
            value: "arn:aws:bcm-dashboards::000000000000:dashboard/NOT-LOWERCASE",
            expression: DASHBOARD_ARN_EXPRESSION,
        },
        PatternCase {
            label: "a dashboard ARN where a scheduled-report ARN is modelled",
            operation: "GetScheduledReport",
            request: json!({
                "arn": "arn:aws:bcm-dashboards::000000000000:dashboard/a-dashboard"
            }),
            path: "arn",
            value: "arn:aws:bcm-dashboards::000000000000:dashboard/a-dashboard",
            expression: SCHEDULED_REPORT_ARN_EXPRESSION,
        },
        PatternCase {
            label: "a resource type the tagging surface's ARN does not admit",
            operation: "ListTagsForResource",
            request: json!({
                "resourceArn": "arn:aws:bcm-dashboards::000000000000:widget/a-widget"
            }),
            path: "resourceArn",
            value: "arn:aws:bcm-dashboards::000000000000:widget/a-widget",
            expression: RESOURCE_ARN_EXPRESSION,
        },
        PatternCase {
            label: "an ARN of another vendor, on an untag request that also names keys",
            operation: "UntagResource",
            request: json!({
                "resourceArn": "arn:aws:s3:::my-bucket-of-dashboards",
                "resourceTagKeys": ["env"]
            }),
            path: "resourceArn",
            value: "arn:aws:s3:::my-bucket-of-dashboards",
            expression: RESOURCE_ARN_EXPRESSION,
        },
        PatternCase {
            label: "a client token carrying a space, outside its printable-ASCII range",
            operation: "ExecuteScheduledReport",
            request: json!({
                "arn": WELL_FORMED_REPORT_ARN,
                "clientToken": "token with a space"
            }),
            path: "clientToken",
            value: "token with a space",
            expression: CLIENT_TOKEN_EXPRESSION,
        },
        PatternCase {
            label: "a widget id of the right length but not hexadecimal, nested in a list",
            operation: "CreateDashboard",
            request: json!({
                "name": "Pattern Rendering",
                "widgets": [
                    {
                        "title": "Spend",
                        "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
                    },
                    {
                        "id": "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
                        "title": "Coverage",
                        "configs": [graph_config("AmortizedCost", "BAR", "DAILY")]
                    }
                ]
            }),
            path: "widgets.2.member.id",
            value: "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            expression: WIDGET_ID_EXPRESSION,
        },
    ]
}

#[tokio::test]
async fn a_pattern_refusal_quotes_the_expression_without_its_anchors() {
    let router = router();

    for case in pattern_cases() {
        let PatternCase {
            label,
            operation,
            request,
            path,
            value,
            expression,
        } = case;

        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 from {operation}, got {status}: {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {label:?}: expected ValidationException from {operation}, got {:?}: {body}",
            error_code(&body)
        );

        let message = error_message(&body);
        assert_eq!(
            message,
            one_error(&pattern_clause(value, path, expression)),
            "case {label:?}: expected the refusal to quote the published expression, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_length_refusal_keeps_its_own_requirement_wording() {
    let router = router();

    // `nextToken` targets `NextPageToken`, length 1..8192 with pattern `^[\S\s]*$` —
    // which admits the empty string — so the empty token breaks the length minimum and
    // nothing else (`smithy_query(query="shape", shape="NextPageToken")`). The clause
    // that states a bound is untouched by the pattern rendering.
    let (status, body) = call(&router, "ListDashboards", &json!({"nextToken": ""})).await;
    assert_eq!(
        status, 400,
        "case an empty nextToken: expected HTTP 400, got {status}: {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message,
        one_error(
            "Value '' at 'nextToken' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
        ),
        "case an empty nextToken: expected the length requirement verbatim, got {message:?}"
    );
}

#[tokio::test]
async fn a_value_breaking_length_and_pattern_states_both_clauses() {
    let router = router();

    // Nine characters: under `DashboardArn`'s 20-character minimum AND outside its
    // pattern. Both clauses are stated, in the order the walk found them, under the
    // plural envelope — only the pattern clause's expression is re-quoted.
    let too_short = "arn:aws:x";
    let (status, body) = call(&router, "GetDashboard", &json!({"arn": too_short})).await;
    assert_eq!(
        status, 400,
        "case a short malformed arn: expected HTTP 400, got {status}: {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message,
        format!(
            "2 validation errors detected: \
             Value '{too_short}' at 'arn' failed to satisfy constraint: \
             Member must have length greater than or equal to 20; {}",
            pattern_clause(too_short, "arn", DASHBOARD_ARN_EXPRESSION)
        ),
        "case a short malformed arn: expected both clauses under the plural envelope, \
         got {message:?}"
    );
}

#[tokio::test]
async fn a_collection_over_its_length_states_no_value_at_all() {
    let router = router();

    // The other rendering this seam owns: a LIST or MAP that broke its `@length` is
    // stated with no value half, because a collection has no scalar rendering to quote.
    // `widgets` targets `WidgetList`, length 0..20
    // (`smithy_query(query="operation", operation="CreateDashboard")`), so 21 widgets is
    // one past the maximum; the clause names the member and the bound and nothing else.
    let widgets: Vec<Value> = (0..21)
        .map(|i| {
            json!({
                "title": format!("Widget-{i}"),
                "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
            })
        })
        .collect();
    let (status, body) = call(
        &router,
        "CreateDashboard",
        &json!({"name": "Too Many Widgets", "widgets": widgets}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case 21 widgets: expected HTTP 400, got {status}: {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message,
        one_error(
            "Value at 'widgets' failed to satisfy constraint: \
             Member must have length less than or equal to 20"
        ),
        "case 21 widgets: expected the collection clause to state no value, got {message:?}"
    );

    // And the bound itself still admits a list AT the maximum, so the refusal is about
    // the size submitted rather than the member.
    let at_bound: Vec<Value> = (0..20)
        .map(|i| {
            json!({
                "title": format!("Widget-{i}"),
                "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
            })
        })
        .collect();
    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({"name": "Exactly Twenty Widgets", "widgets": at_bound}),
    )
    .await;
    assert!(
        created["arn"].as_str().is_some(),
        "case 20 widgets: the maximum must be admitted, got {created}"
    );
}

#[tokio::test]
async fn the_same_members_still_serve_a_value_their_pattern_admits() {
    let router = router();

    // The refusals above are about the VALUE submitted, not a blanket answer: a dashboard
    // created here is read back through `GetDashboard` and `GetResourcePolicy` under the
    // ARN this service minted, which those same `@pattern`-constrained members admit.
    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "Pattern Rendering Round Trip",
            "widgets": [{
                "id": "0123456789abcdef0123456789abcdef",
                "title": "Spend",
                "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
            }]
        }),
    )
    .await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDashboard must answer an arn: {created}"))
        .to_string();

    let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
    assert_eq!(
        fetched["arn"].as_str(),
        Some(arn.as_str()),
        "case the minted arn: GetDashboard must report the dashboard, got {fetched}"
    );
    assert_eq!(
        fetched["widgets"][0]["id"].as_str(),
        Some("0123456789abcdef0123456789abcdef"),
        "case the minted arn: the widget id the request supplied must survive, got {fetched}"
    );

    let policy = ok_call(&router, "GetResourcePolicy", &json!({"resourceArn": arn})).await;
    assert_eq!(
        policy["resourceArn"].as_str(),
        Some(arn.as_str()),
        "case the minted arn: GetResourcePolicy must echo it, got {policy}"
    );
}

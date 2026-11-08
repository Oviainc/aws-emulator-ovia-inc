// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two messages `PutMetricFilter` refuses a metric
//! transformation's DIMENSIONS with: more dimensions than one filter may carry, and a
//! dimension value that is not a selector of a field of the log event.
//!
//! Both are stated by the service as FIXED TEXT for the error class — one sentence that
//! interpolates nothing from the request or from state: no filter name, no log group, no
//! metric name or namespace, no dimension name or value, and not even the maximum itself.
//! So the tables below span what each rule generalises over and every row must answer the
//! same code, the same status and the same sentence:
//!
//! * over the dimension budget — how far past it the put is (one over, several over), which
//!   selector form each dimension value takes, whether the overflow comes from declared
//!   dimensions alone or from declared dimensions BESIDE the system fields the same put
//!   emits as dimensions (which "count toward the total dimension limit"), and whether the
//!   put would create the filter or update one the group already holds;
//! * a dimension value that is no selector — which form the value takes (a bare word, a
//!   number, a `$`-less path, a quoted string, an empty value, a `$` with nothing selected,
//!   a JSON path with an empty segment), how many dimensions the transformation declares,
//!   and where in the map the offending value sits.
//!
//! Each table also carries its own boundary: a put sitting exactly AT the budget, and the
//! same dimension with a selector value, is accepted and readable back through
//! `DescribeMetricFilters` — so neither sentence is returned for any bad parameter, and the
//! two rules keep their own messages.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! message a handler does not actually answer cannot pass.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::Logs;

/// The code CloudWatch Logs answers when a parameter is specified incorrectly. Source:
/// `PutMetricFilter`'s `InvalidParameterException`, errorKind client, httpError 400.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The whole message a put declaring more dimensions than a metric filter may carry is
/// refused with: fixed text for the error class.
const TOO_MANY_DIMENSIONS: &str =
    "Number of dimensions exceeds the maximum number of dimensions supported per metric filter";

/// The whole message a dimension value that is not a field selector is refused with: fixed
/// text for the error class too.
const DIMENSION_VALUE_NOT_A_SELECTOR: &str =
    "Invalid metric transformation: dimension value must be valid selector";

/// The whole message a transformation declaring both `dimensions` and a `defaultValue` is
/// refused with — the operation's OTHER dimension-related bad parameter, asserted here only
/// to show each rule keeps its own sentence.
const DIMENSIONS_AND_DEFAULT_VALUE: &str =
    "Invalid metric transformation: dimensions and default value are mutually exclusive \
     properties";

/// How many dimensions one metric filter may carry, counting the system fields the same put
/// emits as dimensions. Source: `MetricTransformation$dimensions` — "One metric filter can
/// include as many as three dimensions" — with
/// `PutMetricFilter$emitSystemFieldDimensions`'s "These dimensions [...] count toward the
/// total dimension limit for metric filters".
const MAX_DIMENSIONS: usize = 3;

/// The system fields a metric filter may emit as additional dimensions. Source:
/// `PutMetricFilter$emitSystemFieldDimensions` — "Valid values are @aws.account and
/// @aws.region".
const SYSTEM_FIELD_DIMENSIONS: [&str; 2] = ["@aws.account", "@aws.region"];

/// One put that declares more dimensions than the filter may carry.
#[derive(Debug, Clone)]
struct BudgetCase {
    case: &'static str,
    group: &'static str,
    filter: &'static str,
    pattern: &'static str,
    /// The dimensions declared, each value a selector of a field of the log event.
    dimensions: &'static [(&'static str, &'static str)],
    /// The system fields the same put asks to emit as dimensions, which share the budget.
    system_fields: &'static [&'static str],
}

/// The rows over the budget: declared dimensions alone and declared dimensions beside the
/// emitted system fields, one over the limit and several over it, in every selector form.
fn budget_cases() -> Vec<BudgetCase> {
    vec![
        BudgetCase {
            case: "a fourth dimension, all JSON selectors",
            group: "/mf/budget-fourth-json",
            filter: "fourth-dimension",
            pattern: "{ $.status = \"fail\" }",
            dimensions: &[
                ("A", "$.a"),
                ("B", "$.b"),
                ("C", "$.c"),
                ("D", "$.d"),
            ],
            system_fields: &[],
        },
        BudgetCase {
            case: "six dimensions, twice the budget",
            group: "/mf/budget-six",
            filter: "six-dimensions",
            pattern: "{ $.level = \"ERROR\" }",
            dimensions: &[
                ("A", "$.a"),
                ("B", "$.b"),
                ("C", "$.c"),
                ("D", "$.d"),
                ("E", "$.e"),
                ("F", "$.f"),
            ],
            system_fields: &[],
        },
        BudgetCase {
            case: "a fourth dimension in space-delimited and positional selector forms",
            group: "/mf/budget-space-delimited",
            filter: "fourth-space-delimited",
            pattern: "[host, request, bytes, status]",
            dimensions: &[
                ("Host", "$host"),
                ("Request", "$request"),
                ("Position", "$3"),
                ("Status", "$status"),
            ],
            system_fields: &[],
        },
        BudgetCase {
            case: "three dimensions beside one emitted system field",
            group: "/mf/budget-three-and-a-system-field",
            filter: "three-and-one-system-field",
            pattern: "ERROR",
            dimensions: &[("A", "$.a"), ("B", "$.b"), ("C", "$.c")],
            system_fields: &[SYSTEM_FIELD_DIMENSIONS[0]],
        },
        BudgetCase {
            case: "two dimensions beside both emitted system fields",
            group: "/mf/budget-two-and-both-system-fields",
            filter: "two-and-both-system-fields",
            pattern: "%ERROR|WARN%",
            dimensions: &[("Tenant", "$.tenant.id"), ("Shard", "$shard")],
            system_fields: &SYSTEM_FIELD_DIMENSIONS,
        },
        BudgetCase {
            case: "one dimension past the budget both system fields almost fill",
            group: "/mf/budget-four-and-both-system-fields",
            filter: "four-and-both-system-fields",
            pattern: "",
            dimensions: &[
                ("A", "$.a"),
                ("B", "$.b"),
                ("C", "$.c"),
                ("D", "$.d"),
            ],
            system_fields: &SYSTEM_FIELD_DIMENSIONS,
        },
    ]
}

/// One put whose dimension VALUE is not a selector of a field of the log event.
#[derive(Debug, Clone)]
struct SelectorCase {
    case: &'static str,
    filter: &'static str,
    /// The dimensions declared, one of whose values is the offending one.
    dimensions: &'static [(&'static str, &'static str)],
}

/// The rows whose dimension value is no selector: every form a value that selects nothing
/// takes, at every position of a map of one, two and three dimensions.
fn selector_cases() -> Vec<SelectorCase> {
    vec![
        SelectorCase {
            case: "a bare word",
            filter: "literal-word",
            dimensions: &[("Service", "checkout")],
        },
        SelectorCase {
            case: "a number",
            filter: "literal-number",
            dimensions: &[("Shard", "7")],
        },
        SelectorCase {
            case: "a path with no `$` prefix",
            filter: "literal-path",
            dimensions: &[("Service", ".service")],
        },
        SelectorCase {
            case: "a quoted string",
            filter: "literal-quoted",
            dimensions: &[("Service", "\"checkout\"")],
        },
        SelectorCase {
            case: "an empty value",
            filter: "literal-empty",
            dimensions: &[("Service", "")],
        },
        SelectorCase {
            case: "a `$` selecting nothing",
            filter: "literal-bare-selector",
            dimensions: &[("Service", "$")],
        },
        SelectorCase {
            case: "a JSON path with an empty segment",
            filter: "literal-empty-segment",
            dimensions: &[("Service", "$.a..b")],
        },
        SelectorCase {
            case: "a JSON path that ends in a dot",
            filter: "literal-trailing-dot",
            dimensions: &[("Service", "$.service.")],
        },
        SelectorCase {
            case: "one literal after a selector",
            filter: "literal-second-of-two",
            dimensions: &[("Service", "$.service"), ("Region", "us-east-1")],
        },
        SelectorCase {
            case: "one literal between two selectors",
            filter: "literal-middle-of-three",
            dimensions: &[
                ("Service", "$.service"),
                ("Tenant", "acme"),
                ("Shard", "$shard"),
            ],
        },
        SelectorCase {
            case: "one literal before two selectors",
            filter: "literal-first-of-three",
            dimensions: &[
                ("Account", "123456789012"),
                ("Service", "$.service"),
                ("Shard", "$2"),
            ],
        },
    ]
}

/// A `dimensions` map from `(name, value)` pairs.
fn dimensions_of(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect(),
    )
}

/// A `PutMetricFilter` body over one log group, filter name, pattern, dimension map and
/// emitted system-field list.
fn put_body(
    group: &str,
    filter: &str,
    pattern: &str,
    dimensions: &[(&str, &str)],
    system_fields: &[&str],
) -> Value {
    let mut body = json!({
        "logGroupName": group,
        "filterName": filter,
        "filterPattern": pattern,
        "metricTransformations": [{
            "metricName": "Observed",
            "metricNamespace": "Custom/Dimensions",
            "metricValue": "1",
            "dimensions": dimensions_of(dimensions),
        }],
    });
    if !system_fields.is_empty() {
        body["emitSystemFieldDimensions"] = json!(system_fields);
    }
    body
}

/// Create one log group, once per name.
async fn create_group(logs: &Logs, created: &mut BTreeSet<String>, group: &str) {
    if created.insert(group.to_string()) {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
}

/// The filters `DescribeMetricFilters` reports for one log group.
async fn filters_of(logs: &Logs, group: &str) -> Vec<Value> {
    let page = logs
        .ok("DescribeMetricFilters", &json!({"logGroupName": group}))
        .await;
    page.get("metricFilters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeMetricFilters answers metricFilters, got {page}"))
        .clone()
}

/// The one filter the group holds under `name`, or `None`.
async fn one_filter(logs: &Logs, group: &str, name: &str) -> Option<Value> {
    filters_of(logs, group)
        .await
        .into_iter()
        .find(|filter| filter.get("filterName").and_then(Value::as_str) == Some(name))
}

/// Assert one put is refused with `expected` exactly, carrying none of `carries_nothing_of`.
async fn assert_refusal(
    logs: &Logs,
    case: &str,
    label: &str,
    body: &Value,
    expected: &str,
    carries_nothing_of: &[String],
) {
    let (code, message) = logs.refusal("PutMetricFilter", body).await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case {case:?} ({label}): expected {INVALID_PARAMETER}, got {code} {message:?}"
    );
    assert_eq!(
        message, expected,
        "case {case:?} ({label}): expected {expected:?}, got {message:?}"
    );
    for carried in carries_nothing_of {
        if carried.is_empty() {
            continue;
        }
        assert!(
            !message.contains(carried.as_str()),
            "case {case:?} ({label}): the fixed text carries nothing of the request, got \
             {carried:?} in {message:?}"
        );
    }
}

/// Everything of one over-budget put the fixed text must NOT carry — the names and values
/// of the request, and the maximum itself.
fn budget_request_values(case: &BudgetCase) -> Vec<String> {
    let mut values = vec![
        case.group.to_string(),
        case.filter.to_string(),
        MAX_DIMENSIONS.to_string(),
        case.dimensions.len().to_string(),
    ];
    for (name, value) in case.dimensions {
        values.push((*name).to_string());
        values.push((*value).to_string());
    }
    for field in case.system_fields {
        values.push((*field).to_string());
    }
    values
}

/// A put declaring more dimensions than a metric filter may carry — counting the system
/// fields the same put emits as dimensions — is refused with the service's fixed text,
/// whether it would create the filter or update one the group already holds, and the
/// refusal stores nothing.
#[tokio::test]
async fn too_many_dimensions_are_refused_with_the_services_fixed_text() {
    let logs = Logs::new();
    let mut created = BTreeSet::new();

    for case in budget_cases() {
        create_group(&logs, &mut created, case.group).await;
        let refused = put_body(
            case.group,
            case.filter,
            case.pattern,
            case.dimensions,
            case.system_fields,
        );
        let carries_nothing_of = budget_request_values(&case);

        assert_refusal(
            &logs,
            case.case,
            "the create",
            &refused,
            TOO_MANY_DIMENSIONS,
            &carries_nothing_of,
        )
        .await;
        assert!(
            one_filter(&logs, case.group, case.filter).await.is_none(),
            "case {:?}: a refused put persists nothing, got {:?}",
            case.case,
            filters_of(&logs, case.group).await
        );

        // The boundary: the same put trimmed to the budget — the declared dimensions and
        // the emitted system fields together — is accepted and readable back.
        let room = MAX_DIMENSIONS.saturating_sub(case.system_fields.len());
        let accepted = put_body(
            case.group,
            case.filter,
            case.pattern,
            &case.dimensions[..room.min(case.dimensions.len())],
            case.system_fields,
        );
        logs.ok("PutMetricFilter", &accepted).await;
        let stored = one_filter(&logs, case.group, case.filter)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the accepted put is readable back", case.case));

        // The put that would UPDATE the filter the group now holds is refused identically,
        // and leaves the stored filter as it was.
        assert_refusal(
            &logs,
            case.case,
            "the update",
            &refused,
            TOO_MANY_DIMENSIONS,
            &carries_nothing_of,
        )
        .await;
        let after = one_filter(&logs, case.group, case.filter)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the stored filter survives a refusal", case.case));
        assert_eq!(
            after, stored,
            "case {:?}: a refused put leaves the stored filter unchanged",
            case.case
        );
    }
}

/// A dimension VALUE that is not a selector of a field of the log event is refused with the
/// service's fixed text, in every form such a value takes and at every position of the
/// dimension map — while the same dimension with a selector value is accepted.
#[tokio::test]
async fn a_dimension_value_that_is_no_selector_is_refused_with_the_services_fixed_text() {
    let logs = Logs::new();
    const GROUP: &str = "/mf/dimension-value-selector";
    let mut created = BTreeSet::new();
    create_group(&logs, &mut created, GROUP).await;

    for case in selector_cases() {
        let refused = put_body(GROUP, case.filter, "{ $.status = \"fail\" }", case.dimensions, &[]);
        let mut carries_nothing_of = vec![GROUP.to_string(), case.filter.to_string()];
        for (name, value) in case.dimensions {
            carries_nothing_of.push((*name).to_string());
            carries_nothing_of.push((*value).to_string());
        }
        assert_refusal(
            &logs,
            case.case,
            "the create",
            &refused,
            DIMENSION_VALUE_NOT_A_SELECTOR,
            &carries_nothing_of,
        )
        .await;
        assert!(
            one_filter(&logs, GROUP, case.filter).await.is_none(),
            "case {:?}: a refused put persists nothing",
            case.case
        );

        // The boundary: the same dimension names with selector values are accepted, so the
        // refusal is the VALUE and not the dimensions.
        let selectors: Vec<(&str, &str)> = case
            .dimensions
            .iter()
            .map(|(name, _)| (*name, "$.selected"))
            .collect();
        let accepted = put_body(GROUP, case.filter, "{ $.status = \"fail\" }", &selectors, &[]);
        logs.ok("PutMetricFilter", &accepted).await;
        assert!(
            one_filter(&logs, GROUP, case.filter).await.is_some(),
            "case {:?}: the accepted put is readable back",
            case.case
        );
    }
}

/// Each dimension rule keeps its OWN sentence: over the budget, a value that is no
/// selector, and a `defaultValue` declared beside dimensions are three different refusals
/// of the same operation, so a client can still tell them apart.
#[tokio::test]
async fn each_dimension_rule_keeps_its_own_message() {
    let logs = Logs::new();
    const GROUP: &str = "/mf/dimension-rules-apart";
    let mut created = BTreeSet::new();
    create_group(&logs, &mut created, GROUP).await;

    let over_budget = put_body(
        GROUP,
        "apart-over-budget",
        "ERROR",
        &[("A", "$.a"), ("B", "$.b"), ("C", "$.c"), ("D", "$.d")],
        &[],
    );
    let literal = put_body(
        GROUP,
        "apart-literal",
        "ERROR",
        &[("Service", "checkout")],
        &[],
    );
    let mut with_default = put_body(
        GROUP,
        "apart-default-value",
        "ERROR",
        &[("Service", "$.service")],
        &[],
    );
    with_default["metricTransformations"][0]["defaultValue"] = json!(0.0);

    for (case, body, expected) in [
        ("over the dimension budget", over_budget, TOO_MANY_DIMENSIONS),
        (
            "a dimension value that is no selector",
            literal,
            DIMENSION_VALUE_NOT_A_SELECTOR,
        ),
        (
            "a default value beside dimensions",
            with_default,
            DIMENSIONS_AND_DEFAULT_VALUE,
        ),
    ] {
        let (code, message) = logs.refusal("PutMetricFilter", &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

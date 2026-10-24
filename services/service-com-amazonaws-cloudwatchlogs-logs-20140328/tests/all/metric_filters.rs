// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the metric-filter loop: `PutMetricFilter` /
//! `DescribeMetricFilters`, plus the `metricFilterCount` a log group reports.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span the classes the rules generalise over (filter-pattern form,
//! metric-value form, the `StandardUnit` catalogue, the dimension budget, the dimension
//! value's form, the system-field catalogue, the regular-expression quotas, name order,
//! page size, the per-group quota); any single filter an issue happens to mention is one
//! row.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// AWS's documented metric-filter quota, from `PutMetricFilter`: "The maximum number of
/// metric filters that can be associated with a log group is 100."
const MAX_FILTERS_PER_GROUP: usize = 100;

/// The dimensions one metric filter may declare. Source:
/// `MetricTransformation$dimensions` — "One metric filter can include as many as three
/// dimensions" — with `PutMetricFilter$emitSystemFieldDimensions`' system fields counting
/// "toward the total dimension limit for metric filters".
const MAX_DIMENSIONS: usize = 3;

/// The system fields a metric filter may emit as additional dimensions. Source:
/// `PutMetricFilter$emitSystemFieldDimensions` — "Valid values are @aws.account and
/// @aws.region".
const SYSTEM_FIELD_DIMENSIONS: [&str; 2] = ["@aws.account", "@aws.region"];

/// A system field a SUBSCRIPTION filter may emit but a metric filter may not. Source:
/// `PutSubscriptionFilter$emitSystemFields` admits `@source.log`;
/// `PutMetricFilter$emitSystemFieldDimensions` does not list it.
const SUBSCRIPTION_ONLY_SYSTEM_FIELD: &str = "@source.log";

/// The regular expression patterns one filter pattern may carry. Source:
/// `PutMetricFilter` — "there is a quota of two regular expression patterns within a
/// single filter pattern".
const MAX_REGEX_PER_PATTERN: usize = 2;

/// The regular expression patterns one log group's metric filters may carry between them.
/// Source: `PutMetricFilter` — "There is also a quota of five regular expression patterns
/// per log group".
const MAX_REGEX_PER_GROUP: usize = 5;

/// The published bounds of `DescribeMetricFilters$limit`. Source: the `DescribeLimit`
/// shape, `range 1..50`.
const LIMIT_MIN: i64 = 1;
const LIMIT_MAX: i64 = 50;

/// Every unit a metric transformation can assign. Source: the model's `StandardUnit`
/// enum, in the order it declares them
/// (`smithy_query(query="shape",shape="StandardUnit")`).
const STANDARD_UNITS: [&str; 27] = [
    "Seconds",
    "Microseconds",
    "Milliseconds",
    "Bytes",
    "Kilobytes",
    "Megabytes",
    "Gigabytes",
    "Terabytes",
    "Bits",
    "Kilobits",
    "Megabits",
    "Gigabits",
    "Terabits",
    "Percent",
    "Count",
    "Bytes/Second",
    "Kilobytes/Second",
    "Megabytes/Second",
    "Gigabytes/Second",
    "Terabytes/Second",
    "Bits/Second",
    "Kilobits/Second",
    "Megabits/Second",
    "Gigabits/Second",
    "Terabits/Second",
    "Count/Second",
    "None",
];

/// The forms a dimension value may take: a field of the log event, selected the way the
/// filter-pattern syntax selects one. Source: the dimensions documentation
/// `MetricTransformation$dimensions` references — dimensions are published "with metrics
/// from values in JSON or space-delimited log events", so a value is a `$.path` selector
/// of a JSON event or a `$field` / `$N` field of a space-delimited one.
const FIELD_SELECTORS: [&str; 4] = ["$.eventType", "$.user.id", "$server", "$1"];

/// One put row: everything the request names, so the assertions can be derived from it.
#[derive(Debug, Clone)]
struct Row {
    group: &'static str,
    filter: &'static str,
    pattern: &'static str,
    metric_name: &'static str,
    namespace: &'static str,
    metric_value: &'static str,
    default_value: Option<f64>,
    dimensions: &'static [(&'static str, &'static str)],
    unit: Option<&'static str>,
    apply_on_transformed: bool,
    criteria: Option<&'static str>,
    system_fields: Option<&'static [&'static str]>,
}

impl Row {
    /// The one `metricTransformations` entry this row describes: a member the row leaves
    /// out is left out of the request too.
    fn transformation(&self) -> Value {
        let mut transformation = json!({
            "metricName": self.metric_name,
            "metricNamespace": self.namespace,
            "metricValue": self.metric_value,
        });
        if let Some(default_value) = self.default_value {
            transformation["defaultValue"] = json!(default_value);
        }
        if !self.dimensions.is_empty() {
            transformation["dimensions"] = dimensions_of(self.dimensions);
        }
        if let Some(unit) = self.unit {
            transformation["unit"] = json!(unit);
        }
        transformation
    }

    /// The `PutMetricFilter` request body this row describes.
    fn put_body(&self) -> Value {
        let mut body = json!({
            "logGroupName": self.group,
            "filterName": self.filter,
            "filterPattern": self.pattern,
            "metricTransformations": [self.transformation()],
            "applyOnTransformedLogs": self.apply_on_transformed,
        });
        if let Some(criteria) = self.criteria {
            body["fieldSelectionCriteria"] = json!(criteria);
        }
        if let Some(fields) = self.system_fields {
            body["emitSystemFieldDimensions"] = json!(fields);
        }
        body
    }
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

/// The rows the round-trip test spans: one per filter-pattern form the syntax offers (a
/// term, a JSON selector, a space-delimited pattern, a regular expression, and the empty
/// pattern that matches everything), crossed with the metric-value forms (a literal count
/// and an extracted field), `defaultValue` present and absent, no dimensions through the
/// documented maximum, both `applyOnTransformedLogs` values and different
/// `emitSystemFieldDimensions` subsets.
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            group: "mf-group-term",
            filter: "count-errors",
            pattern: "ERROR",
            metric_name: "ErrorCount",
            namespace: "LogMetrics",
            metric_value: "1",
            default_value: Some(0.0),
            dimensions: &[],
            unit: Some("Count"),
            apply_on_transformed: false,
            criteria: None,
            system_fields: None,
        },
        Row {
            group: "mf-group-json",
            filter: "json-errors",
            pattern: "{ $.eventType = \"Error\" }",
            metric_name: "JsonErrors",
            namespace: "Custom/App",
            metric_value: "$.bytes",
            default_value: None,
            dimensions: &[("EventType", "$.eventType")],
            unit: Some("Bytes"),
            apply_on_transformed: true,
            criteria: Some("@aws.region = \"eu-west-2\""),
            system_fields: Some(&[SYSTEM_FIELD_DIMENSIONS[0]]),
        },
        Row {
            group: "mf-group-space",
            filter: "space-delimited",
            pattern: "[ip, user, timestamp, request, status_code = 4*, size]",
            metric_name: "ClientErrorBytes",
            namespace: "Custom/Access",
            metric_value: "$size",
            default_value: None,
            dimensions: &[("Ip", "$ip"), ("Status", "$status_code")],
            unit: None,
            apply_on_transformed: false,
            criteria: None,
            system_fields: None,
        },
        Row {
            group: "mf-group-regex",
            filter: "regex-errors",
            pattern: "%(?i)(error|warn)%",
            metric_name: "RegexMatches",
            namespace: "Custom/Regex",
            metric_value: "1",
            default_value: None,
            dimensions: &[],
            unit: Some("None"),
            apply_on_transformed: true,
            criteria: None,
            system_fields: Some(&SYSTEM_FIELD_DIMENSIONS),
        },
        Row {
            group: "mf-group-empty",
            filter: "match-everything",
            pattern: "",
            metric_name: "AllEvents",
            namespace: "Custom/All",
            metric_value: "1",
            default_value: None,
            // The documented maximum, spanning both selector forms.
            dimensions: &[
                ("Json", FIELD_SELECTORS[0]),
                ("Nested", FIELD_SELECTORS[1]),
                ("Positional", FIELD_SELECTORS[3]),
            ],
            unit: Some("Percent"),
            apply_on_transformed: false,
            criteria: None,
            system_fields: None,
        },
    ]
}

/// Create a log group, which is what a metric filter is associated with.
async fn create_group(logs: &Logs, group: &str) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
}

/// The `metricFilters` list of a `DescribeMetricFilters` page.
fn metric_filters(page: &Value) -> Vec<Value> {
    page.get("metricFilters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeMetricFilters must return metricFilters, got {page}"))
        .clone()
}

/// The `filterName` values of a list of filters, in the order given.
fn names_of(filters: &[Value]) -> Vec<String> {
    filters
        .iter()
        .map(|filter| {
            filter
                .get("filterName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every filter carries a filterName, got {filter}"))
                .to_string()
        })
        .collect()
}

/// The `filterName` values of a page, in the order returned.
fn filter_names(page: &Value) -> Vec<String> {
    names_of(&metric_filters(page))
}

/// The metric filters of one log group, in the order the service returns them.
async fn filters_of(logs: &Logs, group: &str) -> Vec<Value> {
    let page = logs
        .ok(
            "DescribeMetricFilters",
            &json!({"logGroupName": group, "limit": LIMIT_MAX}),
        )
        .await;
    metric_filters(&page)
}

/// The one filter a group holds under `name`.
async fn one_filter(logs: &Logs, group: &str, name: &str) -> Value {
    let found = filters_of(logs, group).await;
    found
        .iter()
        .find(|entry| entry.get("filterName").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| {
            panic!("case {group}/{name}: the filter must be readable back, got {found:?}")
        })
        .clone()
}

/// A member of a returned filter, as a string.
fn member<'a>(filter: &'a Value, name: &str) -> Option<&'a str> {
    filter.get(name).and_then(Value::as_str)
}

/// The one `metricTransformations` entry of a returned filter.
fn transformation_of(filter: &Value) -> Value {
    let transformations = filter
        .get("metricTransformations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a filter carries metricTransformations, got {filter}"));
    assert_eq!(
        transformations.len(),
        1,
        "the modelled MetricTransformations list is length 1..1, got {transformations:?}"
    );
    transformations[0].clone()
}

/// The `metricTransformations` entry a put with no dimensions and no default value names,
/// for a case whose subject is something else.
fn plain_transformation(metric: &str, namespace: &str) -> Value {
    json!({"metricName": metric, "metricNamespace": namespace, "metricValue": "1"})
}

/// A `PutMetricFilter` body for a case whose subject is neither the transformation nor the
/// pattern.
fn put_body(group: &str, filter: &str, pattern: &str) -> Value {
    json!({
        "logGroupName": group,
        "filterName": filter,
        "filterPattern": pattern,
        "metricTransformations": [plain_transformation("Events", "Custom/Suite")],
    })
}

/// Walk every page of one `DescribeMetricFilters` request at `limit`, asserting each page
/// respects the limit, and answer the filter names in the order returned.
async fn walk(logs: &Logs, request: &Value, limit: i64) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..256 {
        let mut body = request.clone();
        body["limit"] = json!(limit);
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let page = logs.ok("DescribeMetricFilters", &body).await;
        let returned = metric_filters(&page);
        assert!(
            returned.len() <= usize::try_from(limit).expect("a positive limit"),
            "case limit {limit} page {page_index}: expected at most {limit} filters, got {}",
            returned.len()
        );
        names.extend(filter_names(&page));
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
    panic!("case limit {limit}: pagination did not terminate, collected {names:?}");
}

/// Every accepted put is readable back through `DescribeMetricFilters` with the members it
/// named, the log group it was associated with, a live creation time, and no member it did
/// not name — over the whole class of patterns, metric values, dimension sets and
/// system-field subsets.
#[tokio::test]
async fn put_is_readable_back_through_describe() {
    let logs = Logs::new();
    let before = now_ms();

    for row in round_trip_rows() {
        create_group(&logs, row.group).await;
        logs.ok("PutMetricFilter", &row.put_body()).await;

        let found = one_filter(&logs, row.group, row.filter).await;
        assert_eq!(
            member(&found, "filterName"),
            Some(row.filter),
            "case {row:?}: filterName must be echoed, got {found}"
        );
        assert_eq!(
            member(&found, "logGroupName"),
            Some(row.group),
            "case {row:?}: the filter belongs to the log group it was put on, got {found}"
        );
        assert_eq!(
            member(&found, "filterPattern"),
            Some(row.pattern),
            "case {row:?}: filterPattern must be echoed, got {found}"
        );
        assert_eq!(
            found.get("applyOnTransformedLogs").and_then(Value::as_bool),
            Some(row.apply_on_transformed),
            "case {row:?}: applyOnTransformedLogs must be echoed, got {found}"
        );
        assert_eq!(
            member(&found, "fieldSelectionCriteria"),
            row.criteria,
            "case {row:?}: fieldSelectionCriteria is reported only when it was specified, got {found}"
        );
        assert_eq!(
            found
                .get("emitSystemFieldDimensions")
                .and_then(Value::as_array)
                .map(|fields| fields
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()),
            row.system_fields
                .map(|fields| fields.iter().map(ToString::to_string).collect::<Vec<_>>()),
            "case {row:?}: emitSystemFieldDimensions is reported only when it was specified, got {found}"
        );
        assert_eq!(
            transformation_of(&found),
            row.transformation(),
            "case {row:?}: the transformation is reported as it was put, got {found}"
        );
        let creation_time = found
            .get("creationTime")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {row:?}: a filter carries a creationTime, got {found}"));
        assert!(
            (before..=now_ms()).contains(&creation_time),
            "case {row:?}: creationTime must be the wall clock of the put, got {creation_time}"
        );
    }
}

/// Every unit the model's `StandardUnit` enum declares round-trips, and a filter put with
/// no unit reports none — the model spells one of the units "None", so an absent member and
/// that unit are different answers.
#[tokio::test]
async fn every_standard_unit_round_trips() {
    let logs = Logs::new();
    let group = "mf-unit-group";
    create_group(&logs, group).await;

    for (index, unit) in STANDARD_UNITS.iter().enumerate() {
        let filter = format!("unit-{index:02}");
        let mut body = put_body(group, &filter, "ERROR");
        body["metricTransformations"][0]["unit"] = json!(unit);
        logs.ok("PutMetricFilter", &body).await;

        let found = one_filter(&logs, group, &filter).await;
        assert_eq!(
            member(&transformation_of(&found), "unit"),
            Some(*unit),
            "case {unit}: the unit must be echoed, got {found}"
        );
    }

    let filter = "no-unit";
    logs.ok("PutMetricFilter", &put_body(group, filter, "ERROR"))
        .await;
    let found = one_filter(&logs, group, filter).await;
    assert!(
        transformation_of(&found).get("unit").is_none(),
        "case no unit: a transformation put without a unit reports none, got {found}"
    );
}

/// A put under a name the group already holds updates that filter in place: it does not add
/// a second filter, it keeps the creation time, and every other member is replaced by what
/// the new put names — including being dropped when the new put omits it.
#[tokio::test]
async fn put_under_one_name_updates_in_place() {
    let logs = Logs::new();
    let group = "mf-updated-group";
    create_group(&logs, group).await;

    let name = "one-name";
    let mut creation_time: Option<i64> = None;
    for row in round_trip_rows() {
        let mut row = row.clone();
        row.group = group;
        row.filter = name;
        logs.ok("PutMetricFilter", &row.put_body()).await;

        let found = filters_of(&logs, group).await;
        assert_eq!(
            names_of(&found),
            vec![name.to_string()],
            "case {row:?}: a put under one name is an update, not a second filter"
        );
        let filter = found.first().expect("the one filter").clone();
        assert_eq!(
            transformation_of(&filter),
            row.transformation(),
            "case {row:?}: an update replaces the transformation, got {filter}"
        );
        assert_eq!(
            member(&filter, "fieldSelectionCriteria"),
            row.criteria,
            "case {row:?}: an update drops fieldSelectionCriteria when the put omits it, got {filter}"
        );
        assert_eq!(
            filter.get("emitSystemFieldDimensions").is_some(),
            row.system_fields.is_some(),
            "case {row:?}: an update drops emitSystemFieldDimensions when the put omits it, got {filter}"
        );

        let now = filter.get("creationTime").and_then(Value::as_i64);
        match creation_time {
            None => creation_time = now,
            Some(first) => assert_eq!(
                now,
                Some(first),
                "case {row:?}: an update keeps the filter's creation time"
            ),
        }
    }
}

/// One dimension case: the transformation and the system fields a put names, and whether
/// the service may accept it.
#[derive(Debug)]
struct DimensionCase {
    case: &'static str,
    dimensions: Vec<(&'static str, &'static str)>,
    default_value: Option<f64>,
    system_fields: Vec<&'static str>,
    accepted: bool,
}

/// The dimension rules, as a table over the class each rule generalises over: the budget
/// shared between declared dimensions and emitted system fields, the mutual exclusion of
/// `dimensions` and `defaultValue`, and a dimension value naming a field of the log event
/// rather than a literal.
fn dimension_cases() -> Vec<DimensionCase> {
    let selector = |index: usize| FIELD_SELECTORS[index];
    let mut cases = vec![
        DimensionCase {
            case: "no dimensions at all",
            dimensions: Vec::new(),
            default_value: None,
            system_fields: Vec::new(),
            accepted: true,
        },
        DimensionCase {
            case: "a default value without dimensions",
            dimensions: Vec::new(),
            default_value: Some(0.0),
            system_fields: Vec::new(),
            accepted: true,
        },
        DimensionCase {
            case: "one dimension on a JSON selector",
            dimensions: vec![("A", selector(0))],
            default_value: None,
            system_fields: Vec::new(),
            accepted: true,
        },
        DimensionCase {
            case: "the documented maximum of three dimensions",
            dimensions: vec![("A", selector(0)), ("B", selector(2)), ("C", selector(3))],
            default_value: None,
            system_fields: Vec::new(),
            accepted: true,
        },
        DimensionCase {
            case: "two dimensions and one emitted system field, at the budget",
            dimensions: vec![("A", selector(0)), ("B", selector(1))],
            default_value: None,
            system_fields: vec![SYSTEM_FIELD_DIMENSIONS[0]],
            accepted: true,
        },
        DimensionCase {
            case: "a fourth dimension",
            dimensions: vec![
                ("A", selector(0)),
                ("B", selector(1)),
                ("C", selector(2)),
                ("D", selector(3)),
            ],
            default_value: None,
            system_fields: Vec::new(),
            accepted: false,
        },
        DimensionCase {
            case: "three dimensions and an emitted system field, one past the budget",
            dimensions: vec![("A", selector(0)), ("B", selector(1)), ("C", selector(2))],
            default_value: None,
            system_fields: vec![SYSTEM_FIELD_DIMENSIONS[1]],
            accepted: false,
        },
        DimensionCase {
            case: "two dimensions and both emitted system fields, one past the budget",
            dimensions: vec![("A", selector(0)), ("B", selector(1))],
            default_value: None,
            system_fields: SYSTEM_FIELD_DIMENSIONS.to_vec(),
            accepted: false,
        },
        DimensionCase {
            case: "a default value beside one dimension",
            dimensions: vec![("A", selector(0))],
            default_value: Some(0.0),
            system_fields: Vec::new(),
            accepted: false,
        },
        DimensionCase {
            case: "a default value beside three dimensions",
            dimensions: vec![("A", selector(0)), ("B", selector(2)), ("C", selector(3))],
            default_value: Some(42.5),
            system_fields: Vec::new(),
            accepted: false,
        },
    ];
    // A dimension value that is not a field of the log event, in every form a literal
    // takes: a word, a number, the empty string, a selector prefix with nothing selected,
    // and a JSON path with an unnamed segment.
    for literal in ["prod", "1", "", "$", "$.", "eventType", "$.user.", "$.user..id"] {
        cases.push(DimensionCase {
            case: "a literal dimension value",
            dimensions: vec![("A", literal)],
            default_value: None,
            system_fields: Vec::new(),
            accepted: false,
        });
        // One literal among valid selectors is still a literal.
        cases.push(DimensionCase {
            case: "a literal dimension value beside two field selectors",
            dimensions: vec![("A", selector(0)), ("B", selector(2)), ("C", literal)],
            default_value: None,
            system_fields: Vec::new(),
            accepted: false,
        });
    }
    // Every documented selector form is accepted on its own.
    for selector in FIELD_SELECTORS {
        cases.push(DimensionCase {
            case: "one dimension on a documented selector form",
            dimensions: vec![("A", selector)],
            default_value: None,
            system_fields: Vec::new(),
            accepted: true,
        });
    }
    cases
}

/// A metric filter's dimensions are held to the rules documented for them, and a refused
/// put persists nothing: the filter it named is not readable back afterwards.
#[tokio::test]
async fn dimension_rules_are_enforced() {
    let logs = Logs::new();
    let group = "mf-dimension-group";
    create_group(&logs, group).await;

    for (index, case) in dimension_cases().iter().enumerate() {
        let label = case.case;
        let filter = format!("dimension-{index:02}");
        let mut transformation = plain_transformation("Dimensioned", "Custom/Dimensions");
        if !case.dimensions.is_empty() {
            transformation["dimensions"] = dimensions_of(&case.dimensions);
        }
        if let Some(default_value) = case.default_value {
            transformation["defaultValue"] = json!(default_value);
        }
        let mut body = put_body(group, &filter, "{ $.eventType = \"Error\" }");
        body["metricTransformations"] = json!([transformation]);
        if !case.system_fields.is_empty() {
            body["emitSystemFieldDimensions"] = json!(case.system_fields);
        }

        if case.accepted {
            logs.ok("PutMetricFilter", &body).await;
            let found = one_filter(&logs, group, &filter).await;
            let reported = transformation_of(&found);
            assert_eq!(
                reported.get("dimensions").cloned(),
                (!case.dimensions.is_empty()).then(|| dimensions_of(&case.dimensions)),
                "case {label} {case:?}: the dimensions are reported as they were put, got {found}"
            );
            assert_eq!(
                reported.get("defaultValue").and_then(Value::as_f64),
                case.default_value,
                "case {label} {case:?}: defaultValue is reported only when it was specified, got {found}"
            );
        } else {
            assert_eq!(
                logs.err("PutMetricFilter", &body).await,
                "InvalidParameterException",
                "case {label} {case:?}: the put must be refused as a bad parameter"
            );
            let stored = names_of(&filters_of(&logs, group).await);
            assert!(
                !stored.contains(&filter),
                "case {label} {case:?}: a refused put persists nothing, got {stored:?}"
            );
        }
    }
}

/// `emitSystemFieldDimensions` is held to the catalogue its own documentation enumerates —
/// including refusing the one system field a SUBSCRIPTION filter may emit and a metric
/// filter may not.
#[tokio::test]
async fn emit_system_field_dimensions_follow_the_catalogue() {
    let logs = Logs::new();
    let group = "mf-system-fields";
    create_group(&logs, group).await;

    for (index, fields) in [
        vec![SYSTEM_FIELD_DIMENSIONS[0]],
        vec![SYSTEM_FIELD_DIMENSIONS[1]],
        SYSTEM_FIELD_DIMENSIONS.to_vec(),
    ]
    .iter()
    .enumerate()
    {
        let filter = format!("system-fields-{index}");
        let mut body = put_body(group, &filter, "ERROR");
        body["emitSystemFieldDimensions"] = json!(fields);
        logs.ok("PutMetricFilter", &body).await;
        let found = one_filter(&logs, group, &filter).await;
        assert_eq!(
            found.get("emitSystemFieldDimensions"),
            Some(&json!(fields)),
            "case {fields:?}: the emitted system fields must be echoed, got {found}"
        );
    }

    for fields in [
        vec![SUBSCRIPTION_ONLY_SYSTEM_FIELD],
        vec![SYSTEM_FIELD_DIMENSIONS[0], SUBSCRIPTION_ONLY_SYSTEM_FIELD],
        vec!["@aws.Account"],
        vec!["accountId"],
    ] {
        let mut body = put_body(group, "rejected-system-field", "ERROR");
        body["emitSystemFieldDimensions"] = json!(fields);
        assert_eq!(
            logs.err("PutMetricFilter", &body).await,
            "InvalidParameterException",
            "case {fields:?}: only the documented system fields may be emitted as dimensions"
        );
    }

    // An explicitly empty list names no system field, so it is the same request as one
    // that omits the member: the filter emits none, and a reader reports none.
    let filter = "system-fields-empty";
    let mut body = put_body(group, filter, "ERROR");
    body["emitSystemFieldDimensions"] = json!([] as [&str; 0]);
    logs.ok("PutMetricFilter", &body).await;
    let found = one_filter(&logs, group, filter).await;
    assert!(
        found.get("emitSystemFieldDimensions").is_none(),
        "case an empty list: a filter emitting no system field reports none, got {found}"
    );
}

/// The regular-expression quotas: a pattern may carry up to two regular expressions, and
/// one log group's metric filters up to five between them — where a put over a filter
/// already stored replaces the expressions it carried rather than adding to them.
#[tokio::test]
async fn regular_expression_quotas_are_enforced() {
    let logs = Logs::new();
    let group = "mf-regex-group";
    create_group(&logs, group).await;

    // One pattern per expression count, from none through one past the per-pattern quota.
    let patterns: [(usize, &str); 4] = [
        (0, "ERROR"),
        (1, "%(?i)error%"),
        (2, "[w1 = %^GET%, w2 = %5[0-9]{2}$%]"),
        (3, "[w1 = %a%, w2 = %b%, w3 = %c%]"),
    ];
    for (count, pattern) in patterns {
        let filter = format!("regex-{count}");
        let body = put_body(group, &filter, pattern);
        if count <= MAX_REGEX_PER_PATTERN {
            logs.ok("PutMetricFilter", &body).await;
            assert_eq!(
                member(&one_filter(&logs, group, &filter).await, "filterPattern"),
                Some(pattern),
                "case {pattern}: a pattern within the quota is stored as it was put"
            );
        } else {
            assert_eq!(
                logs.err("PutMetricFilter", &body).await,
                "InvalidParameterException",
                "case {pattern}: at most {MAX_REGEX_PER_PATTERN} expressions in one pattern"
            );
        }
    }

    // The group now holds 0 + 1 + 2 = 3 expressions. Another two fit; a third pair does
    // not, because it would take the group past five.
    let two = "[w1 = %^GET%, w2 = %5[0-9]{2}$%]";
    logs.ok("PutMetricFilter", &put_body(group, "regex-more", two))
        .await;
    assert_eq!(
        logs.err("PutMetricFilter", &put_body(group, "regex-too-many", two))
            .await,
        "LimitExceededException",
        "a log group's metric filters carry at most {MAX_REGEX_PER_GROUP} expressions"
    );
    // A put over a filter already stored replaces its expressions, so re-putting the same
    // pattern is still accepted at the quota.
    logs.ok("PutMetricFilter", &put_body(group, "regex-more", two))
        .await;
    // At the quota, even one more expression is refused.
    let one = "%(?i)warn%";
    assert_eq!(
        logs.err("PutMetricFilter", &put_body(group, "regex-last", one))
            .await,
        "LimitExceededException",
        "at the quota of {MAX_REGEX_PER_GROUP}, one more expression does not fit"
    );
    // Replacing a filter's two-expression pattern with one that carries none frees them,
    // so the same put is then accepted: the budget is what the group's filters CURRENTLY
    // carry, not what they ever carried.
    logs.ok("PutMetricFilter", &put_body(group, "regex-2", "ERROR"))
        .await;
    logs.ok("PutMetricFilter", &put_body(group, "regex-last", one))
        .await;
    assert_eq!(
        filters_of(&logs, group).await.len(),
        5,
        "every accepted put is stored: three patterns, the extra pair, and the last one"
    );
}

/// A put is refused when the log group it names does not exist, and so is a read of one:
/// a metric filter has nothing to be associated with until the group is created.
#[tokio::test]
async fn an_absent_log_group_is_refused() {
    let logs = Logs::new();

    for group in ["never-created", "deleted-later"] {
        assert_eq!(
            logs.err("PutMetricFilter", &put_body(group, "any", "ERROR"))
                .await,
            "ResourceNotFoundException",
            "case {group}: a filter cannot be associated with a log group that does not exist"
        );
        assert_eq!(
            logs.err("DescribeMetricFilters", &json!({"logGroupName": group}))
                .await,
            "ResourceNotFoundException",
            "case {group}: a log group that does not exist has no filters to list"
        );
    }

    // Deleting the group takes its metric filters with it: the group is gone, so the read
    // is refused rather than reporting a filter associated with nothing.
    let group = "deleted-later";
    create_group(&logs, group).await;
    logs.ok("PutMetricFilter", &put_body(group, "doomed", "ERROR"))
        .await;
    logs.ok("DeleteLogGroup", &json!({"logGroupName": group}))
        .await;
    assert_eq!(
        logs.err("DescribeMetricFilters", &json!({"logGroupName": group}))
            .await,
        "ResourceNotFoundException",
        "a deleted log group's metric filters are gone with it"
    );
    // The account-wide enumeration no longer reports the deleted group's filter either.
    let all = names_of(&metric_filters(
        &logs.ok("DescribeMetricFilters", &json!({})).await,
    ));
    assert!(
        !all.contains(&"doomed".to_string()),
        "a deleted log group's filters leave the account-wide enumeration, got {all:?}"
    );
}

/// The filters are reported ASCII-sorted by filter name whatever order they were put in,
/// `filterNamePrefix` narrows that enumeration, paging walks it exactly once at every page
/// size, and a request that names no log group enumerates the account's filters across
/// every group.
#[tokio::test]
async fn filters_are_sorted_prefix_filtered_and_paged() {
    let logs = Logs::new();
    // Rows differ in the order the names are put in and in where they diverge, so the
    // reported order cannot be the insertion order.
    let cases: [[&str; 3]; 3] = [
        ["b", "a", "c"],
        ["alpha-2", "alpha-10", "alpha-1"],
        ["Z", "a", "A"],
    ];

    for (index, names) in cases.iter().enumerate() {
        let group = format!("mf-sorted-group-{index}");
        create_group(&logs, &group).await;
        for name in names {
            logs.ok("PutMetricFilter", &put_body(&group, name, "")).await;
        }
        let mut expected: Vec<String> = names.iter().map(ToString::to_string).collect();
        expected.sort();

        assert_eq!(
            filter_names(
                &logs
                    .ok("DescribeMetricFilters", &json!({"logGroupName": group}))
                    .await
            ),
            expected,
            "case {names:?}: the results are ASCII-sorted by filter name"
        );

        // Every page size from the published minimum through the maximum walks the
        // enumeration exactly once, in that order.
        for limit in [LIMIT_MIN, 2, LIMIT_MAX] {
            assert_eq!(
                walk(&logs, &json!({"logGroupName": group}), limit).await,
                expected,
                "case {names:?} limit {limit}: paging returns every filter exactly once"
            );
        }

        // A prefix selects the names that start with it, and nothing else.
        let prefix = &expected[0][..1];
        let selected: Vec<String> = expected
            .iter()
            .filter(|name| name.starts_with(prefix))
            .cloned()
            .collect();
        assert_eq!(
            filter_names(
                &logs
                    .ok(
                        "DescribeMetricFilters",
                        &json!({"logGroupName": group, "filterNamePrefix": prefix}),
                    )
                    .await
            ),
            selected,
            "case {names:?} prefix {prefix}: only names with that prefix are listed"
        );
    }

    // A request that names no log group enumerates every group's filters, still sorted by
    // filter name, and pages through them exactly once.
    let mut everything: Vec<String> = cases
        .iter()
        .flat_map(|names| names.iter().map(ToString::to_string))
        .collect();
    everything.sort();
    assert_eq!(
        walk(&logs, &json!({}), LIMIT_MIN).await,
        everything,
        "an account-wide enumeration reports every group's filters, sorted by filter name"
    );
    assert_eq!(
        filter_names(&logs.ok("DescribeMetricFilters", &json!({})).await),
        everything,
        "an account-wide enumeration is sorted by filter name"
    );

    // One past each published bound of `limit` is refused by the modelled `range 1..50`.
    for limit in [LIMIT_MIN - 1, LIMIT_MAX + 1] {
        let (status, body) = logs
            .call("DescribeMetricFilters", &json!({"limit": limit}))
            .await;
        assert_eq!(
            status, 400,
            "case limit {limit}: a limit outside {LIMIT_MIN}..{LIMIT_MAX} is refused, got {status} {body}"
        );
    }

    // A token this operation did not mint, and one minted for a different enumeration, are
    // both refused rather than misread.
    let token = logs
        .ok("DescribeMetricFilters", &json!({"limit": LIMIT_MIN}))
        .await
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a page smaller than the enumeration carries a nextToken")
        .to_string();
    for (case, request) in [
        (
            "a token from another operation",
            json!({"limit": LIMIT_MIN, "nextToken": "not-a-token"}),
        ),
        (
            "a token from another enumeration",
            json!({
                "logGroupName": "mf-sorted-group-0",
                "limit": LIMIT_MIN,
                "nextToken": token,
            }),
        ),
    ] {
        assert_eq!(
            logs.err("DescribeMetricFilters", &request).await,
            "InvalidParameterException",
            "case {case}: a token is only a position within the enumeration that issued it"
        );
    }
}

/// `metricName` and `metricNamespace` select the filters that emit that metric in that
/// namespace, and each documents the other as required: one without the other is refused.
#[tokio::test]
async fn metric_identity_selects_and_must_be_paired() {
    let logs = Logs::new();
    let group = "mf-metric-identity";
    create_group(&logs, group).await;

    // Rows differ in the metric they emit and the namespace they emit it in, so neither
    // member alone places a filter.
    let rows: [(&str, &str, &str); 4] = [
        ("errors-app", "Errors", "Custom/App"),
        ("errors-infra", "Errors", "Custom/Infra"),
        ("latency-app", "Latency", "Custom/App"),
        ("latency-infra", "Latency", "Custom/Infra"),
    ];
    for (filter, metric, namespace) in rows {
        let mut body = put_body(group, filter, "ERROR");
        body["metricTransformations"] = json!([plain_transformation(metric, namespace)]);
        logs.ok("PutMetricFilter", &body).await;
    }

    for (filter, metric, namespace) in rows {
        let page = logs
            .ok(
                "DescribeMetricFilters",
                &json!({"metricName": metric, "metricNamespace": namespace}),
            )
            .await;
        assert_eq!(
            filter_names(&page),
            vec![filter.to_string()],
            "case {metric}/{namespace}: only the filter emitting that metric is listed"
        );
    }

    // A metric no filter emits selects nothing, which is an empty page rather than a
    // refusal.
    assert!(
        filter_names(
            &logs
                .ok(
                    "DescribeMetricFilters",
                    &json!({"metricName": "Errors", "metricNamespace": "Custom/Absent"}),
                )
                .await
        )
        .is_empty(),
        "a metric no filter emits selects nothing"
    );

    for request in [
        json!({"metricName": "Errors"}),
        json!({"metricNamespace": "Custom/App"}),
        json!({"logGroupName": group, "metricName": "Errors"}),
    ] {
        assert_eq!(
            logs.err("DescribeMetricFilters", &request).await,
            "InvalidParameterException",
            "case {request}: metricName and metricNamespace are specified together"
        );
    }
}

/// The per-group quota: a hundred filters associated with one log group, the hundred-and-
/// first refused, a put over a name already stored still accepted because it is an update,
/// and the group's own `metricFilterCount` tracking the filters it holds.
#[tokio::test]
async fn quota_is_a_hundred_filters_per_log_group() {
    let logs = Logs::new();
    let group = "mf-quota-group";
    let other = "mf-quota-other";
    create_group(&logs, group).await;
    create_group(&logs, other).await;

    for index in 0..MAX_FILTERS_PER_GROUP {
        logs.ok(
            "PutMetricFilter",
            &put_body(group, &format!("filter-{index:03}"), "ERROR"),
        )
        .await;
    }
    assert_eq!(
        logs.err(
            "PutMetricFilter",
            &put_body(group, "one-too-many", "ERROR"),
        )
        .await,
        "LimitExceededException",
        "a log group holds at most {MAX_FILTERS_PER_GROUP} metric filters"
    );
    // At the quota, replacing a filter already stored is still accepted.
    logs.ok("PutMetricFilter", &put_body(group, "filter-000", "WARN"))
        .await;
    // The quota is per log group, so another group takes its own.
    logs.ok("PutMetricFilter", &put_body(other, "filter-000", "ERROR"))
        .await;

    // `metricFilterCount` is the number of filters the group holds, which is what the
    // group reports to a client.
    for (name, expected) in [(group, MAX_FILTERS_PER_GROUP), (other, 1)] {
        let described = logs
            .describe_group(name)
            .await
            .unwrap_or_else(|| panic!("case {name}: the log group must exist"));
        assert_eq!(
            described.get("metricFilterCount").and_then(Value::as_i64),
            Some(i64::try_from(expected).expect("a small count")),
            "case {name}: metricFilterCount counts the filters the group holds, got {described}"
        );
    }
}

/// A log group reports no metric filters, and a `metricFilterCount` of zero, until one is
/// put on it — and the dimension budget is a property of the filter, not of the group, so a
/// group whose filter was refused still reports zero.
#[tokio::test]
async fn a_group_without_filters_reports_none() {
    let logs = Logs::new();
    let group = "mf-empty-group";
    create_group(&logs, group).await;

    assert!(
        filters_of(&logs, group).await.is_empty(),
        "a group nothing was put on holds no metric filters"
    );
    let described = logs
        .describe_group(group)
        .await
        .expect("the log group must exist");
    assert_eq!(
        described.get("metricFilterCount").and_then(Value::as_i64),
        Some(0),
        "a group nothing was put on reports metricFilterCount 0, got {described}"
    );

    let mut body = put_body(group, "refused", "ERROR");
    body["metricTransformations"][0]["dimensions"] = dimensions_of(&[
        ("A", FIELD_SELECTORS[0]),
        ("B", FIELD_SELECTORS[1]),
        ("C", FIELD_SELECTORS[2]),
        ("D", FIELD_SELECTORS[3]),
    ]);
    assert_eq!(
        logs.err("PutMetricFilter", &body).await,
        "InvalidParameterException",
        "a put past the dimension budget of {MAX_DIMENSIONS} is refused"
    );
    let described = logs
        .describe_group(group)
        .await
        .expect("the log group must exist");
    assert_eq!(
        described.get("metricFilterCount").and_then(Value::as_i64),
        Some(0),
        "a refused put leaves metricFilterCount where it was, got {described}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the message `PutMetricFilter` refuses a metric transformation that
//! declares BOTH `dimensions` and a `defaultValue` with.
//!
//! The two members are mutually exclusive, and the service states that refusal as fixed
//! text for the error class: one sentence that interpolates nothing from the request or
//! from state — no filter name, no log group, no metric name or namespace, no dimension
//! name or value and no default value. So the table below spans what the rule generalises
//! over — how many dimensions are declared (one through the documented maximum), which
//! selector form each dimension value takes, which default value is named (zero, negative,
//! fractional, large), whether the same put also emits system fields as dimensions, and
//! whether the put CREATES the filter or UPDATES one the group already holds — and every
//! row must answer the same code, the same status and the same sentence.
//!
//! Each row also carries its own boundary: the identical put WITHOUT `defaultValue` is
//! accepted and readable back, and the other `InvalidParameterException` causes of the same
//! operation answer their own messages, so the sentence is keyed to this rule rather than
//! returned for any bad parameter.
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

/// The whole message a metric transformation declaring both `dimensions` and a
/// `defaultValue` is refused with: fixed text for the error class.
const DIMENSIONS_AND_DEFAULT_VALUE: &str =
    "Invalid metric transformation: dimensions and default value are mutually exclusive \
     properties";

/// The system fields a metric filter may emit as additional dimensions, which "count
/// toward the total dimension limit for metric filters". Source:
/// `PutMetricFilter$emitSystemFieldDimensions` — "Valid values are @aws.account and
/// @aws.region".
const SYSTEM_FIELD_DIMENSIONS: [&str; 2] = ["@aws.account", "@aws.region"];

/// One transformation that declares both members, and everything else the put names, so
/// the assertions are derived from the row.
#[derive(Debug, Clone)]
struct Case {
    case: &'static str,
    group: &'static str,
    filter: &'static str,
    metric: &'static str,
    namespace: &'static str,
    pattern: &'static str,
    /// The dimensions declared, each value a field of the log event the filter-pattern
    /// syntax selects (`$.path` in a JSON event, `$field` or `$N` in a space-delimited
    /// one).
    dimensions: &'static [(&'static str, &'static str)],
    default_value: f64,
    /// The system fields the same put asks to emit as dimensions. Declared dimensions and
    /// emitted system fields share one budget of three, so each row stays inside it: the
    /// subject here is the exclusion, not the budget.
    system_fields: &'static [&'static str],
}

/// The rows: the exclusion holds whatever the dimensions, the selector forms, the default
/// value, the emitted system fields and the names involved are.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "one dimension on a JSON selector and a zero default value",
            group: "/mf/exclusion-one-json",
            filter: "dimdef-refused",
            metric: "Refused",
            namespace: "Custom/Exclusion",
            pattern: "{ $.status = \"fail\" }",
            dimensions: &[("Service", "$.service")],
            default_value: 0.0,
            system_fields: &[],
        },
        Case {
            case: "two dimensions and a fractional default value",
            group: "/mf/exclusion-two-dimensions",
            filter: "two-dimensions-and-a-default",
            metric: "Rejected",
            namespace: "Custom/Second",
            pattern: "{ $.level = \"ERROR\" }",
            dimensions: &[("Tenant", "$.tenant.id"), ("Shard", "$.shard")],
            default_value: 42.5,
            system_fields: &[],
        },
        Case {
            case: "the documented maximum of three dimensions and a negative default value",
            group: "/mf/exclusion-three-dimensions",
            filter: "three-dimensions-and-a-default",
            metric: "Declined",
            namespace: "Custom/Third",
            pattern: "[host, request, bytes]",
            dimensions: &[
                ("Tenant", "$.tenant.id"),
                ("Shard", "$shard"),
                ("Position", "$1"),
            ],
            default_value: -1.5,
            system_fields: &[],
        },
        Case {
            case: "a space-delimited field selector and a large default value",
            group: "/mf/exclusion-space-delimited",
            filter: "space-delimited-and-a-default",
            metric: "Spaced",
            namespace: "Custom/Fourth",
            pattern: "[host, request]",
            dimensions: &[("Host", "$host")],
            default_value: 1_000_000_000.0,
            system_fields: &[],
        },
        Case {
            case: "a dimension beside an emitted system field, inside the budget",
            group: "/mf/exclusion-system-field",
            filter: "system-field-and-a-default",
            metric: "Emitted",
            namespace: "Custom/Fifth",
            pattern: "",
            dimensions: &[("Service", "$.service")],
            default_value: 7.0,
            system_fields: &[SYSTEM_FIELD_DIMENSIONS[1]],
        },
        Case {
            case: "two dimensions beside both emitted system fields is over the budget, so \
                   one dimension and both fields sit at it",
            group: "/mf/exclusion-both-system-fields",
            filter: "both-system-fields-and-a-default",
            metric: "Budgeted",
            namespace: "Custom/Sixth",
            pattern: "%ERROR|WARN%",
            dimensions: &[("Service", "$.service")],
            default_value: 0.25,
            system_fields: &SYSTEM_FIELD_DIMENSIONS,
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

/// The `PutMetricFilter` body this row describes, with `defaultValue` named only when
/// `with_default_value` asks for it.
fn put_body(case: &Case, with_default_value: bool) -> Value {
    let mut transformation = json!({
        "metricName": case.metric,
        "metricNamespace": case.namespace,
        "metricValue": "1",
        "dimensions": dimensions_of(case.dimensions),
    });
    if with_default_value {
        transformation["defaultValue"] = json!(case.default_value);
    }
    let mut body = json!({
        "logGroupName": case.group,
        "filterName": case.filter,
        "filterPattern": case.pattern,
        "metricTransformations": [transformation],
    });
    if !case.system_fields.is_empty() {
        body["emitSystemFieldDimensions"] = json!(case.system_fields);
    }
    body
}

/// Everything of the request the fixed text must NOT carry.
fn request_values(case: &Case) -> Vec<String> {
    let mut values = vec![
        case.group.to_string(),
        case.filter.to_string(),
        case.metric.to_string(),
        case.namespace.to_string(),
        case.default_value.to_string(),
    ];
    for (name, value) in case.dimensions {
        values.push((*name).to_string());
        values.push((*value).to_string());
    }
    values
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

/// Assert one put is refused with the error class's fixed text, carrying nothing of the
/// request.
async fn assert_exclusion_refusal(logs: &Logs, case: &Case, label: &str, body: &Value) {
    let (code, message) = logs.refusal("PutMetricFilter", body).await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case {label} ({}): declaring both members is a bad parameter, got {code} {message:?}",
        case.case
    );
    assert_eq!(
        message, DIMENSIONS_AND_DEFAULT_VALUE,
        "case {label} ({}): expected {DIMENSIONS_AND_DEFAULT_VALUE:?}, got {message:?}",
        case.case
    );
    for carried in request_values(case) {
        assert!(
            !message.contains(&carried),
            "case {label} ({}): the fixed text carries nothing of the request, got {carried:?} \
             in {message:?}",
            case.case
        );
    }
}

/// A metric transformation that declares both `dimensions` and a `defaultValue` is refused
/// with the service's fixed text for the error class — on a put that would CREATE the
/// filter and on one that would UPDATE a filter the group already holds — while the
/// identical put without `defaultValue` is accepted and readable back.
#[tokio::test]
async fn dimensions_with_a_default_value_are_refused_with_the_services_fixed_text() {
    let logs = Logs::new();
    let mut created = BTreeSet::new();

    for case in cases() {
        create_group(&logs, &mut created, case.group).await;

        // The put that would create the filter.
        let refused = put_body(&case, true);
        assert_exclusion_refusal(&logs, &case, "the create", &refused).await;
        assert!(
            one_filter(&logs, case.group, case.filter).await.is_none(),
            "case the create ({}): a refused put persists nothing, got {:?}",
            case.case,
            filters_of(&logs, case.group).await
        );

        // The boundary: the same transformation without `defaultValue` is accepted, so the
        // refusal above is the exclusion and not the dimensions.
        let accepted = put_body(&case, false);
        logs.ok("PutMetricFilter", &accepted).await;
        let stored = one_filter(&logs, case.group, case.filter)
            .await
            .unwrap_or_else(|| panic!("case {}: the accepted put is readable back", case.case));

        // The put that would UPDATE the filter the group now holds is refused identically.
        assert_exclusion_refusal(&logs, &case, "the update", &refused).await;
        let after = one_filter(&logs, case.group, case.filter)
            .await
            .unwrap_or_else(|| panic!("case {}: the stored filter survives a refusal", case.case));
        assert_eq!(
            after, stored,
            "case the update ({}): a refused put leaves the stored filter unchanged",
            case.case
        );

        // The accepted filter is the only one the group holds: the refusals stored nothing
        // under another name either.
        let names: Vec<String> = filters_of(&logs, case.group)
            .await
            .iter()
            .filter_map(|filter| {
                filter
                    .get("filterName")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(
            names,
            vec![case.filter.to_string()],
            "case {}: only the accepted filter is associated with the group",
            case.case
        );
    }
}

/// The other side of the exclusion: a `defaultValue` with NO dimensions declared is
/// accepted — including a `dimensions` map that is explicitly empty, which declares none —
/// and the value is reported back as it was put.
#[tokio::test]
async fn a_default_value_without_dimensions_is_accepted() {
    let logs = Logs::new();
    const GROUP: &str = "/mf/exclusion-default-only";
    let mut created = BTreeSet::new();
    create_group(&logs, &mut created, GROUP).await;

    for (index, (form, default_value, empty_map)) in [
        ("the member omitted, zero", 0.0, false),
        ("the member omitted, fractional", 42.5, false),
        ("the member omitted, negative", -1.5, false),
        ("an explicitly empty map, zero", 0.0, true),
        ("an explicitly empty map, large", 1_000_000_000.0, true),
    ]
    .into_iter()
    .enumerate()
    {
        let filter = format!("default-only-{index:02}");
        let mut transformation = json!({
            "metricName": "Defaulted",
            "metricNamespace": "Custom/DefaultOnly",
            "metricValue": "$.bytes",
            "defaultValue": default_value,
        });
        if empty_map {
            transformation["dimensions"] = json!({});
        }
        let body = json!({
            "logGroupName": GROUP,
            "filterName": filter,
            "filterPattern": "{ $.bytes = * }",
            "metricTransformations": [transformation],
        });
        logs.ok("PutMetricFilter", &body).await;

        let stored = one_filter(&logs, GROUP, &filter)
            .await
            .unwrap_or_else(|| panic!("case {form:?}: the accepted put is readable back"));
        let reported = stored
            .get("metricTransformations")
            .and_then(Value::as_array)
            .and_then(|list| list.first().cloned())
            .unwrap_or_else(|| panic!("case {form:?}: a filter carries one transformation"));
        assert_eq!(
            reported.get("defaultValue").and_then(Value::as_f64),
            Some(default_value),
            "case {form:?}: the default value is reported as it was put, got {reported}"
        );
        assert_eq!(
            reported.get("dimensions").and_then(Value::as_object),
            None,
            "case {form:?}: a transformation that declared no dimensions reports none, got \
             {reported}"
        );
    }
}

/// The fixed text belongs to THIS cause: the operation's other bad-parameter causes answer
/// their own messages, so a client can still tell them apart.
#[tokio::test]
async fn other_bad_parameter_causes_keep_their_own_messages() {
    let logs = Logs::new();
    const GROUP: &str = "/mf/exclusion-other-causes";
    let mut created = BTreeSet::new();
    create_group(&logs, &mut created, GROUP).await;

    let transformation = |extra: Value| -> Value {
        let mut base = json!({
            "metricName": "Other",
            "metricNamespace": "Custom/Other",
            "metricValue": "1",
        });
        for (key, value) in extra
            .as_object()
            .expect("a case's extra members are a JSON object")
        {
            base[key.clone()] = value.clone();
        }
        base
    };

    // A dimension value that is a literal rather than a field of the log event, a fourth
    // dimension past the shared budget, and a system field outside the documented
    // catalogue: each a bad parameter of PutMetricFilter, none of them this exclusion.
    let mut cases: Vec<(&str, Value)> = vec![
        (
            "a literal dimension value",
            json!({
                "logGroupName": GROUP,
                "filterName": "other-literal",
                "filterPattern": "ERROR",
                "metricTransformations": [transformation(json!({
                    "dimensions": {"Service": "checkout"},
                }))],
            }),
        ),
        (
            "a fourth dimension",
            json!({
                "logGroupName": GROUP,
                "filterName": "other-fourth-dimension",
                "filterPattern": "ERROR",
                "metricTransformations": [transformation(json!({
                    "dimensions": {
                        "A": "$.a",
                        "B": "$.b",
                        "C": "$.c",
                        "D": "$.d",
                    },
                }))],
            }),
        ),
        (
            "a system field outside the catalogue",
            json!({
                "logGroupName": GROUP,
                "filterName": "other-system-field",
                "filterPattern": "ERROR",
                "metricTransformations": [transformation(json!({}))],
                "emitSystemFieldDimensions": ["@aws.Account"],
            }),
        ),
    ];
    cases.push((
        "an unterminated filter pattern",
        json!({
            "logGroupName": GROUP,
            "filterName": "other-pattern",
            "filterPattern": "{ $.status = \"fail\"",
            "metricTransformations": [transformation(json!({}))],
        }),
    ));

    for (case, body) in cases {
        let (code, message) = logs.refusal("PutMetricFilter", &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        assert_ne!(
            message, DIMENSIONS_AND_DEFAULT_VALUE,
            "case {case:?}: another cause must not answer the exclusion's text, got {message:?}"
        );
    }
}

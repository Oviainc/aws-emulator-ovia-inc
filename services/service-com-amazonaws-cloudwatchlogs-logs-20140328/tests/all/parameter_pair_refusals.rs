// SPDX-License-Identifier: CC0-1.0

//! How this service words a refusal about a PAIR of input parameters, over every pair it
//! checks across two members.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! RULE: a requirement relating TWO input members — these two cannot be combined, these two
//! are meaningful only together, one of these two is required — is no member constraint, so
//! it is not stated as a validation clause but as the service's FIXED sentence for that
//! pair: `<A> and <B> are mutually exclusive parameters.`, `<request> request must contain
//! both <A> and <B>`, `Either <A> or <B> must be provided.` The sentence belongs to the
//! FORM, so it reads the same under every operation reaching it, and nothing of the
//! submitted request — no value, no length, no count — is interpolated into it.
//!
//! The table below spans what the rule generalises over: which of the three forms, which
//! operation and member pair (seven operations, from the log-group selector pair to the
//! lookup table's content sources), which half of a pair is the one supplied, the error
//! shape the operation answers the pair with (its own `InvalidParameterException`, or the
//! `ValidationException` a cross-member check is answered with), and — row by row — the
//! values the request carries, which the sentence must be invariant to.

use serde_json::{json, Value};

use crate::harness::Logs;

/// This service's two refusal shapes for a pair: a cross-member check is answered with
/// `ValidationException`, everything else with the operation's own
/// `InvalidParameterException`.
const VALIDATION: &str = "ValidationException";
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// A `kmsKeyId` that names a KMS key, so an `AssociateKmsKey` row is refused for its
/// parameter pair rather than for the key it names.
const KMS_KEY_ARN: &str = "arn:aws:kms:eu-west-2:123456789012:key/11112222-3333-4444-5555-666677778888";

/// A log group ARN form `logGroupIdentifier` accepts: the `LogGroupIdentifier` pattern
/// (`^[\w#+=/:,.@-]*$`, from `smithy_query(query="shape", shape="LogGroupIdentifier")`)
/// admits no `*`, so this is the form without the trailing `:*`.
const GROUP_ARN: &str = "arn:aws:logs:eu-west-2:123456789012:log-group:pairs";

/// The sentence a pair that cannot be combined is refused with.
fn mutually_exclusive(first: &str, second: &str) -> String {
    format!("{first} and {second} are mutually exclusive parameters.")
}

/// The sentence a pair that is meaningful only together is refused with.
fn must_contain_both(request: &str, first: &str, second: &str) -> String {
    format!("{request} request must contain both {first} and {second}")
}

/// The sentence a pair one of which is required is refused with.
fn either_required(first: &str, second: &str) -> String {
    format!("Either {first} or {second} must be provided.")
}

/// The sentence a KMS-key association naming BOTH of its target members is refused with:
/// this pair is worded about the member that identifies a target on its own.
const RESOURCE_IDENTIFIER_EXCLUSIVE: &str =
    "resourceIdentifier should not be specified with any other log group attributes";

/// One refused request: the operation, the body, the error shape and the whole sentence.
struct Case {
    name: &'static str,
    operation: &'static str,
    body: Value,
    code: &'static str,
    message: String,
    /// The values the request carried, none of which may appear in the sentence.
    sent: Vec<String>,
}

/// Every pair this service refuses, worded through each of the three forms.
fn cases() -> Vec<Case> {
    let selectors_exclusive = mutually_exclusive("LogGroup name", "LogGroup ARN");
    let selector_required = either_required("LogGroupName", "LogGroupArn");
    let kms_exclusive = RESOURCE_IDENTIFIER_EXCLUSIVE.to_string();
    let metric_pair = must_contain_both("Describe Metric Filters", "MetricName", "MetricNamespace");
    vec![
        // The two name filters of a log-group listing: the pair this operation's own
        // `InvalidParameterException` answers.
        Case {
            name: "DescribeLogGroups naming both name filters",
            operation: "DescribeLogGroups",
            body: json!({"logGroupNamePrefix": "Data", "logGroupNamePattern": "Logs"}),
            code: INVALID_PARAMETER,
            message: mutually_exclusive("LogGroup name prefix", "LogGroup name pattern"),
            sent: vec!["Data".to_string(), "Logs".to_string()],
        },
        // The same refusal with other values, of other lengths: the sentence is the form's.
        Case {
            name: "DescribeLogGroups naming both name filters with other values",
            operation: "DescribeLogGroups",
            body: json!({"logGroupNamePrefix": "/aws/lambda/a-much-longer-prefix", "logGroupNamePattern": ""}),
            code: INVALID_PARAMETER,
            message: mutually_exclusive("LogGroup name prefix", "LogGroup name pattern"),
            sent: vec!["/aws/lambda/a-much-longer-prefix".to_string()],
        },
        // The log-group selector pair, on each of the three reads that resolve one.
        Case {
            name: "DescribeLogStreams naming both selectors",
            operation: "DescribeLogStreams",
            body: json!({"logGroupName": "pairs", "logGroupIdentifier": GROUP_ARN}),
            code: VALIDATION,
            message: selectors_exclusive.clone(),
            sent: vec!["pairs".to_string(), GROUP_ARN.to_string()],
        },
        Case {
            name: "GetLogEvents naming both selectors",
            operation: "GetLogEvents",
            body: json!({
                "logGroupName": "pairs",
                "logGroupIdentifier": GROUP_ARN,
                "logStreamName": "stream",
            }),
            code: VALIDATION,
            message: selectors_exclusive.clone(),
            sent: vec!["stream".to_string(), GROUP_ARN.to_string()],
        },
        Case {
            name: "FilterLogEvents naming both selectors",
            operation: "FilterLogEvents",
            body: json!({"logGroupName": "pairs", "logGroupIdentifier": GROUP_ARN}),
            code: VALIDATION,
            message: selectors_exclusive.clone(),
            sent: vec!["pairs".to_string(), GROUP_ARN.to_string()],
        },
        // The same pair, with NEITHER half named: the third form.
        Case {
            name: "DescribeLogStreams naming neither selector",
            operation: "DescribeLogStreams",
            body: json!({"orderBy": "LogStreamName"}),
            code: VALIDATION,
            message: selector_required.clone(),
            sent: vec!["LogStreamName".to_string()],
        },
        Case {
            name: "GetLogEvents naming neither selector",
            operation: "GetLogEvents",
            body: json!({"logStreamName": "stream", "limit": 7}),
            code: VALIDATION,
            message: selector_required.clone(),
            sent: vec!["stream".to_string(), "7".to_string()],
        },
        Case {
            name: "FilterLogEvents naming neither selector",
            operation: "FilterLogEvents",
            body: json!({"filterPattern": "ERROR"}),
            code: VALIDATION,
            message: selector_required.clone(),
            sent: vec!["ERROR".to_string()],
        },
        // The KMS-key target pair, both halves and neither, on both operations that
        // resolve it — refused as the request-validation faults they are, like every other
        // mutually-exclusive selector pair of this service.
        Case {
            name: "AssociateKmsKey naming both targets",
            operation: "AssociateKmsKey",
            body: json!({
                "kmsKeyId": KMS_KEY_ARN,
                "logGroupName": "pairs",
                "resourceIdentifier": GROUP_ARN,
            }),
            code: VALIDATION,
            message: kms_exclusive.clone(),
            sent: vec![KMS_KEY_ARN.to_string(), GROUP_ARN.to_string()],
        },
        Case {
            name: "AssociateKmsKey naming neither target",
            operation: "AssociateKmsKey",
            body: json!({"kmsKeyId": KMS_KEY_ARN}),
            code: VALIDATION,
            message: either_required("logGroupName", "resourceIdentifier"),
            sent: vec![KMS_KEY_ARN.to_string()],
        },
        Case {
            name: "DisassociateKmsKey naming both targets",
            operation: "DisassociateKmsKey",
            body: json!({"logGroupName": "pairs", "resourceIdentifier": GROUP_ARN}),
            code: VALIDATION,
            message: kms_exclusive,
            sent: vec!["pairs".to_string(), GROUP_ARN.to_string()],
        },
        // The metric-identity pair of a metric-filter listing: the form that names the
        // request in the operation's own words, refused for EITHER half supplied alone.
        Case {
            name: "DescribeMetricFilters naming only the metric name",
            operation: "DescribeMetricFilters",
            body: json!({"metricName": "Errors"}),
            code: INVALID_PARAMETER,
            message: metric_pair.clone(),
            sent: vec!["Errors".to_string()],
        },
        Case {
            name: "DescribeMetricFilters naming only the metric namespace",
            operation: "DescribeMetricFilters",
            body: json!({"metricNamespace": "MyApp/Service"}),
            code: INVALID_PARAMETER,
            message: metric_pair.clone(),
            sent: vec!["MyApp/Service".to_string()],
        },
        Case {
            name: "DescribeMetricFilters naming only the metric name beside a log group",
            operation: "DescribeMetricFilters",
            body: json!({"logGroupName": "pairs", "metricName": "Latency"}),
            code: INVALID_PARAMETER,
            message: metric_pair,
            sent: vec!["Latency".to_string()],
        },
        // The lookup table's content sources, both and neither.
        Case {
            name: "CreateLookupTable naming both content sources",
            operation: "CreateLookupTable",
            body: json!({
                "lookupTableName": "pairs",
                "tableBody": "user\nalice\n",
                "queryId": "11112222-3333-4444-5555-666677778888",
            }),
            code: VALIDATION,
            message: mutually_exclusive("tableBody", "queryId"),
            sent: vec!["11112222-3333-4444-5555-666677778888".to_string()],
        },
        Case {
            name: "CreateLookupTable naming neither content source",
            operation: "CreateLookupTable",
            body: json!({"lookupTableName": "pairs"}),
            code: VALIDATION,
            message: either_required("tableBody", "queryId"),
            sent: vec!["pairs".to_string()],
        },
    ]
}

/// Each pair refusal states the service's whole fixed sentence for its form, under the
/// error shape the operation answers that pair with, and carries nothing of the request.
#[tokio::test]
async fn a_pair_refusal_states_the_services_fixed_sentence() {
    let logs = Logs::new();
    // The log group the rows name EXISTS, so no row can be passing on a not-found instead:
    // a pair refusal precedes resolving the resource the request names.
    logs.ok("CreateLogGroup", &json!({"logGroupName": "pairs"}))
        .await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": "pairs", "logStreamName": "stream"}),
    )
    .await;
    for case in cases() {
        let Case {
            name,
            operation,
            body,
            code,
            message,
            sent,
        } = case;
        let (actual_code, actual) = logs.refusal(operation, &body).await;
        assert_eq!(
            actual_code, code,
            "case {name:?}: expected {code}, got {actual_code}"
        );
        assert_eq!(
            actual, message,
            "case {name:?}: expected {message:?}, got {actual:?}"
        );
        for value in sent {
            assert!(
                !actual.contains(&value),
                "case {name:?}: the sentence states nothing of the request, got {actual:?} \
                 carrying {value:?}"
            );
        }
    }
}

/// The exclusive sentence is invariant to the values the two members carry: the same
/// `DescribeLogGroups` pair refusal whatever prefix and pattern a client sends, over values
/// differing in content, length and emptiness.
#[tokio::test]
async fn the_exclusive_sentence_is_invariant_to_the_values_sent() {
    let logs = Logs::new();
    let expected = mutually_exclusive("LogGroup name prefix", "LogGroup name pattern");
    let rows = [
        ("short values", "a", "b"),
        ("a shared value", "same", "same"),
        ("an empty pattern", "/aws/lambda/fn", ""),
        (
            "a long prefix",
            "/aws/lambda/an-extremely-long-function-name-that-goes-on",
            "lambda",
        ),
        ("values that are each other's reverse", "ab.cd", "dc.ba"),
    ];
    for (row, prefix, pattern) in rows {
        let (code, message) = logs
            .refusal(
                "DescribeLogGroups",
                &json!({"logGroupNamePrefix": prefix, "logGroupNamePattern": pattern}),
            )
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {row}: expected {INVALID_PARAMETER}, got {code}"
        );
        assert_eq!(
            message, expected,
            "case {row}: expected {expected:?}, got {message:?}"
        );
    }
    // Naming ONE of the two filters is no pair violation at all: the listing is served.
    for (row, request) in [
        ("only the prefix", json!({"logGroupNamePrefix": "p"})),
        ("only the pattern", json!({"logGroupNamePattern": "p"})),
        ("neither", json!({})),
    ] {
        let page = logs.ok("DescribeLogGroups", &request).await;
        assert!(
            page.get("logGroups").and_then(Value::as_array).is_some(),
            "case {row}: a request naming at most one filter is served, got {page}"
        );
    }
}

/// The paired-parameter sentence is likewise the form's: `DescribeMetricFilters` states it
/// for either half alone, and serves the listing once BOTH halves arrive — which is what
/// makes the refusal a statement about the pair rather than about a member.
#[tokio::test]
async fn the_paired_sentence_holds_for_either_half_and_lifts_when_both_arrive() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": "pairs"}))
        .await;
    logs.ok(
        "PutMetricFilter",
        &json!({
            "logGroupName": "pairs",
            "filterName": "errors",
            "filterPattern": "ERROR",
            "metricTransformations": [{
                "metricName": "Errors",
                "metricNamespace": "MyApp/Service",
                "metricValue": "1",
            }],
        }),
    )
    .await;
    let expected = must_contain_both("Describe Metric Filters", "MetricName", "MetricNamespace");
    let halves = [
        ("the name alone", json!({"metricName": "Errors"})),
        (
            "the namespace alone",
            json!({"metricNamespace": "MyApp/Service"}),
        ),
        ("an empty name alone", json!({"metricName": ""})),
    ];
    for (row, request) in halves {
        let (code, message) = logs.refusal("DescribeMetricFilters", &request).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {row}: expected {INVALID_PARAMETER}, got {code}"
        );
        assert_eq!(
            message, expected,
            "case {row}: expected {expected:?}, got {message:?}"
        );
    }
    let page = logs
        .ok(
            "DescribeMetricFilters",
            &json!({"metricName": "Errors", "metricNamespace": "MyApp/Service"}),
        )
        .await;
    let names: Vec<&str> = page
        .get("metricFilters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case both halves: the listing is served, got {page}"))
        .iter()
        .filter_map(|filter| filter.get("filterName").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names,
        vec!["errors"],
        "case both halves: expected the stored filter, got {page}"
    );
}

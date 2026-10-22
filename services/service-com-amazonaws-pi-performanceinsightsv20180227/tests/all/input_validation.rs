// SPDX-License-Identifier: CC0-1.0

//! How Performance Insights words the two classes of bad request, over the wire.
//!
//! RULE 1 — a violation of a constraint the MODEL declares (`@required`,
//! `@length`, `@range`, `@pattern`, an enum value set) is found by the validation
//! stage of the request walk, before any handler runs, so none of the three
//! errors the pi model declares on its operations is reachable from it: the
//! answer is HTTP 400 with the wire error code `ValidationException` and the
//! framework template `Validation error(s) detected: Value at '<member>' failed
//! to satisfy constraints.` — the member is named, the offending value is NOT
//! echoed, and the Smithy constraint that rejected it is NOT quoted.
//!
//! RULE 2 — a cross-member ordering rule is not a single-member constraint, so
//! the validation stage passes it and the HANDLER refuses it, with the modelled
//! `InvalidArgumentException` (HTTP 400) and ONE service-wide sentence that names
//! no member, no operation and nothing from the request.
//!
//! Both rules are asserted over a table spanning constraint kinds, operations,
//! member types (integer, string, list), and both bounds of every published
//! range — not over one recorded request.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator};

/// 2026-01-01T00:00:00Z. `awsJson1_1` carries an `ISOTimestamp` as epoch
/// seconds.
const T0: f64 = 1_767_225_600.0;

/// A pattern- and length-valid `IdentifierString` (`^[a-zA-Z0-9-]+$`, 0..256).
const IDENTIFIER: &str = "db-ABCDEFG1234567";

/// The template the validation stage states for one violated member.
fn expected_validation_message(member_path: &str) -> String {
    format!("Validation error(s) detected: Value at '{member_path}' failed to satisfy constraints.")
}

/// The one sentence this service answers an inverted time range with.
const TIME_RANGE_MESSAGE: &str = "Specify a start time that is before or equal to the end time.";

/// One row of the modelled-constraint table.
struct ConstraintCase {
    /// What the row exercises, quoted in every assertion.
    label: &'static str,
    operation: &'static str,
    input: Value,
    /// The dotted member path the template must name.
    member_path: &'static str,
    /// The offending value as the caller sent it: the message must NOT contain
    /// it. Empty for a row whose violation is an absent member.
    echoed: String,
}

/// A `DescribeDimensionKeys` request that satisfies every modelled constraint,
/// for a row to break exactly one member of.
///
/// Members and constraints from
/// `smithy_query(query="operation",operation="DescribeDimensionKeys")`:
/// `ServiceType`, `Identifier`, `StartTime`, `EndTime`, `Metric` and `GroupBy`
/// are all `@required`.
fn describe_dimension_keys_input() -> Value {
    json!({
        "ServiceType": "RDS",
        "Identifier": IDENTIFIER,
        "StartTime": T0,
        "EndTime": T0 + 3_600.0,
        "Metric": "db.load.avg",
        "GroupBy": {"Group": "db.sql"},
    })
}

/// A `GetResourceMetrics` request that satisfies every modelled constraint.
fn resource_metrics_input() -> Value {
    json!({
        "ServiceType": "RDS",
        "Identifier": IDENTIFIER,
        "MetricQueries": [{"Metric": "db.load.avg"}],
        "StartTime": T0,
        "EndTime": T0 + 3_600.0,
    })
}

/// A `ListAvailableResourceDimensions` request that satisfies every modelled
/// constraint (`Metrics` is `@required` with `@length` 1..5).
fn available_dimensions_input() -> Value {
    json!({
        "ServiceType": "RDS",
        "Identifier": IDENTIFIER,
        "Metrics": ["db.load.avg"],
    })
}

/// `n` distinct metric names, for a row that varies a list's SIZE.
fn metrics(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| Value::String(format!("db.sql.stat{index}")))
            .collect(),
    )
}

/// The table: every constraint kind the model uses, on every member type it
/// uses them on, across five operations. `@range` and `@length` rows sit one
/// step past each published bound.
fn constraint_cases() -> Vec<ConstraintCase> {
    let mut cases: Vec<ConstraintCase> = Vec::new();

    // `MaxResults` targets `MaxResults`, `@range` 0..25 — one step past each
    // bound, and far past the top.
    for (label, value) in [
        ("DescribeDimensionKeys MaxResults one past the top", 26_i64),
        ("DescribeDimensionKeys MaxResults far past the top", 999),
        ("DescribeDimensionKeys MaxResults one below the bottom", -1),
    ] {
        let mut input = describe_dimension_keys_input();
        input["MaxResults"] = json!(value);
        cases.push(ConstraintCase {
            label,
            operation: "DescribeDimensionKeys",
            input,
            member_path: "maxResults",
            echoed: value.to_string(),
        });
    }

    // The same `@range`, reached through two other operations: the stage is not
    // per-operation.
    let mut list_reports = json!({"ServiceType": "RDS", "Identifier": IDENTIFIER});
    list_reports["MaxResults"] = json!(26);
    cases.push(ConstraintCase {
        label: "ListPerformanceAnalysisReports MaxResults one past the top",
        operation: "ListPerformanceAnalysisReports",
        input: list_reports,
        member_path: "maxResults",
        echoed: "26".to_string(),
    });
    let mut metrics_page = resource_metrics_input();
    metrics_page["MaxResults"] = json!(26);
    cases.push(ConstraintCase {
        label: "GetResourceMetrics MaxResults one past the top",
        operation: "GetResourceMetrics",
        input: metrics_page,
        member_path: "maxResults",
        echoed: "26".to_string(),
    });

    // `Metrics` targets `DimensionsMetricList`, `@length` 1..5 — a list member,
    // one step past each bound.
    for (label, count) in [
        ("ListAvailableResourceDimensions Metrics one past the top", 6_usize),
        ("ListAvailableResourceDimensions Metrics one below the bottom", 0),
    ] {
        let mut input = available_dimensions_input();
        input["Metrics"] = metrics(count);
        cases.push(ConstraintCase {
            label,
            operation: "ListAvailableResourceDimensions",
            input,
            member_path: "metrics",
            // The stage renders a list's length as "<n> items"; neither the
            // count nor any element may come back.
            echoed: format!("{count} items"),
        });
    }

    // `MetricQueries` targets `MetricQueryList`, `@length` 1..15 — the same
    // constraint kind on another operation's list member.
    for (label, count) in [
        ("GetResourceMetrics MetricQueries one past the top", 16_usize),
        ("GetResourceMetrics MetricQueries one below the bottom", 0),
    ] {
        let mut input = resource_metrics_input();
        input["MetricQueries"] = Value::Array(
            (0..count)
                .map(|index| json!({"Metric": format!("db.sql.stat{index}")}))
                .collect(),
        );
        cases.push(ConstraintCase {
            label,
            operation: "GetResourceMetrics",
            input,
            member_path: "metricQueries",
            echoed: format!("{count} items"),
        });
    }

    // `Identifier` targets `IdentifierString`, `@pattern ^[a-zA-Z0-9-]+$` with
    // `@length` 0..256 — a string member, broken by a space, an underscore, a
    // punctuation mark, and (length) one character past the top bound.
    for (label, identifier) in [
        ("GetResourceMetadata Identifier with spaces", "not a valid id!".to_string()),
        ("GetResourceMetadata Identifier with an underscore", "db_ABCDEFG1234567".to_string()),
        ("GetResourceMetadata Identifier with a plus", "db+ABCDEFG".to_string()),
        ("GetResourceMetadata Identifier one character past the length bound", "a".repeat(257)),
    ] {
        let input = json!({"ServiceType": "RDS", "Identifier": identifier});
        cases.push(ConstraintCase {
            label,
            operation: "GetResourceMetadata",
            input,
            member_path: "identifier",
            echoed: identifier,
        });
    }

    // `NextToken` targets `NextToken`, `@pattern ^[a-zA-Z0-9_=-]+$` — the same
    // kind on a different member and operation.
    let mut bad_token = resource_metrics_input();
    bad_token["NextToken"] = json!("has spaces");
    cases.push(ConstraintCase {
        label: "GetResourceMetrics NextToken outside the pattern",
        operation: "GetResourceMetrics",
        input: bad_token,
        member_path: "nextToken",
        echoed: "has spaces".to_string(),
    });

    // An enum value set (`ServiceType` is `RDS | DOCDB`).
    cases.push(ConstraintCase {
        label: "GetResourceMetadata ServiceType outside the enum value set",
        operation: "GetResourceMetadata",
        input: json!({"ServiceType": "AURORA", "Identifier": IDENTIFIER}),
        member_path: "serviceType",
        echoed: "AURORA".to_string(),
    });

    // `@required`: an absent member, at the top level and nested inside a list
    // element, where the path names the element's 1-based position.
    cases.push(ConstraintCase {
        label: "GetResourceMetadata without Identifier",
        operation: "GetResourceMetadata",
        input: json!({"ServiceType": "RDS"}),
        member_path: "identifier",
        echoed: String::new(),
    });
    cases.push(ConstraintCase {
        label: "CreatePerformanceAnalysisReport without StartTime",
        operation: "CreatePerformanceAnalysisReport",
        input: json!({"ServiceType": "RDS", "Identifier": IDENTIFIER}),
        member_path: "startTime",
        echoed: String::new(),
    });
    let mut queries_without_metric = resource_metrics_input();
    queries_without_metric["MetricQueries"] = json!([{"Metric": "db.load.avg"}, {}]);
    cases.push(ConstraintCase {
        label: "GetResourceMetrics with a MetricQueries element missing Metric",
        operation: "GetResourceMetrics",
        input: queries_without_metric,
        member_path: "metricQueries.2.member.metric",
        echoed: String::new(),
    });

    cases
}

/// RULE 1: every modelled-constraint violation is answered with HTTP 400,
/// `ValidationException`, and the member-named template — whatever the
/// constraint kind, the member type or the operation.
#[tokio::test]
async fn modelled_constraint_violations_are_validation_exceptions() {
    let emulator = emulator();

    for case in constraint_cases() {
        let ConstraintCase {
            label,
            operation,
            input,
            member_path,
            echoed,
        } = case;
        let answer = call(&emulator, operation, input).await;

        assert_eq!(
            answer.status, 400,
            "case {label}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {label}: expected ValidationException, got {:?}",
            answer.body
        );
        let message = answer.error_message();
        assert_eq!(
            message,
            expected_validation_message(member_path),
            "case {label}: expected the framework template naming '{member_path}', got \
             {message:?}"
        );
        // The template quotes no Smithy constraint text…
        for forbidden in ["Member must", "regular expression", "satisfy constraint:"] {
            assert!(
                !message.contains(forbidden),
                "case {label}: expected no constraint text, but {message:?} contains \
                 {forbidden:?}"
            );
        }
        // …and echoes no caller-supplied value back out of the service.
        if !echoed.is_empty() {
            assert!(
                !message.contains(&echoed),
                "case {label}: expected the offending value to be suppressed, but \
                 {message:?} contains {echoed:?}"
            );
        }
    }
}

/// RULE 1, the other side of each published bound: a value INSIDE a modelled
/// constraint is never refused by the validation stage. Each row sits exactly on
/// a bound, including the empty-but-legal cases, so the stage is pinned as
/// refusing nothing it should pass. The answer's own status is not pinned — some
/// of these operations are still scaffolded — only that the stage let it through.
#[tokio::test]
async fn values_on_the_published_bounds_pass_the_validation_stage() {
    let emulator = emulator();

    let mut cases: Vec<(&str, &str, Value)> = Vec::new();
    for value in [0_i64, 1, 24, 25] {
        let mut input = describe_dimension_keys_input();
        input["MaxResults"] = json!(value);
        cases.push((
            "DescribeDimensionKeys MaxResults on the range bounds",
            "DescribeDimensionKeys",
            input,
        ));
    }
    for count in [1_usize, 5] {
        let mut input = available_dimensions_input();
        input["Metrics"] = metrics(count);
        cases.push((
            "ListAvailableResourceDimensions Metrics on the length bounds",
            "ListAvailableResourceDimensions",
            input,
        ));
    }
    for identifier in ["a".to_string(), "A-0".to_string(), "a".repeat(256)] {
        cases.push((
            "GetResourceMetadata Identifier on the length bounds",
            "GetResourceMetadata",
            json!({"ServiceType": "RDS", "Identifier": identifier}),
        ));
    }
    for service_type in ["RDS", "DOCDB"] {
        cases.push((
            "GetResourceMetadata every enum value",
            "GetResourceMetadata",
            json!({"ServiceType": service_type, "Identifier": IDENTIFIER}),
        ));
    }

    for (label, operation, input) in cases {
        let answer = call(&emulator, operation, input.clone()).await;
        assert_ne!(
            answer.error_code(),
            "ValidationException",
            "case {label} ({input}): expected the validation stage to pass it, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

/// RULE 2: a window whose start does not precede its end is the HANDLER's
/// refusal — the modelled `InvalidArgumentException` — and every operation that
/// enforces the ordering states the SAME sentence, which carries nothing from
/// the request.
#[tokio::test]
async fn an_inverted_time_range_is_refused_with_one_service_wide_sentence() {
    let emulator = emulator();

    // The rows differ in how the window is inverted (equal ends, a sub-second
    // inversion, a second, a day, a decade) and in the resource named, so
    // nothing in the expected sentence can come from the request.
    let inversions: Vec<(&str, f64, f64)> = vec![
        ("equal ends", T0, T0),
        ("a sub-second inversion", T0 + 0.5, T0),
        ("one second inverted", T0, T0 - 1.0),
        ("a day inverted", T0, T0 - 86_400.0),
        ("a decade inverted", T0, T0 - 315_360_000.0),
    ];

    for (label, start, end) in inversions {
        for identifier in [IDENTIFIER, "db-0123456789"] {
            let cases: Vec<(&str, Value)> = vec![
                (
                    "GetResourceMetrics",
                    json!({
                        "ServiceType": "RDS",
                        "Identifier": identifier,
                        "MetricQueries": [{"Metric": "db.load.avg"}],
                        "StartTime": start,
                        "EndTime": end,
                    }),
                ),
                (
                    "CreatePerformanceAnalysisReport",
                    json!({
                        "ServiceType": "DOCDB",
                        "Identifier": identifier,
                        "StartTime": start,
                        "EndTime": end,
                    }),
                ),
            ];

            for (operation, input) in cases {
                let answer = call(&emulator, operation, input).await;
                assert_eq!(
                    answer.status, 400,
                    "case {label} / {operation}: expected 400, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidArgumentException",
                    "case {label} / {operation}: expected InvalidArgumentException, got {:?}",
                    answer.body
                );
                let message = answer.error_message();
                assert_eq!(
                    message, TIME_RANGE_MESSAGE,
                    "case {label} / {operation}: expected the service-wide ordering sentence, \
                     got {message:?}"
                );
                // Nothing in the sentence comes from the request or names the
                // operation it was stated by.
                for forbidden in [identifier, operation, "StartTime", "EndTime"] {
                    assert!(
                        !message.contains(forbidden),
                        "case {label} / {operation}: expected a request-independent sentence, \
                         but {message:?} contains {forbidden:?}"
                    );
                }
            }
        }
    }
}

/// RULE 2 does not swallow RULE 1: a request that is BOTH inverted and breaks a
/// modelled constraint is refused by the stage that runs first, so the
/// validation stage's answer is what the caller sees.
#[tokio::test]
async fn the_validation_stage_answers_before_the_handler() {
    let emulator = emulator();

    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "GetResourceMetrics inverted with a bad Identifier",
            "GetResourceMetrics",
            json!({
                "ServiceType": "RDS",
                "Identifier": "not a valid id!",
                "MetricQueries": [{"Metric": "db.load.avg"}],
                "StartTime": T0,
                "EndTime": T0 - 1.0,
            }),
            "identifier",
        ),
        (
            "CreatePerformanceAnalysisReport inverted with a ServiceType outside the enum",
            "CreatePerformanceAnalysisReport",
            json!({
                "ServiceType": "AURORA",
                "Identifier": IDENTIFIER,
                "StartTime": T0,
                "EndTime": T0 - 1.0,
            }),
            "serviceType",
        ),
    ];

    for (label, operation, input, member_path) in cases {
        let answer = call(&emulator, operation, input).await;
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {label}: expected the validation stage to answer first, got {} {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.error_message(),
            expected_validation_message(member_path),
            "case {label}: expected the template naming '{member_path}', got {:?}",
            answer.body
        );
    }
}

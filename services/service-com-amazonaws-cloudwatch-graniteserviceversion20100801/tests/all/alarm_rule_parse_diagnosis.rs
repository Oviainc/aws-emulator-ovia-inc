// SPDX-License-Identifier: CC0-1.0

//! How `PutCompositeAlarm` DIAGNOSES an `AlarmRule` it cannot parse.
//!
//! The rule under test: where a rule carries a token at a position that can only start a
//! TERM, the call is refused `400` under the request-validation class and the message is CloudWatch's parse
//! diagnosis — the `Error in AlarmRule [...]` envelope carrying `Unsupported token
//! '<token>' at char <index>, must be: '(', 'NOT', AT_LEAST, TRUE or FALSE, ALARM, OK, or
//! INSUFFICIENT_DATA`, where the token is the one the submitted rule carries at that
//! position and the index is its offset in CHARACTERS from the start of that rule. That
//! message is what separates a mistyped expression from the two OTHER refusals this
//! operation answers under the same error code (a rule referencing an alarm that does not
//! exist, and a child a live rule depends on).
//!
//! The rows differ along what the rule generalises over rather than along one recorded
//! request: WHERE the offending token sits (the start of the rule, after a binary operator,
//! after `NOT`, inside a group), WHAT it looks like (a word, a single punctuation character, a
//! misspelled function), and whether the text before it is ASCII — because the index is
//! counted in characters, not bytes. The recorded case, a rule with a duplicated `AND`, is one
//! row. A rule that simply ENDS where a term is owed carries no token to name, so it is still
//! refused under the same code but is NOT given the diagnosis with an empty token in it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call, error_code, error_message, ok};

/// The alarm every rule below references, so a row is refused for its SYNTAX and never for
/// naming an alarm that does not exist.
const CHILD: &str = "orders-latency";

/// The tokens the diagnosis lists for a position that can only start a term, as CloudWatch
/// spells them.
const MUST_BE: &str = "'(', 'NOT', AT_LEAST, TRUE or FALSE, ALARM, OK, or INSUFFICIENT_DATA";

/// The diagnosis CloudWatch answers for `token` at `at_char` of the submitted rule.
fn unsupported_token(token: &str, at_char: usize) -> String {
    format!("Error in AlarmRule [Unsupported token '{token}' at char {at_char}, must be: {MUST_BE}]")
}

/// A metric alarm a rule may reference.
async fn put_metric_alarm(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "Custom/Orders",
            "MetricName": "Latency",
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
            "TreatMissingData": "ignore",
        }),
    )
    .await;
}

/// The composite alarms `DescribeAlarms` reports for `name`.
async fn composite_alarms(router: &MultiServiceRouter, name: &str) -> Value {
    ok(
        router,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": ["CompositeAlarm"]}),
    )
    .await["CompositeAlarms"]
        .clone()
}

#[tokio::test]
async fn a_rule_that_stops_making_sense_is_diagnosed_the_way_cloudwatch_diagnoses_it() {
    let router = build_router();
    put_metric_alarm(&router, CHILD).await;
    // A child whose name is not ASCII, so a row can prove the index is a CHARACTER offset
    // rather than a byte one.
    let accented = "café-latency";
    put_metric_alarm(&router, accented).await;

    // (case, the rule, the offending token, its offset in characters)
    let rows: Vec<(&str, String, &str, usize)> = vec![
        (
            // The recorded shape: an operator where the second operand belongs.
            "a duplicated AND",
            format!("ALARM(\"{CHILD}\") AND AND OK(\"{CHILD}\")"),
            "AND",
            format!("ALARM(\"{CHILD}\") AND ").chars().count(),
        ),
        (
            "a rule that opens with a binary operator",
            format!("OR ALARM(\"{CHILD}\")"),
            "OR",
            0,
        ),
        (
            "an operator after NOT",
            format!("NOT OR ALARM(\"{CHILD}\")"),
            "OR",
            4,
        ),
        (
            "an operator inside a group",
            format!("(AND ALARM(\"{CHILD}\"))"),
            "AND",
            1,
        ),
        (
            // A single punctuation character rather than a word.
            "a stray closing parenthesis where a term belongs",
            format!("ALARM(\"{CHILD}\") OR )"),
            ")",
            format!("ALARM(\"{CHILD}\") OR ").chars().count(),
        ),
        (
            "a misspelled state function",
            format!("ALARMED(\"{CHILD}\")"),
            "ALARMED",
            0,
        ),
        (
            // Three bytes, one character: the index must count the character.
            "a non-ASCII child name before the offending token",
            format!("ALARM(\"{accented}\") AND AND TRUE"),
            "AND",
            format!("ALARM(\"{accented}\") AND ").chars().count(),
        ),
    ];

    for (index, (case, rule, token, at_char)) in rows.iter().enumerate() {
        let name = format!("composite-{index}");
        let (status, body) = call(
            &router,
            "PutCompositeAlarm",
            &json!({"AlarmName": name, "AlarmRule": rule}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException for {rule:?}, got {status} with \
             {body}"
        );
        let expected = unsupported_token(token, *at_char);
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the parse diagnosis {expected:?} for {rule:?}, got \
             {actual:?}"
        );
        // A refused rule writes nothing: the diagnosis is the whole answer.
        let stored = composite_alarms(&router, &name).await;
        assert_eq!(
            stored,
            json!([]),
            "case {case:?}: a refused rule must store no composite alarm, got {stored}"
        );
    }
}

#[tokio::test]
async fn the_parse_diagnosis_is_told_apart_from_the_other_refusals_of_the_same_code() {
    let router = build_router();
    put_metric_alarm(&router, CHILD).await;

    // A rule this parser READS is accepted, so the diagnosis above is about the syntax and
    // not about every rule.
    ok(
        &router,
        "PutCompositeAlarm",
        &json!({
            "AlarmName": "orders-health",
            "AlarmRule": format!("ALARM(\"{CHILD}\") OR NOT OK(\"{CHILD}\")"),
        }),
    )
    .await;

    // The other refusal of the same error code: a rule whose syntax is fine and whose child
    // does not exist is NOT diagnosed as a parse failure, so a client can tell the two apart
    // by the message alone.
    let (status, body) = call(
        &router,
        "PutCompositeAlarm",
        &json!({"AlarmName": "ghost-parent", "AlarmRule": "ALARM(\"no-such-alarm\")"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a rule naming an alarm that does not exist is refused, got {status} with {body}"
    );
    let message = error_message(&body);
    assert!(
        !message.starts_with("Error in AlarmRule ["),
        "a rule that PARSES must not be refused as a parse failure, got {message:?}"
    );
    assert!(
        message.contains("do not exist"),
        "the existence refusal must say so, got {message:?}"
    );

    // A rule that ENDS where it owes a term carries no token to name, so it is refused under
    // the same code without the diagnosis above: a message naming the empty string as the
    // token the rule carries would be a variant of the service's wording that nothing records.
    let (status, body) = call(
        &router,
        "PutCompositeAlarm",
        &json!({"AlarmName": "truncated", "AlarmRule": format!("ALARM(\"{CHILD}\") AND")}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a rule that ends before the term it owes is refused, got {status} with {body}"
    );
    let message = error_message(&body);
    assert!(
        !message.contains("Unsupported token ''"),
        "a rule with no token at the position it stopped at must not be diagnosed as carrying \
         one, got {message:?}"
    );
    let stored = composite_alarms(&router, "truncated").await;
    assert_eq!(
        stored,
        json!([]),
        "the truncated rule must store no composite alarm, got {stored}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! How Amazon Keyspaces states a request its BOUNDARY validator refused, over every
//! operation and member whose shape carries a constraint — driven end to end over the
//! service's own `awsJson1_0` wire through the shared
//! [`crate::keyspace_table_lifecycle::Emulator`] harness.
//!
//! The rule under test, exercised over a SET of inputs rather than the two operations one
//! divergence was reported on: a refused request is answered `400 ValidationException`
//! with AWS's aggregated envelope — `N validation error(s) detected: ` and one
//! `Value '<value>' at '<member path>' failed to satisfy constraint: <requirement>` clause
//! per violation, joined with `; ` — where a `@pattern` requirement is quoted WITHOUT the
//! whole-value anchor pair the model wraps it in, and the several clauses about ONE member
//! come out in the order that member's own refusal is recorded in.
//!
//! The rows span:
//!
//! * the member: `keyspaceName` and `tableName` (the two shapes carrying the name
//!   pattern), the `source`/`target` spellings `RestoreTable` binds them under, and
//!   `resourceArn` (a different shape with a different, one-sided pattern);
//! * the operation: a create, a read, a drop, an update, a list and a restore, so the
//!   rendering is pinned where the validating walk runs and not per handler;
//! * the number and kind of violations: the pattern alone, the pattern with a maximum
//!   length, the pattern with a minimum length;
//! * the form of the offending value: over-long, empty, a disallowed separator, a
//!   disallowed first character, and both published length bounds plus one past each —
//!   a value AT a bound must reach the handler rather than the validator.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::scaffolded;

use crate::keyspace_table_lifecycle::{error_code, minimal_schema, Emulator};

/// The `message` member of a failure body, as a string.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message member>")
        .to_string()
}

/// `KeyspaceName` and `TableName` are both modelled `@length 1..48` with this `@pattern`
/// (`smithy_query(query="shape",shape="KeyspaceName,TableName",model_path=\
/// "services/service-com-amazonaws-keyspaces-keyspacesservice/smithy-model/\
/// com-amazonaws-keyspaces-keyspacesservice.json")`), so every member targeting either
/// shape carries the same pair of constraints.
const NAME_PATTERN_AS_MODELLED: &str = "^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$";
/// `@length`'s bounds on those two shapes, from the same query.
const NAME_LENGTH: (usize, usize) = (1, 48);
/// `ARN`'s `@pattern`, from the same query: anchored at ONE end only, which is a
/// different expression rather than an anchored spelling of an unanchored one.
const ARN_PATTERN_AS_MODELLED: &str = "^arn:(aws[a-zA-Z0-9-]*):cassandra:.+";
/// `ARN`'s `@length` minimum, from the same query.
const ARN_LENGTH_MINIMUM: usize = 20;

/// The expression a refusal quotes for `pattern`: the whole-value anchor PAIR the model
/// wrote around the service's own expression is dropped, and an expression anchored at one
/// end only is quoted as the model declares it.
fn stated_pattern(pattern: &str) -> String {
    match pattern.strip_prefix('^').and_then(|p| p.strip_suffix('$')) {
        Some(inner) => inner.to_string(),
        None => pattern.to_string(),
    }
}

/// The `@pattern` requirement clause's text.
fn must_match(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        stated_pattern(pattern)
    )
}

/// The `@length` maximum requirement clause's text.
fn must_be_no_longer_than(maximum: usize) -> String {
    format!("Member must have length less than or equal to {maximum}")
}

/// The `@length` minimum requirement clause's text.
fn must_be_at_least(minimum: usize) -> String {
    format!("Member must have length greater than or equal to {minimum}")
}

/// The whole message: the count, the noun it agrees with, and one clause per requirement
/// in the order stated.
fn envelope(value: &str, member: &str, requirements: &[String]) -> String {
    let noun = match requirements.len() {
        1 => "validation error",
        _ => "validation errors",
    };
    let clauses: Vec<String> = requirements
        .iter()
        .map(|requirement| {
            format!("Value '{value}' at '{member}' failed to satisfy constraint: {requirement}")
        })
        .collect();
    format!(
        "{} {noun} detected: {}",
        requirements.len(),
        clauses.join("; ")
    )
}

/// One refusal to drive: the call, the member the clauses are about, the value it carried,
/// and the requirements in the order the message states them.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    member: &'static str,
    value: String,
    requirements: Vec<String>,
}

/// RULE: a boundary refusal is AWS's aggregated validation envelope, with every
/// `@pattern` requirement quoted without the model's whole-value anchor pair, wherever the
/// validating walk runs.
#[tokio::test]
async fn every_refused_member_states_its_pattern_requirement_unanchored() {
    let emulator = Emulator::start();
    let (minimum, maximum) = NAME_LENGTH;
    // One past the maximum, and far past it: the pattern admits at most `maximum`
    // characters too, so either value breaks both constraints.
    let one_too_long = "a".repeat(maximum + 1);
    let far_too_long = format!("ks_{}", "9".repeat(maximum * 2));

    let rows: Vec<Row> = vec![
        // `keyspaceName`: the pattern clause is stated BEFORE that member's other clauses.
        Row {
            case: "CreateKeyspace/keyspaceName one past the maximum",
            operation: "CreateKeyspace",
            request: json!({"keyspaceName": one_too_long}),
            member: "keyspaceName",
            value: one_too_long.clone(),
            requirements: vec![
                must_match(NAME_PATTERN_AS_MODELLED),
                must_be_no_longer_than(maximum),
            ],
        },
        Row {
            case: "ListTables/keyspaceName far past the maximum",
            operation: "ListTables",
            request: json!({"keyspaceName": far_too_long}),
            member: "keyspaceName",
            value: far_too_long.clone(),
            requirements: vec![
                must_match(NAME_PATTERN_AS_MODELLED),
                must_be_no_longer_than(maximum),
            ],
        },
        Row {
            case: "GetKeyspace/keyspaceName empty",
            operation: "GetKeyspace",
            request: json!({"keyspaceName": ""}),
            member: "keyspaceName",
            value: String::new(),
            requirements: vec![
                must_match(NAME_PATTERN_AS_MODELLED),
                must_be_at_least(minimum),
            ],
        },
        Row {
            case: "DeleteKeyspace/keyspaceName with a disallowed separator",
            operation: "DeleteKeyspace",
            request: json!({"keyspaceName": "dash-name"}),
            member: "keyspaceName",
            value: "dash-name".to_string(),
            requirements: vec![must_match(NAME_PATTERN_AS_MODELLED)],
        },
        Row {
            case: "ListTypes/keyspaceName with a disallowed first character",
            operation: "ListTypes",
            request: json!({"keyspaceName": "_leading_underscore"}),
            member: "keyspaceName",
            value: "_leading_underscore".to_string(),
            requirements: vec![must_match(NAME_PATTERN_AS_MODELLED)],
        },
        // `tableName`: the same pair of clauses, in the validating walk's own order.
        Row {
            case: "CreateTable/tableName one past the maximum",
            operation: "CreateTable",
            request: json!({
                "keyspaceName": "envelope_ks",
                "tableName": one_too_long,
                "schemaDefinition": minimal_schema("pk"),
            }),
            member: "tableName",
            value: one_too_long.clone(),
            requirements: vec![
                must_be_no_longer_than(maximum),
                must_match(NAME_PATTERN_AS_MODELLED),
            ],
        },
        Row {
            case: "GetTable/tableName empty",
            operation: "GetTable",
            request: json!({"keyspaceName": "envelope_ks", "tableName": ""}),
            member: "tableName",
            value: String::new(),
            requirements: vec![
                must_be_at_least(minimum),
                must_match(NAME_PATTERN_AS_MODELLED),
            ],
        },
        Row {
            case: "DeleteTable/tableName with a disallowed separator",
            operation: "DeleteTable",
            request: json!({"keyspaceName": "envelope_ks", "tableName": "Dash-Name"}),
            member: "tableName",
            value: "Dash-Name".to_string(),
            requirements: vec![must_match(NAME_PATTERN_AS_MODELLED)],
        },
        Row {
            case: "UpdateTable/tableName with a disallowed separator",
            operation: "UpdateTable",
            request: json!({
                "keyspaceName": "envelope_ks",
                "tableName": "has space",
                "addColumns": [{"name": "c", "type": "int"}],
            }),
            member: "tableName",
            value: "has space".to_string(),
            requirements: vec![must_match(NAME_PATTERN_AS_MODELLED)],
        },
        Row {
            case: "GetTableAutoScalingSettings/tableName far past the maximum",
            operation: "GetTableAutoScalingSettings",
            request: json!({"keyspaceName": "envelope_ks", "tableName": far_too_long}),
            member: "tableName",
            value: far_too_long.clone(),
            requirements: vec![
                must_be_no_longer_than(maximum),
                must_match(NAME_PATTERN_AS_MODELLED),
            ],
        },
        // The same two shapes under the spellings `RestoreTable` binds them to: the member
        // PATH is what a clause names, so neither inherits `keyspaceName`'s clause order.
        Row {
            case: "RestoreTable/targetTableName one past the maximum",
            operation: "RestoreTable",
            request: json!({
                "sourceKeyspaceName": "envelope_ks",
                "sourceTableName": "src_tbl",
                "targetKeyspaceName": "envelope_ks",
                "targetTableName": one_too_long,
            }),
            member: "targetTableName",
            value: one_too_long.clone(),
            requirements: vec![
                must_be_no_longer_than(maximum),
                must_match(NAME_PATTERN_AS_MODELLED),
            ],
        },
        Row {
            case: "RestoreTable/sourceKeyspaceName with a disallowed separator",
            operation: "RestoreTable",
            request: json!({
                "sourceKeyspaceName": "dash-name",
                "sourceTableName": "src_tbl",
                "targetKeyspaceName": "envelope_ks",
                "targetTableName": "tgt_tbl",
            }),
            member: "sourceKeyspaceName",
            value: "dash-name".to_string(),
            requirements: vec![must_match(NAME_PATTERN_AS_MODELLED)],
        },
        // A different shape, whose `@pattern` anchors one end only: the pair is what the
        // model wrote around the expression, so there is no pair to drop here.
        Row {
            case: "ListTagsForResource/resourceArn below its minimum length",
            operation: "ListTagsForResource",
            request: json!({"resourceArn": "not-an-arn"}),
            member: "resourceArn",
            value: "not-an-arn".to_string(),
            requirements: vec![
                must_be_at_least(ARN_LENGTH_MINIMUM),
                must_match(ARN_PATTERN_AS_MODELLED),
            ],
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, body) = emulator.call(row.operation, row.request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ValidationException"),
            "case {case:?}: expected the modelled validation failure, got {status} {body}"
        );
        let expected = envelope(&row.value, row.member, &row.requirements);
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(NAME_PATTERN_AS_MODELLED),
            "case {case:?}: the message quotes the model's anchored pattern, got {actual:?}"
        );
    }
}

/// RULE, the other side: a value the constraints ADMIT is not refused at the boundary, so
/// each published length bound reaches the handler and only a value past it is refused.
#[tokio::test]
async fn a_name_at_either_published_length_bound_reaches_the_handler() {
    let emulator = Emulator::start();
    let (minimum, maximum) = NAME_LENGTH;
    let shortest = "a".repeat(minimum);
    let longest = format!("k{}", "9".repeat(maximum - 1));
    let one_too_long = "a".repeat(maximum + 1);

    // (case, keyspaceName, whether the boundary validator refuses it)
    let rows: Vec<(String, String, bool)> = vec![
        (format!("at the minimum ({minimum})"), shortest, false),
        (format!("at the maximum ({maximum})"), longest, false),
        (
            format!("one past the maximum ({})", maximum + 1),
            one_too_long,
            true,
        ),
        ("one below the minimum (0)".to_string(), String::new(), true),
    ];

    for (case, name, refused) in &rows {
        let (status, body) = emulator
            .call("GetKeyspace", json!({"keyspaceName": name}))
            .await;
        let expected_code = match refused {
            true => "ValidationException",
            false => "ResourceNotFoundException",
        };
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, expected_code),
            "case {case:?}: expected {expected_code}, got {status} {body}"
        );
        let actual = message(&body);
        if !refused {
            assert_eq!(
                actual,
                format!("Keyspace {name} does not exist"),
                "case {case:?}: the handler, not the validator, must answer, got {actual:?}"
            );
        }
    }
}

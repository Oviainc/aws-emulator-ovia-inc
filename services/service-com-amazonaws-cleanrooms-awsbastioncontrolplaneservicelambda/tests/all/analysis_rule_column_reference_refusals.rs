// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal every Clean Rooms operation shares when a configured
//! table's analysis rule would reference columns the table's `allowedColumns` do not
//! carry.
//!
//! The rule under test: `allowedColumns` are "the columns of the underlying table that can
//! be used by collaborations or analysis rules", so a request that would leave an analysis
//! rule naming a column the table does not expose — whether it is the POLICY that names
//! the column (`CreateConfiguredTableAnalysisRule`,
//! `UpdateConfiguredTableAnalysisRule`) or the COLUMN LIST that stops exposing one the
//! stored policy names (`UpdateConfiguredTable`) — is refused with HTTP 400
//! `ValidationException` whose message states the unsatisfied constraint and the remedy
//! and interpolates exactly one thing: the referenced-but-not-allowed column names,
//! bracketed and comma separated in the order the policy names them. Neither the
//! configured table addressed nor the analysis rule type appears in that sentence.
//!
//! The tables vary what the rule generalises over: which operation reaches the
//! constraint, every `ConfiguredTableAnalysisRuleType` (so every policy union member and
//! every column-bearing member inside it), how many columns are referenced but not
//! allowed (one, two, all of them) and in which order the policy names them. A row that
//! strands no reference is the positive control, and the no-mutation and
//! delete-then-retry legs pin that only the wording moved. Every request goes over the
//! wire through the same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{account_id, error_code, error_message, Emulator};
use super::cleanrooms_configured_table::{table_reference, COLUMN_NAMES};
use super::configured_table_analysis_rules::ALL_ANALYSIS_RULE_TYPES;

/// The column names these tables draw on: the suites' shared `COLUMN_NAMES` catalog plus
/// two more, so a row can keep a column no policy names and strand up to three that a
/// policy does name. `ColumnName` matches
/// `^[a-z0-9_](([a-z0-9_ ]+-)*([a-z0-9_ ]+))?$`, so both additions are valid.
const COLUMNS: [&str; 5] = [
    COLUMN_NAMES[0],
    COLUMN_NAMES[1],
    COLUMN_NAMES[2],
    "shipping_zone",
    "promo_code",
];

/// The sentence AWS refuses an analysis rule's unsatisfied column reference with: fixed
/// text, with the referenced-but-not-allowed column names as its only variable part.
fn expected_message(columns: &[&str]) -> String {
    format!(
        "The following columns in the analysis rule aren't present in the allowed columns \
         list: [{}]. To be used in the analysis rule, columns must be specified as allowed \
         columns.",
        columns.join(", ")
    )
}

/// A `ConfiguredTableAnalysisRulePolicy` of `rule_type` that fills its three
/// column-bearing roles with `roles` (join, value and dimension), and the column names it
/// references in the order the policy names them.
///
/// Which members of a policy name a column, and in which order this collects them, comes
/// from the model: `AnalysisRuleList`'s `joinColumns` then `listColumns`;
/// `AnalysisRuleAggregation`'s `aggregateColumns[].columnNames`, `joinColumns`,
/// `dimensionColumns` then `outputConstraints[].columnName`; `AnalysisRuleCustom`'s
/// `disallowedOutputColumns` then `differentialPrivacy.columns[].name`.
fn policy_over(rule_type: &str, roles: [&'static str; 3]) -> (Value, Vec<&'static str>) {
    let [join, value, dimension] = roles;
    match rule_type {
        "LIST" => (
            json!({ "v1": { "list": {
                "joinColumns": [join],
                "listColumns": [value, dimension],
                "allowedJoinOperators": ["OR"],
            }}}),
            vec![join, value, dimension],
        ),
        "AGGREGATION" => (
            json!({ "v1": { "aggregation": {
                "aggregateColumns": [{ "columnNames": [value], "function": "SUM" }],
                "joinColumns": [join],
                "dimensionColumns": [dimension],
                "scalarFunctions": ["ABS"],
                "outputConstraints": [{
                    "columnName": dimension,
                    "minimum": 2,
                    "type": "COUNT_DISTINCT",
                }],
            }}}),
            vec![value, join, dimension, dimension],
        ),
        "CUSTOM" => (
            json!({ "v1": { "custom": {
                "allowedAnalyses": ["ANY_QUERY"],
                "allowedAnalysisProviders": [account_id()],
                "disallowedOutputColumns": [dimension, value],
                "differentialPrivacy": { "columns": [{ "name": join }] },
            }}}),
            vec![dimension, value, join],
        ),
        other => panic!("no ConfiguredTableAnalysisRulePolicyV1 member for rule type {other:?}"),
    }
}

/// The columns `referenced` names that `allowed` does not carry, each once, in the order
/// the policy names them — what the refusal must list.
fn stranded(referenced: &[&'static str], allowed: &[&'static str]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for column in referenced {
        if !allowed.contains(column) && !out.contains(column) {
            out.push(column);
        }
    }
    out
}

/// A configured table over `allowed`, and its id.
async fn table_over(emulator: &Emulator, name: &str, allowed: &[&str]) -> String {
    let (status, created) = emulator
        .create_configured_table(json!({
            "name": name,
            "tableReference": table_reference("GLUE"),
            "allowedColumns": allowed.to_vec(),
            "analysisMethod": "DIRECT_QUERY",
        }))
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateConfiguredTable 200, got {status} with {created}"
    );
    created["configuredTable"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: the created table must carry an id, got {created}"))
        .to_string()
}

/// Assert one refusal: HTTP 400, `ValidationException`, and AWS's sentence over exactly
/// `expected_columns` — and that it interpolates neither the table nor the rule type.
fn assert_column_reference_refusal(
    label: &str,
    table_id: &str,
    rule_type: &str,
    expected_columns: &[&str],
    status: u16,
    response: &Value,
) {
    assert_eq!(
        status, 400,
        "case {label:?}: a stranded column reference must be refused with 400, got {status} with {response}"
    );
    assert_eq!(
        error_code(response),
        "ValidationException",
        "case {label:?}: expected ValidationException, got {response}"
    );
    let message = error_message(response);
    assert_eq!(
        message,
        expected_message(expected_columns),
        "case {label:?}: expected {:?}, got {message:?}",
        expected_message(expected_columns)
    );
    assert!(
        !message.contains(table_id),
        "case {label:?}: the refusal must not name the configured table, got {message:?}"
    );
    assert!(
        !message.contains(rule_type),
        "case {label:?}: the refusal must not name the analysis rule type, got {message:?}"
    );
}

#[tokio::test]
async fn update_configured_table_states_the_column_reference_constraint_it_refuses_by() {
    let emulator = Emulator::new();

    // Role assignment: join, value and dimension are the first three catalog columns, so
    // `COLUMNS[3]` is a column no policy names and `COLUMNS[4]` is never allowed.
    let roles: [&str; 3] = [COLUMNS[0], COLUMNS[1], COLUMNS[2]];
    // What the narrowing update leaves allowed: one stranded column, two, all three, and
    // (the positive control) none.
    let narrowings: [(&str, Vec<&str>); 4] = [
        ("drops the join column", vec![COLUMNS[1], COLUMNS[2], COLUMNS[3]]),
        ("drops two referenced columns", vec![COLUMNS[0], COLUMNS[3]]),
        ("drops every referenced column", vec![COLUMNS[3]]),
        ("drops only the unreferenced column", vec![COLUMNS[0], COLUMNS[1], COLUMNS[2]]),
    ];

    let mut row = 0;
    for rule_type in ALL_ANALYSIS_RULE_TYPES {
        let (policy, referenced) = policy_over(rule_type, roles);
        for (narrowing, kept) in &narrowings {
            let label = format!("{rule_type} rule, update that {narrowing}");
            row += 1;
            let table_id = table_over(&emulator, &format!("column-refusal-{row}"), &COLUMNS[..4]).await;
            let (status, created) = emulator
                .create_analysis_rule(&table_id, rule_type, policy.clone())
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected CreateConfiguredTableAnalysisRule 200, got {status} with {created}"
            );

            let expected = stranded(&referenced, kept);
            let (status, response) = emulator
                .call(
                    "PATCH",
                    &format!("/configuredTables/{table_id}"),
                    Some(json!({ "allowedColumns": kept.clone() })),
                )
                .await;

            if expected.is_empty() {
                assert_eq!(
                    status, 200,
                    "case {label:?}: a narrowing that strands no reference must be accepted, got {status} with {response}"
                );
                assert_eq!(
                    response["configuredTable"]["allowedColumns"], json!(kept.clone()),
                    "case {label:?}: expected allowedColumns {:?}, got {}",
                    kept,
                    response["configuredTable"]["allowedColumns"]
                );
                continue;
            }

            assert_column_reference_refusal(
                &label,
                &table_id,
                rule_type,
                &expected,
                status,
                &response,
            );

            // The refused update persisted nothing: the table still exposes every column
            // the create gave it.
            let (status, read) = emulator.get_configured_table(&table_id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetConfiguredTable 200, got {status} with {read}"
            );
            assert_eq!(
                read["configuredTable"]["allowedColumns"],
                json!(COLUMNS[..4].to_vec()),
                "case {label:?}: the refused update must leave allowedColumns at {:?}, got {}",
                &COLUMNS[..4],
                read["configuredTable"]["allowedColumns"]
            );

            // With no rule left to strand, the identical narrowing is accepted — the
            // refusal is about the rule's references, not about narrowing the list.
            let (status, deleted) = emulator.delete_analysis_rule(&table_id, rule_type).await;
            assert_eq!(
                status, 204,
                "case {label:?}: expected DeleteConfiguredTableAnalysisRule 204, got {status} with {deleted}"
            );
            let (status, response) = emulator
                .call(
                    "PATCH",
                    &format!("/configuredTables/{table_id}"),
                    Some(json!({ "allowedColumns": kept.clone() })),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: with the rule deleted the same update must succeed, got {status} with {response}"
            );
            assert_eq!(
                response["configuredTable"]["allowedColumns"], json!(kept.clone()),
                "case {label:?}: expected allowedColumns {:?}, got {}",
                kept,
                response["configuredTable"]["allowedColumns"]
            );
        }
    }
}

#[tokio::test]
async fn analysis_rule_writes_state_the_same_column_reference_constraint() {
    let emulator = Emulator::new();

    // The columns the tables below expose: the join column and the one no policy names.
    // Everything else a policy references is stranded.
    let allowed: [&str; 2] = [COLUMNS[0], COLUMNS[3]];
    // Role assignments, so the stranded set is one column, two, or all three — and in
    // each policy's own naming order.
    let assignments: [(&str, [&'static str; 3]); 3] = [
        ("one stranded column", [COLUMNS[0], COLUMNS[3], COLUMNS[2]]),
        ("two stranded columns", [COLUMNS[0], COLUMNS[1], COLUMNS[2]]),
        ("three stranded columns", [COLUMNS[4], COLUMNS[1], COLUMNS[2]]),
    ];

    let mut row = 0;
    for rule_type in ALL_ANALYSIS_RULE_TYPES {
        for (assignment, roles) in assignments {
            let (policy, referenced) = policy_over(rule_type, roles);
            let expected = stranded(&referenced, &allowed);
            assert!(
                !expected.is_empty(),
                "case {assignment:?}: this table's rows must strand at least one column"
            );

            // CreateConfiguredTableAnalysisRule: the policy names columns the table does
            // not expose.
            row += 1;
            let label = format!("create a {rule_type} rule with {assignment}");
            let table_id = table_over(&emulator, &format!("policy-refusal-{row}"), &allowed).await;
            let (status, response) = emulator
                .create_analysis_rule(&table_id, rule_type, policy.clone())
                .await;
            assert_column_reference_refusal(
                &label,
                &table_id,
                rule_type,
                &expected,
                status,
                &response,
            );
            let (status, read) = emulator.get_analysis_rule(&table_id, rule_type).await;
            assert_eq!(
                status, 404,
                "case {label:?}: the refused create must persist no rule, got {status} with {read}"
            );

            // UpdateConfiguredTableAnalysisRule: the same policy against a table that
            // already holds a rule over the columns it does expose.
            row += 1;
            let label = format!("update a {rule_type} rule to {assignment}");
            let table_id = table_over(&emulator, &format!("policy-refusal-{row}"), &allowed).await;
            let (stored_policy, _) =
                policy_over(rule_type, [allowed[0], allowed[1], allowed[0]]);
            let (status, created) = emulator
                .create_analysis_rule(&table_id, rule_type, stored_policy)
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected CreateConfiguredTableAnalysisRule 200, got {status} with {created}"
            );
            let stored_rule = created["analysisRule"].clone();

            let (status, response) = emulator
                .update_analysis_rule(&table_id, rule_type, policy.clone())
                .await;
            assert_column_reference_refusal(
                &label,
                &table_id,
                rule_type,
                &expected,
                status,
                &response,
            );
            let (status, read) = emulator.get_analysis_rule(&table_id, rule_type).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the stored rule to still read back, got {status} with {read}"
            );
            assert_eq!(
                read["analysisRule"], stored_rule,
                "case {label:?}: the refused update must leave the rule at {stored_rule}, got {}",
                read["analysisRule"]
            );
        }
    }
}

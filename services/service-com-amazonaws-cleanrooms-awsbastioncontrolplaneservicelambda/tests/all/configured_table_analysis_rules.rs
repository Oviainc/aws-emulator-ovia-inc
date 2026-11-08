// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Clean Rooms configured-table analysis-rule slice
//! (`CreateConfiguredTableAnalysisRule` + `GetConfiguredTableAnalysisRule`).
//!
//! The rules under test:
//!
//! * a create persists the rule and mints every member the SERVICE decides from the
//!   STORED configured table the identifier resolves to (`configuredTableId`,
//!   `configuredTableArn`, `createTime`/`updateTime`), reports the `type` and the
//!   `policy` the caller decided unchanged, and records the type in the configured
//!   table's `analysisRuleTypes`; a later read reports exactly the persisted rule and
//!   changes nothing;
//! * each member of the `ConfiguredTableAnalysisRulePolicyV1` union is the policy of
//!   exactly one `ConfiguredTableAnalysisRuleType`, so a request whose `analysisRuleType`
//!   names another type is refused with a `ValidationException` and persists nothing;
//! * `CreateConfiguredTableAnalysisRule`: "Currently, only one analysis rule can be
//!   created for a given configured table" — a further create fails that input constraint
//!   and is a `ValidationException` (400) whatever type it asks for, stating the constraint
//!   and the type of the rule the table already holds, and leaves the stored rule alone;
//! * a configured table / analysis rule type pair the store holds no rule for is a
//!   `ResourceNotFoundException` ("Configured table analysis rules are uniquely
//!   identified by their configured table identifier and analysis rule type").
//!
//! The tables span every analysis rule type crossed with the configured table the rule
//! hangs off, and vary what each policy carries (which columns, how many, which
//! functions, which optional members), so no row is privileged. Every request goes over
//! the wire through the same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{account_id, error_code, error_message, Emulator};
use super::cleanrooms_configured_table::{
    create_body, ALL_ANALYSIS_METHODS, ALL_TABLE_REFERENCE_TYPES, COLUMN_NAMES,
};

/// Every `ConfiguredTableAnalysisRuleType` the model declares (shape
/// `ConfiguredTableAnalysisRuleType`: AGGREGATION | LIST | CUSTOM), which is also one per
/// member of the `ConfiguredTableAnalysisRulePolicyV1` union.
pub(crate) const ALL_ANALYSIS_RULE_TYPES: [&str; 3] = ["AGGREGATION", "LIST", "CUSTOM"];

/// Every `AggregateFunctionName` the model declares (shape `AggregateFunctionName`).
const ALL_AGGREGATE_FUNCTIONS: [&str; 5] = ["SUM", "SUM_DISTINCT", "COUNT", "COUNT_DISTINCT", "AVG"];

/// Some of the `ScalarFunctions` the model declares (shape `ScalarFunctions`), used to
/// vary how many scalar functions a policy allows.
const SCALAR_FUNCTIONS: [&str; 4] = ["ABS", "CEILING", "FLOOR", "TRUNC"];

/// Every `AdditionalAnalyses` the model declares (shape `AdditionalAnalyses`).
const ALL_ADDITIONAL_ANALYSES: [&str; 3] = ["ALLOWED", "REQUIRED", "NOT_ALLOWED"];

/// Every `JoinOperator` the model declares (shape `JoinOperator`).
const ALL_JOIN_OPERATORS: [&str; 2] = ["OR", "AND"];

/// The published bounds of `AggregationConstraint$minimum` (model
/// `AggregationConstraint$minimum`, range 2..100000).
const MIN_AGGREGATION_MINIMUM: i64 = 2;
const MAX_AGGREGATION_MINIMUM: i64 = 100_000;

/// The `ConfiguredTableAnalysisRulePolicy` for one rule type, varied by `row` so the
/// rows of a table differ in which columns a policy names, how many, which functions it
/// allows and which optional members it carries.
fn analysis_rule_policy(rule_type: &str, row: usize) -> Value {
    let v1 = match rule_type {
        // `AnalysisRuleAggregation`: aggregateColumns (len 1..), joinColumns,
        // dimensionColumns, scalarFunctions and outputConstraints (len 1..) are required.
        "AGGREGATION" => json!({
            "aggregation": {
                "aggregateColumns": [{
                    "columnNames": COLUMN_NAMES[1..2 + row % 2].to_vec(),
                    "function": ALL_AGGREGATE_FUNCTIONS[row % ALL_AGGREGATE_FUNCTIONS.len()],
                }],
                "joinColumns": [COLUMN_NAMES[0]],
                "joinRequired": "QUERY_RUNNER",
                "dimensionColumns": COLUMN_NAMES[2..].to_vec(),
                "scalarFunctions": SCALAR_FUNCTIONS[..1 + row % SCALAR_FUNCTIONS.len()].to_vec(),
                "outputConstraints": [{
                    "columnName": COLUMN_NAMES[2],
                    "minimum": MIN_AGGREGATION_MINIMUM + row as i64,
                    "type": "COUNT_DISTINCT",
                }],
            }
        }),
        // `AnalysisRuleList`: joinColumns (len 1..) and listColumns are required;
        // allowedJoinOperators and additionalAnalyses are optional.
        "LIST" => json!({
            "list": {
                "joinColumns": [COLUMN_NAMES[0]],
                "allowedJoinOperators": ALL_JOIN_OPERATORS[..1 + row % 2].to_vec(),
                "listColumns": COLUMN_NAMES[1..].to_vec(),
                "additionalAnalyses": ALL_ADDITIONAL_ANALYSES[row % ALL_ADDITIONAL_ANALYSES.len()],
            }
        }),
        // `AnalysisRuleCustom`: allowedAnalyses is required; the provider list is
        // "Required when allowedAnalyses is ANY_QUERY" and carries 12-digit account ids.
        "CUSTOM" => json!({
            "custom": {
                "allowedAnalyses": ["ANY_QUERY"],
                "allowedAnalysisProviders": [account_id()],
                "additionalAnalyses": ALL_ADDITIONAL_ANALYSES[row % ALL_ADDITIONAL_ANALYSES.len()],
                "disallowedOutputColumns": COLUMN_NAMES[..row % 2].to_vec(),
            }
        }),
        other => panic!("no ConfiguredTableAnalysisRulePolicyV1 member for rule type {other:?}"),
    };
    json!({ "v1": v1 })
}

impl Emulator {
    pub(crate) async fn create_analysis_rule(
        &self,
        configured_table_id: &str,
        rule_type: &str,
        policy: Value,
    ) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/configuredTables/{configured_table_id}/analysisRule"),
            Some(json!({ "analysisRuleType": rule_type, "analysisRulePolicy": policy })),
        )
        .await
    }

    pub(crate) async fn get_analysis_rule(&self, configured_table_id: &str, rule_type: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/configuredTables/{configured_table_id}/analysisRule/{rule_type}"),
            None,
        )
        .await
    }

    /// A fresh configured table to hang analysis rules off, and its `(id, arn)`.
    async fn configured_table(&self, row: usize) -> (String, String) {
        let (status, created) = self
            .create_configured_table(create_body(
                &format!("analysis-rule-table-{row}"),
                ALL_TABLE_REFERENCE_TYPES[row % ALL_TABLE_REFERENCE_TYPES.len()],
                ALL_ANALYSIS_METHODS[row % ALL_ANALYSIS_METHODS.len()],
                None,
                COLUMN_NAMES.len(),
            ))
            .await;
        assert_eq!(
            status, 200,
            "row {row}: expected CreateConfiguredTable 200, got {status} with {created}"
        );
        let table = &created["configuredTable"];
        (
            table["id"].as_str().unwrap_or_default().to_string(),
            table["arn"].as_str().unwrap_or_default().to_string(),
        )
    }
}

#[tokio::test]
async fn create_configured_table_analysis_rule_persists_the_rule_a_later_read_reports() {
    let emulator = Emulator::new();

    // Every rule type, twice, so the policy variation (`row`) differs between the two
    // tables that carry the same type.
    for (row, rule_type) in ALL_ANALYSIS_RULE_TYPES
        .iter()
        .chain(ALL_ANALYSIS_RULE_TYPES.iter())
        .enumerate()
    {
        let label = format!("row {row}: analysisRuleType={rule_type}");
        let (table_id, table_arn) = emulator.configured_table(row).await;
        let policy = analysis_rule_policy(rule_type, row);

        let (status, created) = emulator
            .create_analysis_rule(&table_id, rule_type, policy.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateConfiguredTableAnalysisRule 200, got {status} with {created}"
        );
        let rule = &created["analysisRule"];

        // What the caller decided is reported back unchanged.
        assert_eq!(
            rule["type"].as_str(),
            Some(*rule_type),
            "case {label:?}: expected type {rule_type:?}, got {rule}"
        );
        assert_eq!(
            rule["policy"], policy,
            "case {label:?}: expected policy {policy}, got {}",
            rule["policy"]
        );

        // What the SERVICE decides comes from the stored configured table.
        assert_eq!(
            rule["configuredTableId"].as_str(),
            Some(table_id.as_str()),
            "case {label:?}: expected configuredTableId {table_id:?}, got {rule}"
        );
        assert_eq!(
            rule["configuredTableArn"].as_str(),
            Some(table_arn.as_str()),
            "case {label:?}: expected configuredTableArn {table_arn:?}, got {rule}"
        );
        assert_eq!(
            rule["createTime"], rule["updateTime"],
            "case {label:?}: a rule never updated reports createTime == updateTime, got {} and {}",
            rule["createTime"], rule["updateTime"]
        );

        // The read reports the persisted rule, and reading does not change it.
        let (status, read) = emulator.get_analysis_rule(&table_id, rule_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetConfiguredTableAnalysisRule 200, got {status} with {read}"
        );
        assert_eq!(
            read["analysisRule"], *rule,
            "case {label:?}: the read must report the created rule {rule}, got {}",
            read["analysisRule"]
        );
        let (status, reread) = emulator.get_analysis_rule(&table_id, rule_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a second GetConfiguredTableAnalysisRule 200, got {status} with {reread}"
        );
        assert_eq!(
            reread, read,
            "case {label:?}: a read must not change what a later read sees; first {read}, then {reread}"
        );

        // The configured table now reports the type of the rule it holds, and nothing
        // else about it moved.
        let (status, table) = emulator.get_configured_table(&table_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetConfiguredTable 200, got {status} with {table}"
        );
        assert_eq!(
            table["configuredTable"]["analysisRuleTypes"], json!([rule_type]),
            "case {label:?}: expected analysisRuleTypes [{rule_type:?}], got {}",
            table["configuredTable"]["analysisRuleTypes"]
        );
        assert_eq!(
            table["configuredTable"]["arn"].as_str(),
            Some(table_arn.as_str()),
            "case {label:?}: expected the table's arn {table_arn:?} to be untouched, got {}",
            table["configuredTable"]
        );
    }
}

#[tokio::test]
async fn analysis_rule_policy_must_carry_the_policy_of_the_declared_type() {
    let emulator = Emulator::new();

    // Every (declared type, policy type) pair the catalog admits: the matching rows are
    // accepted, the mismatched ones refused.
    let mut row = 0;
    for declared in ALL_ANALYSIS_RULE_TYPES {
        for policy_type in ALL_ANALYSIS_RULE_TYPES {
            let label = format!("analysisRuleType={declared}, policy={policy_type}");
            let (table_id, _) = emulator.configured_table(row).await;
            let policy = analysis_rule_policy(policy_type, row);
            let (status, response) = emulator
                .create_analysis_rule(&table_id, declared, policy)
                .await;
            row += 1;

            if declared == policy_type {
                assert_eq!(
                    status, 200,
                    "case {label:?}: a policy of the declared type must be accepted, got {status} with {response}"
                );
                continue;
            }

            assert_eq!(
                status, 400,
                "case {label:?}: a policy of another type must be refused with 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {response}"
            );
            assert_eq!(
                response["fieldList"][0]["name"].as_str(),
                Some("analysisRulePolicy"),
                "case {label:?}: expected the fieldList to name analysisRulePolicy, got {response}"
            );
            assert!(
                response.get("analysisRule").is_none(),
                "case {label:?}: a refused create must report no analysis rule, got {response}"
            );

            // Nothing was persisted: neither the declared type nor the policy's type is
            // readable, and the table still holds no analysis rule.
            for rule_type in [declared, policy_type] {
                let (status, read) = emulator.get_analysis_rule(&table_id, rule_type).await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: a refused create persists nothing, so {rule_type} must be 404, got {status} with {read}"
                );
            }
            let (status, table) = emulator.get_configured_table(&table_id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetConfiguredTable 200, got {status} with {table}"
            );
            assert_eq!(
                table["configuredTable"]["analysisRuleTypes"], json!([]),
                "case {label:?}: a refused create leaves analysisRuleTypes empty, got {}",
                table["configuredTable"]["analysisRuleTypes"]
            );
        }
    }
}

#[tokio::test]
async fn only_one_analysis_rule_can_be_created_for_a_configured_table() {
    let emulator = Emulator::new();

    // Every (first type, second type) pair, so the refusal covers a repeat of the SAME
    // type and a request for a different one.
    let mut row = 0;
    for first in ALL_ANALYSIS_RULE_TYPES {
        for second in ALL_ANALYSIS_RULE_TYPES {
            let label = format!("first={first}, second={second}");
            let (table_id, _) = emulator.configured_table(row).await;
            let first_policy = analysis_rule_policy(first, row);
            let second_policy = analysis_rule_policy(second, row + 1);
            row += 1;

            let (status, created) = emulator
                .create_analysis_rule(&table_id, first, first_policy)
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the first create to succeed, got {status} with {created}"
            );
            let stored = created["analysisRule"].clone();

            let (status, response) = emulator
                .create_analysis_rule(&table_id, second, second_policy)
                .await;
            assert_eq!(
                status, 400,
                "case {label:?}: a second analysis rule must be refused with 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {response}"
            );
            // The sentence states the constraint and what the stored table already holds:
            // the table the request addressed, and the type of the rule ON it — which is
            // the FIRST type even when the second create asks for another one.
            assert_eq!(
                error_message(&response),
                format!(
                    "Only one Analysis Rule is allowed per Configured Table Configured Table \
                     {table_id} already has a {first} analysis rule."
                ),
                "case {label:?}: the message must name the addressed table and the type it \
                 already holds, got {:?}",
                error_message(&response)
            );
            assert_eq!(
                response["reason"].as_str(),
                Some("INVALID_CONFIGURATION"),
                "case {label:?}: expected reason INVALID_CONFIGURATION, got {response}"
            );

            // The refused create left the stored rule and its table exactly as they were.
            let (status, read) = emulator.get_analysis_rule(&table_id, first).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the stored {first} rule to still read back, got {status} with {read}"
            );
            assert_eq!(
                read["analysisRule"], stored,
                "case {label:?}: the stored rule must still be {stored}, got {}",
                read["analysisRule"]
            );
            let (status, table) = emulator.get_configured_table(&table_id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetConfiguredTable 200, got {status} with {table}"
            );
            assert_eq!(
                table["configuredTable"]["analysisRuleTypes"], json!([first]),
                "case {label:?}: expected analysisRuleTypes [{first:?}], got {}",
                table["configuredTable"]["analysisRuleTypes"]
            );
        }
    }
}

#[tokio::test]
async fn an_analysis_rule_no_create_minted_is_not_found() {
    let emulator = Emulator::new();

    // A table that holds no rule at all: every type is 404 on it.
    let (empty_table, _) = emulator.configured_table(0).await;
    for rule_type in ALL_ANALYSIS_RULE_TYPES {
        let label = format!("table with no rules, analysisRuleType={rule_type}");
        let (status, response) = emulator.get_analysis_rule(&empty_table, rule_type).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected GetConfiguredTableAnalysisRule 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            response["resourceId"].as_str(),
            Some(empty_table.as_str()),
            "case {label:?}: expected resourceId {empty_table:?}, got {response}"
        );
        assert_eq!(
            response["resourceType"].as_str(),
            Some("CONFIGURED_TABLE"),
            "case {label:?}: expected resourceType CONFIGURED_TABLE, got {response}"
        );
    }

    // A table that holds ONE rule: the other two types are still 404 on it, so the read
    // is keyed on the (table, type) pair and not on the table alone.
    let (one_rule_table, _) = emulator.configured_table(1).await;
    let held = ALL_ANALYSIS_RULE_TYPES[0];
    let (status, created) = emulator
        .create_analysis_rule(&one_rule_table, held, analysis_rule_policy(held, 1))
        .await;
    assert_eq!(
        status, 200,
        "expected the {held} create to succeed, got {status} with {created}"
    );
    for rule_type in ALL_ANALYSIS_RULE_TYPES.into_iter().skip(1) {
        let (status, response) = emulator.get_analysis_rule(&one_rule_table, rule_type).await;
        assert_eq!(
            status, 404,
            "case {rule_type:?}: a table holding only {held} must answer 404 for it, got {status} with {response}"
        );
    }

    // Identifiers in the form the model's `ConfiguredTableIdentifier` requires that no
    // create minted: the two extremes of the space and one in between.
    let unheld = [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "deadbeef-0000-4000-8000-000000000001",
    ];
    for table_id in unheld {
        assert_ne!(
            table_id, empty_table,
            "the unheld identifiers must not include a created one"
        );
        for rule_type in ALL_ANALYSIS_RULE_TYPES {
            let label = format!("unheld table {table_id}, analysisRuleType={rule_type}");
            let (status, response) = emulator.get_analysis_rule(table_id, rule_type).await;
            assert_eq!(
                status, 404,
                "case {label:?}: expected GetConfiguredTableAnalysisRule 404, got {status} with {response}"
            );

            let (status, response) = emulator
                .create_analysis_rule(table_id, rule_type, analysis_rule_policy(rule_type, 0))
                .await;
            assert_eq!(
                status, 404,
                "case {label:?}: creating a rule on an unheld table must be 404, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {label:?}: expected ResourceNotFoundException, got {response}"
            );
            assert_eq!(
                response["resourceId"].as_str(),
                Some(table_id),
                "case {label:?}: expected resourceId {table_id:?}, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn aggregation_output_constraint_minimum_is_bounded() {
    let emulator = Emulator::new();

    // `AggregationConstraint$minimum` is published as range 2..100000: both bounds are
    // accepted, one past each is refused.
    let rows: [(i64, bool); 6] = [
        (MIN_AGGREGATION_MINIMUM, true),
        (MIN_AGGREGATION_MINIMUM + 1, true),
        (MAX_AGGREGATION_MINIMUM - 1, true),
        (MAX_AGGREGATION_MINIMUM, true),
        (MIN_AGGREGATION_MINIMUM - 1, false),
        (MAX_AGGREGATION_MINIMUM + 1, false),
    ];

    for (row, (minimum, accepted)) in rows.into_iter().enumerate() {
        let label = format!("outputConstraints[0].minimum={minimum}");
        let (table_id, _) = emulator.configured_table(row).await;
        let mut policy = analysis_rule_policy("AGGREGATION", row);
        policy["v1"]["aggregation"]["outputConstraints"][0]["minimum"] = json!(minimum);

        let (status, response) = emulator
            .create_analysis_rule(&table_id, "AGGREGATION", policy)
            .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: a minimum inside [{MIN_AGGREGATION_MINIMUM}, {MAX_AGGREGATION_MINIMUM}] must be accepted, got {status} with {response}"
            );
            assert_eq!(
                response["analysisRule"]["policy"]["v1"]["aggregation"]["outputConstraints"][0]
                    ["minimum"],
                json!(minimum),
                "case {label:?}: expected the stored minimum {minimum}, got {}",
                response["analysisRule"]["policy"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: a minimum outside [{MIN_AGGREGATION_MINIMUM}, {MAX_AGGREGATION_MINIMUM}] must be refused with 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {response}"
            );
            let (status, read) = emulator.get_analysis_rule(&table_id, "AGGREGATION").await;
            assert_eq!(
                status, 404,
                "case {label:?}: a refused create persists nothing, got {status} with {read}"
            );
        }
    }
}

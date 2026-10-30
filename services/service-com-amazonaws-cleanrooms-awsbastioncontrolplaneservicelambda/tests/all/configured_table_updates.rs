// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateConfiguredTable`, read back through
//! `GetConfiguredTable`.
//!
//! The rules under test:
//!
//! * the update is a PATCH over the stored `ConfiguredTable`: every member the request
//!   carries replaces the stored one, every member it omits keeps its stored value, the
//!   members the service decides (`id`, `arn`, `createTime`, `analysisRuleTypes`) do not
//!   move, `tableType` follows the `TableReference` variant the table points at, and
//!   `updateTime` is restamped;
//! * `allowedColumns` are "the columns of the underlying table that can be used by
//!   collaborations or analysis rules", so the list an update leaves on the table must
//!   still carry every column its analysis rule references: dropping one is refused with
//!   a `ValidationException` and persists nothing, while dropping a column no rule names
//!   is accepted. The same rule refuses a `CreateConfiguredTableAnalysisRule` whose
//!   policy names a column the table does not expose;
//! * `selectedAnalysisMethods`, "when configured, you must specify at least two analysis
//!   methods", is refused when it configures fewer;
//! * an identifier the store does not hold is a `ResourceNotFoundException`.
//!
//! Each table varies what the rules generalise over — which member the request carries,
//! which `TableReference` variant, every `AnalysisMethod`, the number and the order of
//! allowed columns and of selected analysis methods, and every
//! `ConfiguredTableAnalysisRuleType` crossed with each column its policy references — so
//! no row is privileged. Every request goes over the wire through the same
//! `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{account_id, error_code, error_message, Emulator};
use super::cleanrooms_configured_table::{
    table_reference, ALL_ANALYSIS_METHODS, ALL_SELECTED_ANALYSIS_METHODS,
    ALL_TABLE_REFERENCE_TYPES, COLUMN_NAMES, MIN_SELECTED_ANALYSIS_METHODS,
};
use super::configured_table_analysis_rules::ALL_ANALYSIS_RULE_TYPES;

/// The columns these tables configure: the suites' shared `COLUMN_NAMES` catalog plus one
/// more column that none of the analysis-rule policies below names, so a row can drop a
/// column the table's rule does not reference. `ColumnName` matches
/// `^[a-z0-9_](([a-z0-9_ ]+-)*([a-z0-9_ ]+))?$`, so the added one is a valid one.
const COLUMNS: [&str; 4] = [
    COLUMN_NAMES[0],
    COLUMN_NAMES[1],
    COLUMN_NAMES[2],
    "unreferenced_column",
];

impl Emulator {
    async fn update_configured_table(&self, id: &str, body: Value) -> (u16, Value) {
        self.call("PATCH", &format!("/configuredTables/{id}"), Some(body))
            .await
    }

    /// A configured table over `columns`, with `analysis_method`, and the resource the
    /// create persisted.
    async fn table_over(
        &self,
        name: &str,
        table_type: &str,
        analysis_method: &str,
        columns: &[&str],
    ) -> Value {
        let (status, created) = self
            .create_configured_table(json!({
                "name": name,
                "tableReference": table_reference(table_type),
                "allowedColumns": columns.to_vec(),
                "analysisMethod": analysis_method,
            }))
            .await;
        assert_eq!(
            status, 200,
            "setup {name:?}: expected CreateConfiguredTable 200, got {status} with {created}"
        );
        created["configuredTable"].clone()
    }
}

/// A policy for `rule_type` over `columns` (one column per role the policy fills), and
/// the columns it references.
///
/// Which members of a policy name a column comes from the model: `AnalysisRuleList`'s
/// `joinColumns` and `listColumns`; `AnalysisRuleAggregation`'s
/// `aggregateColumns[].columnNames`, `joinColumns`, `dimensionColumns` and
/// `outputConstraints[].columnName`; `AnalysisRuleCustom`'s `disallowedOutputColumns` and
/// `differentialPrivacy.columns[].name`.
fn analysis_rule_case(rule_type: &str) -> (Value, Vec<&'static str>) {
    let (join, aggregate, dimension) = (COLUMNS[0], COLUMNS[1], COLUMNS[2]);
    match rule_type {
        "LIST" => (
            json!({ "v1": { "list": {
                "joinColumns": [join],
                "listColumns": [aggregate],
                "allowedJoinOperators": ["OR"],
            }}}),
            vec![join, aggregate],
        ),
        "AGGREGATION" => (
            json!({ "v1": { "aggregation": {
                "aggregateColumns": [{ "columnNames": [aggregate], "function": "SUM" }],
                "joinColumns": [join],
                "dimensionColumns": [dimension],
                "scalarFunctions": ["ABS"],
                "outputConstraints": [{
                    "columnName": dimension,
                    "minimum": 2,
                    "type": "COUNT_DISTINCT",
                }],
            }}}),
            vec![aggregate, join, dimension],
        ),
        "CUSTOM" => (
            json!({ "v1": { "custom": {
                "allowedAnalyses": ["ANY_QUERY"],
                "allowedAnalysisProviders": [account_id()],
                "disallowedOutputColumns": [dimension],
                "differentialPrivacy": { "columns": [{ "name": join }] },
            }}}),
            vec![dimension, join],
        ),
        other => panic!("no ConfiguredTableAnalysisRulePolicyV1 member for rule type {other:?}"),
    }
}

/// The `createTime`/`updateTime` of a configured table, as the wire reports them.
fn timestamps(table: &Value) -> (f64, f64) {
    let read = |member: &str| {
        table[member]
            .as_f64()
            .unwrap_or_else(|| panic!("{member} must be a timestamp, got {table}"))
    };
    (read("createTime"), read("updateTime"))
}

/// One row of the PATCH table: the body to send and the members it must leave behind.
struct PatchCase {
    label: &'static str,
    /// The update request body.
    body: Value,
    /// `(member, expected value)` for every member the update must have moved.
    expected: Vec<(&'static str, Value)>,
}

fn patch_cases() -> Vec<PatchCase> {
    let both = ALL_SELECTED_ANALYSIS_METHODS.to_vec();
    let both_reversed = vec![
        ALL_SELECTED_ANALYSIS_METHODS[1],
        ALL_SELECTED_ANALYSIS_METHODS[0],
    ];
    let mut cases = vec![
        PatchCase {
            label: "name only",
            body: json!({ "name": "renamed-table" }),
            expected: vec![("name", json!("renamed-table"))],
        },
        PatchCase {
            label: "description only",
            body: json!({ "description": "a new description" }),
            expected: vec![("description", json!("a new description"))],
        },
        PatchCase {
            label: "allowedColumns widened to every column",
            body: json!({ "allowedColumns": COLUMNS.to_vec() }),
            expected: vec![("allowedColumns", json!(COLUMNS.to_vec()))],
        },
        PatchCase {
            label: "allowedColumns narrowed to one column",
            body: json!({ "allowedColumns": [COLUMNS[0]] }),
            expected: vec![("allowedColumns", json!([COLUMNS[0]]))],
        },
        PatchCase {
            label: "allowedColumns reordered",
            body: json!({ "allowedColumns": [COLUMNS[2], COLUMNS[0], COLUMNS[1]] }),
            expected: vec![(
                "allowedColumns",
                json!([COLUMNS[2], COLUMNS[0], COLUMNS[1]]),
            )],
        },
        PatchCase {
            label: "selectedAnalysisMethods, both methods",
            body: json!({ "selectedAnalysisMethods": both.clone() }),
            expected: vec![("selectedAnalysisMethods", json!(both))],
        },
        PatchCase {
            label: "selectedAnalysisMethods, both methods reversed",
            body: json!({ "selectedAnalysisMethods": both_reversed.clone() }),
            expected: vec![("selectedAnalysisMethods", json!(both_reversed))],
        },
        PatchCase {
            label: "every member at once",
            body: json!({
                "name": "fully-updated",
                "description": "every member moved",
                "allowedColumns": [COLUMNS[1], COLUMNS[3]],
                "analysisMethod": "MULTIPLE",
                "selectedAnalysisMethods": ALL_SELECTED_ANALYSIS_METHODS.to_vec(),
                "tableReference": table_reference(ALL_TABLE_REFERENCE_TYPES[2]),
            }),
            expected: vec![
                ("name", json!("fully-updated")),
                ("description", json!("every member moved")),
                ("allowedColumns", json!([COLUMNS[1], COLUMNS[3]])),
                ("analysisMethod", json!("MULTIPLE")),
                (
                    "selectedAnalysisMethods",
                    json!(ALL_SELECTED_ANALYSIS_METHODS.to_vec()),
                ),
                (
                    "tableReference",
                    table_reference(ALL_TABLE_REFERENCE_TYPES[2]),
                ),
                ("tableType", json!(ALL_TABLE_REFERENCE_TYPES[2])),
            ],
        },
        PatchCase {
            label: "no member at all",
            body: json!({}),
            expected: Vec::new(),
        },
    ];
    // Every `AnalysisMethod`, and every `TableReference` variant: the `tableType` the
    // table reports must follow the variant it points at.
    for analysis_method in ALL_ANALYSIS_METHODS {
        cases.push(PatchCase {
            label: "analysisMethod only",
            body: json!({ "analysisMethod": analysis_method }),
            expected: vec![("analysisMethod", json!(analysis_method))],
        });
    }
    for table_type in ALL_TABLE_REFERENCE_TYPES {
        cases.push(PatchCase {
            label: "tableReference only",
            body: json!({ "tableReference": table_reference(table_type) }),
            expected: vec![
                ("tableReference", table_reference(table_type)),
                ("tableType", json!(table_type)),
            ],
        });
    }
    cases
}

#[tokio::test]
async fn update_configured_table_patches_only_the_members_the_request_carries() {
    let emulator = Emulator::new();

    for (row, case) in patch_cases().into_iter().enumerate() {
        let label = format!("row {row}: {}", case.label);
        // The table every row starts from: a description set, three of the four columns,
        // no selected analysis methods, and the first `TableReference` variant.
        let (status, created) = emulator
            .create_configured_table(json!({
                "name": format!("patch-{row}"),
                "description": "the description the create set",
                "tableReference": table_reference(ALL_TABLE_REFERENCE_TYPES[0]),
                "allowedColumns": COLUMNS[..3].to_vec(),
                "analysisMethod": "DIRECT_QUERY",
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateConfiguredTable 200, got {status} with {created}"
        );
        let before = created["configuredTable"].clone();
        let id = before["id"].as_str().unwrap_or_default().to_string();

        let (status, response) = emulator.update_configured_table(&id, case.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected UpdateConfiguredTable 200 for {}, got {status} with {response}",
            case.body
        );
        let after = response["configuredTable"].clone();

        // Every member the request carried moved to the requested value.
        for (member, expected) in &case.expected {
            assert_eq!(
                after[member], *expected,
                "case {label:?}: expected {member} {expected}, got {}",
                after[member]
            );
        }
        // Every member it did NOT carry kept the value the create persisted.
        let moved: Vec<&str> = case.expected.iter().map(|(member, _)| *member).collect();
        for member in [
            "name",
            "description",
            "allowedColumns",
            "analysisMethod",
            "selectedAnalysisMethods",
            "tableReference",
            "tableType",
        ] {
            if moved.contains(&member) {
                continue;
            }
            assert_eq!(
                after[member], before[member],
                "case {label:?}: {member} was not in the request, so it must still be {}, got {}",
                before[member], after[member]
            );
        }

        // The members the SERVICE decides do not move, and the update restamps
        // `updateTime` ("The time the configured table was last updated").
        for member in ["id", "arn", "createTime", "analysisRuleTypes"] {
            assert_eq!(
                after[member], before[member],
                "case {label:?}: {member} is the service's and must stay {}, got {}",
                before[member], after[member]
            );
        }
        // The update restamps `updateTime`, so it stands strictly after the
        // `createTime` the create minted (and after the stamp the table carried before
        // the call, which for a never-updated table is that same instant): a handler
        // that left the member alone would report the two equal.
        let (create_time, update_time) = timestamps(&after);
        let (_, update_time_before) = timestamps(&before);
        assert!(
            update_time > create_time,
            "case {label:?}: expected updateTime to be restamped after createTime {create_time}, got {update_time}"
        );
        assert!(
            update_time > update_time_before,
            "case {label:?}: expected updateTime to be restamped after {update_time_before}, got {update_time}"
        );

        // The read reports exactly what the update persisted, and reading changes
        // nothing.
        let (status, read) = emulator.get_configured_table(&id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetConfiguredTable 200 for {id}, got {status} with {read}"
        );
        assert_eq!(
            read["configuredTable"], after,
            "case {label:?}: the read must report the updated table {after}, got {}",
            read["configuredTable"]
        );
    }
}

#[tokio::test]
async fn update_configured_table_keeps_every_column_its_analysis_rule_references() {
    let emulator = Emulator::new();

    for rule_type in ALL_ANALYSIS_RULE_TYPES {
        let (policy, referenced) = analysis_rule_case(rule_type);

        // One row per column the policy references: the narrowed list keeps every other
        // column, so the only reason to refuse it is the one column it drops.
        for dropped in referenced.iter().copied() {
            let label = format!("{rule_type} rule, dropping referenced column {dropped:?}");
            let table = emulator
                .table_over(
                    &format!("rule-{rule_type}-{dropped}"),
                    ALL_TABLE_REFERENCE_TYPES[0],
                    "DIRECT_QUERY",
                    &COLUMNS,
                )
                .await;
            let id = table["id"].as_str().unwrap_or_default().to_string();
            let (status, rule) = emulator
                .create_analysis_rule(&id, rule_type, policy.clone())
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected CreateConfiguredTableAnalysisRule 200, got {status} with {rule}"
            );
            // The table as it stands WITH the rule: creating one records its type in
            // `analysisRuleTypes` and restamps the table, so this is the state a refused
            // update must leave behind.
            let (status, read) = emulator.get_configured_table(&id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetConfiguredTable 200 for {id}, got {status} with {read}"
            );
            let with_rule = read["configuredTable"].clone();

            let kept: Vec<&str> = COLUMNS.iter().copied().filter(|c| *c != dropped).collect();
            let (status, response) = emulator
                .update_configured_table(&id, json!({ "allowedColumns": kept.clone() }))
                .await;
            assert_eq!(
                status, 400,
                "case {label:?}: a column the analysis rule references must not be removable, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {response}"
            );
            assert!(
                error_message(&response).contains(dropped),
                "case {label:?}: the refusal must name the column it is about, got {:?}",
                error_message(&response)
            );
            // A refused update persists nothing.
            let (status, read) = emulator.get_configured_table(&id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetConfiguredTable 200 for {id}, got {status} with {read}"
            );
            assert_eq!(
                read["configuredTable"], with_rule,
                "case {label:?}: the refused update must leave the table at {with_rule}, got {}",
                read["configuredTable"]
            );

            // Dropping only the columns NO rule references is accepted, and so is the
            // list that keeps exactly the referenced ones.
            let unreferenced: Vec<&str> = COLUMNS
                .iter()
                .copied()
                .filter(|c| !referenced.contains(c))
                .collect();
            let accepted: Vec<Vec<&str>> = vec![
                COLUMNS
                    .iter()
                    .copied()
                    .filter(|c| !unreferenced.contains(c))
                    .collect(),
                COLUMNS.to_vec(),
            ];
            for columns in accepted {
                let (status, response) = emulator
                    .update_configured_table(&id, json!({ "allowedColumns": columns.clone() }))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: allowedColumns {columns:?} keeps every referenced column, so it must be accepted, got {status} with {response}"
                );
                assert_eq!(
                    response["configuredTable"]["allowedColumns"], json!(columns),
                    "case {label:?}: expected allowedColumns {columns:?}, got {}",
                    response["configuredTable"]["allowedColumns"]
                );
            }
        }
    }

    // The refusal is about the analysis rule, not about narrowing: a table that holds no
    // rule can be narrowed to a single column.
    let table = emulator
        .table_over("no-rule", ALL_TABLE_REFERENCE_TYPES[1], "MULTIPLE", &COLUMNS)
        .await;
    let id = table["id"].as_str().unwrap_or_default().to_string();
    let (status, response) = emulator
        .update_configured_table(&id, json!({ "allowedColumns": [COLUMNS[3]] }))
        .await;
    assert_eq!(
        status, 200,
        "a table with no analysis rule must accept any allowedColumns, got {status} with {response}"
    );
    assert_eq!(
        response["configuredTable"]["allowedColumns"], json!([COLUMNS[3]]),
        "expected allowedColumns {:?}, got {}",
        [COLUMNS[3]],
        response["configuredTable"]["allowedColumns"]
    );
}

#[tokio::test]
async fn create_configured_table_analysis_rule_refuses_a_policy_over_an_unexposed_column() {
    let emulator = Emulator::new();

    // The same rule — every column an analysis rule names has to be one of the table's
    // allowedColumns — at the other door: the policy is fixed and the table's column list
    // is what varies, so each row drops exactly one column the policy references.
    for rule_type in ALL_ANALYSIS_RULE_TYPES {
        let (policy, referenced) = analysis_rule_case(rule_type);
        for missing in referenced.iter().copied() {
            let label = format!("{rule_type} policy over a table without {missing:?}");
            let exposed: Vec<&str> = COLUMNS.iter().copied().filter(|c| *c != missing).collect();
            let table = emulator
                .table_over(
                    &format!("unexposed-{rule_type}-{missing}"),
                    ALL_TABLE_REFERENCE_TYPES[0],
                    "DIRECT_QUERY",
                    &exposed,
                )
                .await;
            let id = table["id"].as_str().unwrap_or_default().to_string();

            let (status, response) = emulator
                .create_analysis_rule(&id, rule_type, policy.clone())
                .await;
            assert_eq!(
                status, 400,
                "case {label:?}: a policy over a column the table does not expose must be refused, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {response}"
            );
            assert!(
                error_message(&response).contains(missing),
                "case {label:?}: the refusal must name the column it is about, got {:?}",
                error_message(&response)
            );
            // Nothing was persisted: the table holds no rule of that type, and the table
            // itself is exactly as the create left it.
            let (status, read) = emulator.get_analysis_rule(&id, rule_type).await;
            assert_eq!(
                status, 404,
                "case {label:?}: the refused create must persist no rule, got {status} with {read}"
            );
            let (status, read) = emulator.get_configured_table(&id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetConfiguredTable 200 for {id}, got {status} with {read}"
            );
            assert_eq!(
                read["configuredTable"], table,
                "case {label:?}: the refused create must leave the table at {table}, got {}",
                read["configuredTable"]
            );
        }

        // The same policy over a table that exposes every column it names is accepted.
        let table = emulator
            .table_over(
                &format!("exposed-{rule_type}"),
                ALL_TABLE_REFERENCE_TYPES[0],
                "DIRECT_QUERY",
                &COLUMNS,
            )
            .await;
        let id = table["id"].as_str().unwrap_or_default().to_string();
        let (status, response) = emulator
            .create_analysis_rule(&id, rule_type, policy.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {rule_type:?}: a policy over exposed columns must be accepted, got {status} with {response}"
        );
    }
}

#[tokio::test]
async fn update_configured_table_selected_analysis_methods_must_configure_at_least_two() {
    let emulator = Emulator::new();

    // Rows span the member's domain: every list the two-value `SelectedAnalysisMethod`
    // enum admits (both orders), the one-method lists, a list that repeats one method (so
    // it still configures ONE) and the empty list.
    let rows: Vec<(Vec<&str>, bool)> = vec![
        (ALL_SELECTED_ANALYSIS_METHODS.to_vec(), true),
        (
            vec![
                ALL_SELECTED_ANALYSIS_METHODS[1],
                ALL_SELECTED_ANALYSIS_METHODS[0],
            ],
            true,
        ),
        (vec![ALL_SELECTED_ANALYSIS_METHODS[0]], false),
        (vec![ALL_SELECTED_ANALYSIS_METHODS[1]], false),
        (
            vec![
                ALL_SELECTED_ANALYSIS_METHODS[0],
                ALL_SELECTED_ANALYSIS_METHODS[0],
            ],
            false,
        ),
        (Vec::new(), false),
    ];

    for (row, (selected, accepted)) in rows.into_iter().enumerate() {
        let label = format!("selectedAnalysisMethods={selected:?}");
        let table = emulator
            .table_over(
                &format!("selected-{row}"),
                ALL_TABLE_REFERENCE_TYPES[row % ALL_TABLE_REFERENCE_TYPES.len()],
                "MULTIPLE",
                &COLUMNS,
            )
            .await;
        let id = table["id"].as_str().unwrap_or_default().to_string();

        let (status, response) = emulator
            .update_configured_table(&id, json!({ "selectedAnalysisMethods": selected.clone() }))
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: expected UpdateConfiguredTable 200, got {status} with {response}"
            );
            assert_eq!(
                response["configuredTable"]["selectedAnalysisMethods"], json!(selected),
                "case {label:?}: expected selectedAnalysisMethods {selected:?}, got {}",
                response["configuredTable"]["selectedAnalysisMethods"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: fewer than {MIN_SELECTED_ANALYSIS_METHODS} analysis methods must be refused with 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {response}"
            );
            // The refused update left the stored table exactly where it was.
            let (status, read) = emulator.get_configured_table(&id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetConfiguredTable 200 for {id}, got {status} with {read}"
            );
            assert_eq!(
                read["configuredTable"], table,
                "case {label:?}: the refused update must leave the table at {table}, got {}",
                read["configuredTable"]
            );
        }
    }
}

#[tokio::test]
async fn update_configured_table_of_an_unheld_identifier_is_not_found() {
    let emulator = Emulator::new();

    // One table exists, so the 404s below are about the identifier and not about an empty
    // store.
    let held = emulator
        .table_over("the-one-table", ALL_TABLE_REFERENCE_TYPES[0], "MULTIPLE", &COLUMNS)
        .await;
    let held_id = held["id"].as_str().unwrap_or_default().to_string();

    // Identifiers in the form the model's `ConfiguredTableIdentifier` requires that no
    // create minted: the two extremes of the space and one in between.
    for id in [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "deadbeef-0000-4000-8000-000000000001",
    ] {
        assert_ne!(
            id, held_id,
            "the unheld identifiers must not include the created one"
        );
        let (status, response) = emulator
            .update_configured_table(id, json!({ "name": "no-such-table" }))
            .await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected UpdateConfiguredTable 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            response["resourceId"].as_str(),
            Some(id),
            "case {id:?}: expected resourceId {id:?}, got {response}"
        );
        assert_eq!(
            response["resourceType"].as_str(),
            Some("CONFIGURED_TABLE"),
            "case {id:?}: expected resourceType CONFIGURED_TABLE, got {response}"
        );
    }

    // The failed updates left the held table exactly where it was.
    let (status, read) = emulator.get_configured_table(&held_id).await;
    assert_eq!(
        status, 200,
        "expected GetConfiguredTable 200 for {held_id}, got {status} with {read}"
    );
    assert_eq!(
        read["configuredTable"], held,
        "the held table must still read back as {held}, got {}",
        read["configuredTable"]
    );
}

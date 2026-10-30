// SPDX-License-Identifier: CC0-1.0

//! `CreateTable`'s static-column rule, driven end to end over the service's own
//! `awsJson1_0` wire protocol through the shared
//! [`crate::keyspace_table_lifecycle::Emulator`] harness, so what is pinned is the
//! emulator's observable behaviour: a refusal is only believed once a read shows nothing
//! was stored, and an acceptance only once a read reports the schema back.
//!
//! The rule under test, exercised over a SET of schemas rather than one example: a static
//! column's values "are shared by all rows in the same partition"
//! (`SchemaDefinition$staticColumns`) and what divides a partition into rows are its
//! clustering columns (`ClusteringKey`), so a schema that declares at least one static
//! column is a table only when it also declares at least one clustering column — any
//! other such schema is the modelled `ValidationException` (400) with nothing stored,
//! whatever the counts of columns, keys and static columns involved.
//!
//! The counts are what the rows below vary: the rule is about the PRESENCE of a clustering
//! column, not about how many of anything there is, so one static column and several, one
//! clustering key and several, an omitted list and an empty one all answer by the same
//! rule.

use serde_json::{json, Value};

use crate::keyspace_table_lifecycle::{assert_failure, assert_ok, settle_wait, Emulator};

/// One column of `allColumns`.
fn column(name: &str, column_type: &str) -> Value {
    json!({"name": name, "type": column_type})
}

/// The `message` member of a failure body, for an assertion on the refusal's wording.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message member>")
        .to_string()
}

/// A schema whose lists are given exactly as the case names them: `clustering` is the
/// `clusteringKeys` entries and `statics` the `staticColumns` entries, each `None` when
/// the request omits the member entirely.
fn schema(
    columns: &[(&str, &str)],
    partition: &[&str],
    clustering: Option<&[(&str, &str)]>,
    statics: Option<&[&str]>,
) -> Value {
    let mut definition = json!({
        "allColumns": columns
            .iter()
            .map(|(name, column_type)| column(name, column_type))
            .collect::<Vec<Value>>(),
        "partitionKeys": partition
            .iter()
            .map(|name| json!({"name": name}))
            .collect::<Vec<Value>>(),
    });
    if let Some(clustering) = clustering {
        definition["clusteringKeys"] = clustering
            .iter()
            .map(|(name, order)| json!({"name": name, "orderBy": order}))
            .collect::<Vec<Value>>()
            .into();
    }
    if let Some(statics) = statics {
        definition["staticColumns"] = statics
            .iter()
            .map(|name| json!({"name": name}))
            .collect::<Vec<Value>>()
            .into();
    }
    definition
}

/// The rule over a table of schemas: a static column requires a clustering column, and
/// nothing else about the schema changes that answer.
#[tokio::test]
async fn a_static_column_requires_at_least_one_clustering_column() {
    let emulator = Emulator::start();
    let keyspace = "static_rule_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    // Each row: the case name, its schema, and whether the schema is a table. The rows
    // differ in the number of partition keys, clustering keys and static columns, and in
    // whether each optional list is omitted or supplied empty — the dimensions the rule
    // generalises over.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            // The cited shape: one static column, no clusteringKeys member at all.
            "one_static_no_clustering_member",
            schema(
                &[("pk", "int"), ("s", "text")],
                &["pk"],
                None,
                Some(&["s"]),
            ),
            false,
        ),
        (
            // Supplying the member empty declares no clustering column either.
            "one_static_empty_clustering_list",
            schema(
                &[("pk", "int"), ("s", "text")],
                &["pk"],
                Some(&[]),
                Some(&["s"]),
            ),
            false,
        ),
        (
            // Several static columns, still no clustering column.
            "three_statics_no_clustering",
            schema(
                &[
                    ("pk", "int"),
                    ("s1", "text"),
                    ("s2", "int"),
                    ("s3", "boolean"),
                ],
                &["pk"],
                None,
                Some(&["s1", "s2", "s3"]),
            ),
            false,
        ),
        (
            // A composite partition key is still one row per partition.
            "composite_partition_key_no_clustering",
            schema(
                &[("pk1", "int"), ("pk2", "text"), ("s", "text")],
                &["pk1", "pk2"],
                None,
                Some(&["s"]),
            ),
            false,
        ),
        (
            // One clustering column is enough, however many static columns there are.
            "two_statics_one_clustering",
            schema(
                &[
                    ("pk", "int"),
                    ("ck", "timestamp"),
                    ("s1", "text"),
                    ("s2", "int"),
                ],
                &["pk"],
                Some(&[("ck", "ASC")]),
                Some(&["s1", "s2"]),
            ),
            true,
        ),
        (
            // ... and so are several, in either sort order.
            "one_static_two_clustering_keys",
            schema(
                &[
                    ("pk", "int"),
                    ("ck1", "timestamp"),
                    ("ck2", "text"),
                    ("s", "text"),
                ],
                &["pk"],
                Some(&[("ck1", "DESC"), ("ck2", "ASC")]),
                Some(&["s"]),
            ),
            true,
        ),
        (
            // A schema with no static column is unaffected by the rule, with or without
            // a clustering column.
            "no_statics_no_clustering",
            schema(&[("pk", "int"), ("val", "text")], &["pk"], None, None),
            true,
        ),
        (
            "no_statics_with_clustering",
            schema(
                &[("pk", "int"), ("ck", "int"), ("val", "text")],
                &["pk"],
                Some(&[("ck", "ASC")]),
                None,
            ),
            true,
        ),
        (
            // An empty staticColumns list declares no static column, so a table with no
            // clustering column is still a table.
            "empty_statics_no_clustering",
            schema(
                &[("pk", "int"), ("val", "text")],
                &["pk"],
                None,
                Some(&[]),
            ),
            true,
        ),
    ];

    for (case, definition, is_a_table) in rows {
        let answer = emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": case,
                    "schemaDefinition": definition.clone(),
                }),
            )
            .await;
        if is_a_table {
            assert_ok(case, answer);
            tokio::time::sleep(settle_wait()).await;
            let got = assert_ok(
                case,
                emulator
                    .call(
                        "GetTable",
                        json!({"keyspaceName": keyspace, "tableName": case}),
                    )
                    .await,
            );
            // An accepted create stored the schema it was given: the reader proves it,
            // static columns included.
            assert_eq!(
                got["schemaDefinition"]["allColumns"], definition["allColumns"],
                "case {case:?}: expected allColumns {}, got {}",
                definition["allColumns"], got["schemaDefinition"]["allColumns"]
            );
            assert_eq!(
                got["schemaDefinition"]["staticColumns"], definition["staticColumns"],
                "case {case:?}: expected staticColumns {}, got {}",
                definition["staticColumns"], got["schemaDefinition"]["staticColumns"]
            );
            assert_eq!(
                got["schemaDefinition"]["clusteringKeys"], definition["clusteringKeys"],
                "case {case:?}: expected clusteringKeys {}, got {}",
                definition["clusteringKeys"], got["schemaDefinition"]["clusteringKeys"]
            );
        } else {
            let (_, body) = answer.clone();
            assert_failure(case, answer, 400, "ValidationException");
            // The refusal names the static column the caller has to fix, so the message
            // is the rule and not a blanket no.
            let refusal = message(&body);
            assert!(
                refusal.contains("staticColumns") && refusal.contains("clustering column"),
                "case {case:?}: expected a refusal naming staticColumns and the \
                 clustering-column requirement, got {refusal:?}"
            );
            // A refused create stored nothing.
            assert_failure(
                case,
                emulator
                    .call(
                        "GetTable",
                        json!({"keyspaceName": keyspace, "tableName": case}),
                    )
                    .await,
                400,
                "ResourceNotFoundException",
            );
        }
    }
}

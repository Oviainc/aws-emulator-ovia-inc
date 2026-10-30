// SPDX-License-Identifier: CC0-1.0

//! Which column a name inside a definition addresses, driven end to end over the service's
//! own `awsJson1_0` wire protocol through the shared
//! [`crate::keyspace_table_lifecycle::Emulator`] harness — so what is pinned is the
//! emulator's observable behaviour: a refusal is only believed once a read shows nothing
//! was stored, and an acceptance only once `GetTable` reports the schema back.
//!
//! The rule under test, exercised over a SET of inputs rather than one example: a name
//! inside a definition is a CQL identifier, so two names are the SAME name when they are
//! the same identifier — an unquoted name is case-folded ("Amazon Keyspaces converts upper
//! case characters automatically into lower case characters"), a double-quoted one stands
//! for itself. Every comparison of names supplied in one request therefore compares
//! identifiers and not spellings: `allColumns` declaring one identifier twice is the
//! modelled `ValidationException` (400) with nothing stored, a key or static column spelled
//! in another casing names the column `allColumns` declares, and two casings of one name in
//! two role lists are one column given two roles.
//!
//! The rows below vary what the rule generalises over: which member carries the differing
//! spelling (`allColumns`, `partitionKeys`, `clusteringKeys`, `staticColumns`,
//! `addColumns`), the form of the name (unquoted, upper case, mixed case, double-quoted),
//! the number of columns and keys, and whether the duplicate arrives in the same request
//! (`CreateTable`) or in a later one (`UpdateTable`).

use serde_json::{json, Value};

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, reported_schema, settle_wait, Emulator,
};

/// The `message` member of a failure body, for an assertion on the refusal's wording.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message member>")
        .to_string()
}

/// A `schemaDefinition` built from the four lists, each member omitted when its slice is
/// empty — exactly as a client that has nothing to put in it leaves it out.
fn schema(
    columns: &[(&str, &str)],
    partition: &[&str],
    clustering: &[(&str, &str)],
    statics: &[&str],
) -> Value {
    let mut definition = json!({
        "allColumns": columns
            .iter()
            .map(|(name, column_type)| json!({"name": name, "type": column_type}))
            .collect::<Vec<Value>>(),
        "partitionKeys": partition
            .iter()
            .map(|name| json!({"name": name}))
            .collect::<Vec<Value>>(),
    });
    if !clustering.is_empty() {
        definition["clusteringKeys"] = clustering
            .iter()
            .map(|(name, order)| json!({"name": name, "orderBy": order}))
            .collect::<Vec<Value>>()
            .into();
    }
    if !statics.is_empty() {
        definition["staticColumns"] = statics
            .iter()
            .map(|name| json!({"name": name}))
            .collect::<Vec<Value>>()
            .into();
    }
    definition
}

/// What a case expects of the schema it supplies.
#[derive(Debug)]
enum Expected {
    /// The schema is a table, and reads back exactly as it was supplied.
    Stored,
    /// The schema is refused 400 `ValidationException`, with nothing stored, and the
    /// refusal's wording carries this fragment so the caller is told which name to fix.
    Refused(&'static str),
}

/// `GetTable` for one table, as its HTTP status and body.
async fn get_table(emulator: &Emulator, keyspace: &str, table: &str) -> (u16, Value) {
    emulator
        .call(
            "GetTable",
            json!({"keyspaceName": keyspace, "tableName": table}),
        )
        .await
}

/// THE RULE on a create: `CreateTable` compares every name in `schemaDefinition` as a CQL
/// identifier, so one identifier cannot be declared twice and a key may be spelled in any
/// casing of the column `allColumns` declares.
#[tokio::test]
async fn create_table_compares_column_names_as_cql_identifiers() {
    let emulator = Emulator::start();
    let keyspace = "column_identity_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    // Each row: the case name (also the table name), its schema, and what the schema is.
    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            // The baseline: one spelling per column, nothing to fold.
            "all_lower_case_baseline",
            schema(&[("pk", "int"), ("v", "text")], &["pk"], &[], &[]),
            Expected::Stored,
        ),
        (
            // The cited shape: `id` and `ID` are one unquoted identifier.
            "dup_lower_and_upper",
            schema(&[("id", "int"), ("ID", "text")], &["id"], &[], &[]),
            Expected::Refused("allColumns declares column 'ID' more than once"),
        ),
        (
            // Mixed case, a longer name, and the duplicate third in a longer list.
            "dup_mixed_case",
            schema(
                &[("pk", "int"), ("UserName", "text"), ("username", "text")],
                &["pk"],
                &[],
                &[],
            ),
            Expected::Refused("allColumns declares column 'username' more than once"),
        ),
        (
            // The same spelling twice is the same identifier too.
            "dup_identical_spelling",
            schema(
                &[("pk", "int"), ("dup", "int"), ("dup", "text")],
                &["pk"],
                &[],
                &[],
            ),
            Expected::Refused("allColumns declares column 'dup' more than once"),
        ),
        (
            // A double-quoted name is its own identifier, and equal to itself.
            "dup_quoted_spelling",
            schema(
                &[("pk", "int"), ("\"Id\"", "int"), ("\"Id\"", "text")],
                &["pk"],
                &[],
                &[],
            ),
            Expected::Refused("allColumns declares column '\"Id\"' more than once"),
        ),
        (
            // ... but a quoted name is NOT the unquoted one of the same letters, so these
            // are two columns.
            "quoted_is_a_second_column",
            schema(
                &[("id", "int"), ("\"Id\"", "text")],
                &["ID"],
                &[],
                &[],
            ),
            Expected::Stored,
        ),
        (
            // A partition key spelled in another casing names the declared column.
            "partition_key_casing_differs",
            schema(&[("Id", "int"), ("payload", "text")], &["id"], &[], &[]),
            Expected::Stored,
        ),
        (
            // ... and so does a clustering key, in a composite primary key.
            "clustering_key_casing_differs",
            schema(
                &[
                    ("PK1", "int"),
                    ("pk2", "text"),
                    ("CK", "timestamp"),
                    ("v", "text"),
                ],
                &["pk1", "PK2"],
                &[("ck", "DESC")],
                &[],
            ),
            Expected::Stored,
        ),
        (
            // ... and so does a static column.
            "static_column_casing_differs",
            schema(
                &[("pk", "int"), ("ck", "int"), ("Shared", "text")],
                &["pk"],
                &[("CK", "ASC")],
                &["shared"],
            ),
            Expected::Stored,
        ),
        (
            // A quoted key name stands for itself, so it does not name the unquoted column.
            "quoted_key_is_not_the_unquoted_column",
            schema(
                &[("Id", "int"), ("payload", "text")],
                &["\"Id\""],
                &[],
                &[],
            ),
            Expected::Refused(
                "schemaDefinition.partitionKeys names column '\"Id\"', which \
                 schemaDefinition.allColumns does not declare",
            ),
        ),
        (
            // ... nor does an unquoted key name a quoted column.
            "unquoted_key_is_not_the_quoted_column",
            schema(
                &[("\"Id\"", "int"), ("payload", "text")],
                &["Id"],
                &[],
                &[],
            ),
            Expected::Refused(
                "schemaDefinition.partitionKeys names column 'Id', which \
                 schemaDefinition.allColumns does not declare",
            ),
        ),
        (
            // A name of no declared column at all, in any casing.
            "key_names_no_column",
            schema(&[("pk", "int"), ("v", "text")], &["Other"], &[], &[]),
            Expected::Refused(
                "schemaDefinition.partitionKeys names column 'Other', which \
                 schemaDefinition.allColumns does not declare",
            ),
        ),
        (
            // Two casings in two role lists are one column asked to hold two roles.
            "role_conflict_across_casings",
            schema(
                &[("pk", "int"), ("ck", "int"), ("v", "text")],
                &["pk"],
                &[("PK", "ASC")],
                &[],
            ),
            Expected::Refused(
                "schemaDefinition.clusteringKeys names column 'PK', which the schema \
                 already gives another role",
            ),
        ),
        (
            // The same conflict between a clustering key and a static column.
            "static_conflicts_with_clustering_key",
            schema(
                &[("pk", "int"), ("ck", "int"), ("v", "text")],
                &["pk"],
                &[("ck", "ASC")],
                &["CK"],
            ),
            Expected::Refused(
                "schemaDefinition.staticColumns names column 'CK', which the schema \
                 already gives another role",
            ),
        ),
    ];

    for (case, definition, expected) in rows {
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
        match expected {
            Expected::Stored => {
                assert_ok(case, answer);
                tokio::time::sleep(settle_wait()).await;
                let got = assert_ok(case, get_table(&emulator, keyspace, case).await);
                // The names are stored as the caller spelled them, as a type's field names
                // are: the identity rule decides which column a name addresses, it does not
                // rewrite what was stored.
                assert_eq!(
                    got["schemaDefinition"],
                    reported_schema(&definition),
                    "case {case:?}: expected the stored schema {definition}, got {}",
                    got["schemaDefinition"]
                );
            }
            Expected::Refused(fragment) => {
                let (_, body) = answer.clone();
                assert_failure(case, answer, 400, "ValidationException");
                let refusal = message(&body);
                assert!(
                    refusal.contains(fragment),
                    "case {case:?}: expected a refusal carrying {fragment:?}, got {refusal:?}"
                );
                // A refused create stored nothing.
                assert_failure(
                    case,
                    get_table(&emulator, keyspace, case).await,
                    400,
                    "ResourceNotFoundException",
                );
            }
        }
    }
}

/// THE RULE on an update: `addColumns` names a new column only when its identifier is one
/// the stored schema does not already declare and this request does not repeat.
#[tokio::test]
async fn add_columns_compares_column_names_as_cql_identifiers() {
    let emulator = Emulator::start();
    let keyspace = "add_column_identity_ks";
    let table = "t1";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    let created = schema(&[("pk", "text"), ("payload", "int")], &["pk"], &[], &[]);
    assert_ok(
        "create the table",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": created.clone(),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    // Every row here is REFUSED, so the stored schema stays the one the create left behind.
    for (case, added, fragment) in [
        (
            // The cited shape: another casing of a column the schema already declares.
            "upper_case_of_a_declared_column",
            json!([{"name": "PAYLOAD", "type": "text"}]),
            "addColumns names column 'PAYLOAD', which the table's schema already declares",
        ),
        (
            // The key column is a declared column too.
            "mixed_case_of_the_partition_key",
            json!([{"name": "Pk", "type": "text"}]),
            "addColumns names column 'Pk', which the table's schema already declares",
        ),
        (
            // The same spelling is the same identifier.
            "identical_spelling",
            json!([{"name": "payload", "type": "int"}]),
            "addColumns names column 'payload', which the table's schema already declares",
        ),
        (
            // Two casings of one new name inside ONE request: the second repeats the first.
            "two_casings_in_one_request",
            json!([{"name": "extra", "type": "int"}, {"name": "EXTRA", "type": "text"}]),
            "addColumns names column 'EXTRA', which the table's schema already declares",
        ),
    ] {
        let answer = emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "addColumns": added,
                }),
            )
            .await;
        let (_, body) = answer.clone();
        assert_failure(case, answer, 400, "ValidationException");
        let refusal = message(&body);
        assert!(
            refusal.contains(fragment),
            "case {case:?}: expected a refusal carrying {fragment:?}, got {refusal:?}"
        );
        let got = assert_ok(case, get_table(&emulator, keyspace, table).await);
        assert_eq!(
            got["schemaDefinition"],
            reported_schema(&created),
            "case {case:?}: a refused update must leave the stored schema alone, got {}",
            got["schemaDefinition"]
        );
    }

    // The accepted half, applied in order: a quoted spelling is a column of its own, a new
    // name in any casing is added verbatim, and once it is stored another casing of IT is
    // refused in turn.
    let mut expected_columns = created["allColumns"]
        .as_array()
        .expect("the created schema declares columns")
        .clone();
    for (case, name, column_type) in [
        ("a_quoted_spelling_is_its_own_column", "\"Payload\"", "text"),
        ("a_new_mixed_case_name", "Added", "text"),
    ] {
        assert_ok(
            case,
            emulator
                .call(
                    "UpdateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "addColumns": [{"name": name, "type": column_type}],
                    }),
                )
                .await,
        );
        tokio::time::sleep(settle_wait()).await;
        expected_columns.push(json!({"name": name, "type": column_type}));
        let got = assert_ok(case, get_table(&emulator, keyspace, table).await);
        assert_eq!(
            got["schemaDefinition"]["allColumns"],
            json!(expected_columns),
            "case {case:?}: expected the added column to read back verbatim, got {}",
            got["schemaDefinition"]["allColumns"]
        );
    }

    let case = "another_casing_of_a_column_added_earlier";
    let answer = emulator
        .call(
            "UpdateTable",
            json!({
                "keyspaceName": keyspace,
                "tableName": table,
                "addColumns": [{"name": "added", "type": "int"}],
            }),
        )
        .await;
    let (_, body) = answer.clone();
    assert_failure(case, answer, 400, "ValidationException");
    let refusal = message(&body);
    assert!(
        refusal.contains("addColumns names column 'added', which the table's schema already declares"),
        "case {case:?}: expected the refusal to name the column already declared, got {refusal:?}"
    );
    let got = assert_ok(case, get_table(&emulator, keyspace, table).await);
    assert_eq!(
        got["schemaDefinition"]["allColumns"],
        json!(expected_columns),
        "case {case:?}: a refused update must leave the stored schema alone, got {}",
        got["schemaDefinition"]["allColumns"]
    );
}

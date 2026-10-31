// SPDX-License-Identifier: CC0-1.0

//! In which ORDER this service grades the two faults ONE column or field list can carry
//! at once — a name the list declares twice (or that the stored schema already declares)
//! and a type expression naming a user-defined type the keyspace cannot resolve — driven
//! end to end over the service's own `awsJson1_0` wire protocol through the shared
//! [`crate::keyspace_table_lifecycle::Emulator`] harness, so what is pinned is the
//! emulator's observable behaviour.
//!
//! The rule under test, exercised over a SET of inputs rather than one example: a
//! per-entry refusal is decided over the WHOLE list before the next per-entry refusal
//! begins — the duplicate-declaration refusal (the modelled `ValidationException`) first,
//! the type resolution of each entry (the modelled `ResourceNotFoundException`) second —
//! so which of the two a list carrying both faults answers is decided by the faults it
//! carries and NOT by where in the list each faulty entry sits. Every operation that
//! carries such a list grades the pair the same way: `CreateTable` for a schema's columns,
//! `CreateType` for a type's fields and `UpdateTable` for the columns it adds.
//!
//! Its consequences, also pinned here: a list carrying only the unresolvable reference
//! still earns the not-found it deserves, in any position (the duplicate refusal orders
//! the two faults, it does not swallow one), and a refused request stores nothing.

use serde_json::{json, Value};

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, reported_schema, settle_wait, Emulator,
};

/// The modelled refusal a duplicate declaration earns, on the client-error status every
/// refusal of this RPC service travels with
/// (`service_..._keyspacesservice::scaffolded::CLIENT_ERROR_STATUS`).
const DUPLICATE: (u16, &str) = (400, "ValidationException");

/// The modelled refusal an unresolvable type reference earns.
const NOT_FOUND: (u16, &str) = (400, "ResourceNotFoundException");

/// The two columns the table under test is created with: `pk` (its partition key) and
/// `payload`.
const PARTITION_KEY: &str = "pk";

/// One type expression that cannot be resolved, and the user-defined type name the
/// service must report as missing.
///
/// The rows span how the unresolvable reference is written: bare, and nested inside a type
/// constructor in the element and in the KEY position — the constructors are the service's
/// own published catalog (`service_..._keyspacesservice::state::CQL_TYPE_CONSTRUCTORS`,
/// whose source is `ColumnDefinition$type` -> Cassandra data type support in the Amazon
/// Keyspaces Developer Guide), and two of the three rows name a type the issue this suite
/// is about never cited.
struct BadType {
    name: &'static str,
    expression: &'static str,
    unresolvable: &'static str,
}

fn bad_types() -> Vec<BadType> {
    vec![
        BadType {
            name: "bare",
            expression: "nosuchtype",
            unresolvable: "nosuchtype",
        },
        BadType {
            name: "in_frozen",
            expression: "frozen<ghost_udt>",
            unresolvable: "ghost_udt",
        },
        BadType {
            name: "in_map_key",
            expression: "map<missing_key, text>",
            unresolvable: "missing_key",
        },
    ]
}

/// One duplicate-declaration fault in an `addColumns` list: the entries that carry it, and
/// the column spellings the refusal may name.
///
/// The rows span the forms the fault takes: a column the stored schema already declares
/// spelled exactly as stored, one spelled in another casing (an unquoted CQL identifier is
/// case-folded, so another casing addresses the same column), the table's partition key —
/// a declared column like any other — and a FRESH name the request itself repeats, where
/// either spelling may be the one reported because the refusal names the entry that
/// repeats an earlier one.
struct DuplicateFault {
    name: &'static str,
    entries: Vec<Value>,
    spellings: Vec<&'static str>,
}

fn duplicate_faults() -> Vec<DuplicateFault> {
    vec![
        DuplicateFault {
            name: "stored_column_same_spelling",
            entries: vec![json!({"name": "payload", "type": "int"})],
            spellings: vec!["payload"],
        },
        DuplicateFault {
            name: "stored_column_other_casing",
            entries: vec![json!({"name": "PayLoad", "type": "text"})],
            spellings: vec!["PayLoad"],
        },
        DuplicateFault {
            name: "partition_key_other_casing",
            entries: vec![json!({"name": "PK", "type": "text"})],
            spellings: vec!["PK"],
        },
        DuplicateFault {
            name: "fresh_name_repeated_in_one_request",
            entries: vec![
                json!({"name": "extra", "type": "int"}),
                json!({"name": "EXTRA", "type": "text"}),
            ],
            spellings: vec!["extra", "EXTRA"],
        },
    ]
}

/// The well-formed entries every list below also carries, so the faulty ones are never the
/// only ones and the order under test is a real ordering.
fn fine_entries(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({"name": format!("fine_{index}"), "type": "text"}))
        .collect()
}

/// Every ordering of `entries`, so a rule that must not depend on the order of a list is
/// checked against ALL of them rather than against the two the report happened to cite.
fn permutations(entries: &[Value]) -> Vec<Vec<Value>> {
    if entries.len() <= 1 {
        return vec![entries.to_vec()];
    }
    let mut orders = Vec::new();
    for index in 0..entries.len() {
        let mut rest = entries.to_vec();
        let head = rest.remove(index);
        for tail in permutations(&rest) {
            let mut order = vec![head.clone()];
            order.extend(tail);
            orders.push(order);
        }
    }
    orders
}

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message member>")
        .to_string()
}

/// Assert a refusal carries exactly `expected` as its message.
fn assert_message(case: &str, body: &Value, expected: &str) {
    let actual = message(body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the message {expected:?}, got {actual:?}"
    );
}

/// Assert a refusal is the duplicate-declaration one, naming one of `spellings`.
fn assert_duplicate_message(case: &str, body: &Value, spellings: &[&str]) {
    let actual = message(body);
    let accepted: Vec<String> = spellings
        .iter()
        .map(|spelling| {
            format!(
                "addColumns names column '{spelling}', which the table's schema already \
                 declares"
            )
        })
        .collect();
    assert!(
        accepted.iter().any(|expected| expected == &actual),
        "case {case:?}: expected one of the messages {accepted:?}, got {actual:?}"
    );
}

/// One `UpdateTable` naming `added`, as its HTTP status and body.
async fn add_columns(
    emulator: &Emulator,
    keyspace: &str,
    table: &str,
    added: &[Value],
) -> (u16, Value) {
    emulator
        .call(
            "UpdateTable",
            json!({
                "keyspaceName": keyspace,
                "tableName": table,
                "addColumns": added,
            }),
        )
        .await
}

/// Assert the table still reports the schema it was created with: every call in this suite
/// is refused, so none of them may have grown it.
async fn assert_schema_unchanged(emulator: &Emulator, keyspace: &str, table: &str, case: &str) {
    let got = assert_ok(
        case,
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    );
    assert_eq!(
        got["schemaDefinition"],
        reported_schema(&minimal_schema(PARTITION_KEY)),
        "case {case:?}: a refused update must leave the stored schema alone, got {got}"
    );
}

/// Create `keyspace` and an `ACTIVE` table `table` in it, the starting point of every case
/// below, since a table can only be updated from `ACTIVE`.
async fn active_table(emulator: &Emulator, keyspace: &str, table: &str) {
    assert_ok(
        keyspace,
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    assert_ok(
        table,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": minimal_schema(PARTITION_KEY),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
}

/// RULE: of the two faults an `addColumns` list can carry, the duplicate declaration is
/// the one the caller is told about — in EVERY ordering of the entries, so the same
/// request content never gets two different answers.
#[tokio::test]
async fn the_duplicate_declaration_outranks_an_unresolvable_type_in_every_order() {
    let emulator = Emulator::start();
    let keyspace = "add_columns_grade_order_ks";
    let table = "tbl";
    active_table(&emulator, keyspace, table).await;

    for duplicate in duplicate_faults() {
        for bad in bad_types() {
            // One list carrying BOTH faults, padded with well-formed entries to four
            // entries in total, and then sent in every one of the 24 orders of those
            // four.
            let mut entries = duplicate.entries.clone();
            entries.push(json!({"name": "typed", "type": bad.expression}));
            entries.extend(fine_entries(4 - entries.len()));
            let orders = permutations(&entries);
            assert_eq!(
                orders.len(),
                24,
                "case {:?}: four entries have 24 orderings, got {}",
                duplicate.name,
                orders.len()
            );

            for (index, order) in orders.iter().enumerate() {
                let case = format!(
                    "{} with {} type, order {index}: {}",
                    duplicate.name,
                    bad.name,
                    Value::from(order.clone())
                );
                let got = add_columns(&emulator, keyspace, table, order).await;
                let body = got.1.clone();
                assert_failure(&case, got, DUPLICATE.0, DUPLICATE.1);
                assert_duplicate_message(&case, &body, &duplicate.spellings);
            }
        }
        assert_schema_unchanged(&emulator, keyspace, table, duplicate.name).await;
    }
}

/// The other half of the rule: ordering the two faults does not swallow either of them, so
/// a list whose ONLY fault is the unresolvable reference earns the not-found in every
/// position, and one whose only fault is the duplicate earns the validation refusal.
#[tokio::test]
async fn each_fault_alone_still_earns_its_own_refusal_in_every_position() {
    let emulator = Emulator::start();
    let keyspace = "add_columns_single_fault_ks";
    let table = "tbl";
    active_table(&emulator, keyspace, table).await;

    for bad in bad_types() {
        let mut entries = vec![json!({"name": "typed", "type": bad.expression})];
        entries.extend(fine_entries(2));
        for (index, order) in permutations(&entries).iter().enumerate() {
            let case = format!("only the {} type, order {index}", bad.name);
            let got = add_columns(&emulator, keyspace, table, order).await;
            let body = got.1.clone();
            assert_failure(&case, got, NOT_FOUND.0, NOT_FOUND.1);
            assert_message(
                &case,
                &body,
                &format!("UDT {} does not exist", bad.unresolvable),
            );
        }
    }

    for duplicate in duplicate_faults() {
        let mut entries = duplicate.entries.clone();
        entries.extend(fine_entries(3 - entries.len()));
        for (index, order) in permutations(&entries).iter().enumerate() {
            let case = format!("only the {} duplicate, order {index}", duplicate.name);
            let got = add_columns(&emulator, keyspace, table, order).await;
            let body = got.1.clone();
            assert_failure(&case, got, DUPLICATE.0, DUPLICATE.1);
            assert_duplicate_message(&case, &body, &duplicate.spellings);
        }
    }

    assert_schema_unchanged(&emulator, keyspace, table, "every single-fault refusal").await;
}

/// The rule across the operations that carry such a list: `CreateTable`'s columns,
/// `CreateType`'s fields and `UpdateTable`'s added columns all answer the duplicate
/// declaration for a list carrying both faults, in every order — so one request shape
/// never gets two different answers depending on which sibling served it.
#[tokio::test]
async fn every_operation_carrying_a_definition_list_grades_the_pair_the_same_way() {
    let emulator = Emulator::start();
    let keyspace = "sibling_grade_order_ks";
    let table = "tbl";
    active_table(&emulator, keyspace, table).await;

    // `CreateTable`: the duplicate is a repeated `allColumns` entry, the partition key is
    // declared once, and one entry carries the unresolvable reference. Every ordering of
    // the four entries is the same refusal.
    let create_entries = vec![
        json!({"name": PARTITION_KEY, "type": "text"}),
        json!({"name": "dup", "type": "text"}),
        json!({"name": "dup", "type": "int"}),
        json!({"name": "typed", "type": "nosuchtype"}),
    ];
    for (index, order) in permutations(&create_entries).iter().enumerate() {
        let case = format!("CreateTable both faults, order {index}");
        let got = emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": format!("created_{index}"),
                    "schemaDefinition": {
                        "allColumns": order,
                        "partitionKeys": [{"name": PARTITION_KEY}],
                    },
                }),
            )
            .await;
        let body = got.1.clone();
        assert_failure(&case, got, DUPLICATE.0, DUPLICATE.1);
        assert_message(
            &case,
            &body,
            "schemaDefinition.allColumns declares column 'dup' more than once",
        );
    }

    // `CreateType`: the same pair over `fieldDefinitions`.
    let field_entries = vec![
        json!({"name": "dup", "type": "text"}),
        json!({"name": "dup", "type": "int"}),
        json!({"name": "typed", "type": "nosuchtype"}),
    ];
    for (index, order) in permutations(&field_entries).iter().enumerate() {
        let case = format!("CreateType both faults, order {index}");
        let got = emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": keyspace,
                    "typeName": format!("created_{index}"),
                    "fieldDefinitions": order,
                }),
            )
            .await;
        let body = got.1.clone();
        assert_failure(&case, got, DUPLICATE.0, DUPLICATE.1);
        assert_message(
            &case,
            &body,
            "fieldDefinitions declares field 'dup' more than once",
        );
    }

    // `UpdateTable`: the same pair over `addColumns`, where the duplicate is a fresh name
    // the request repeats.
    let added_entries = vec![
        json!({"name": "dup", "type": "text"}),
        json!({"name": "dup", "type": "int"}),
        json!({"name": "typed", "type": "nosuchtype"}),
    ];
    for (index, order) in permutations(&added_entries).iter().enumerate() {
        let case = format!("UpdateTable both faults, order {index}");
        let got = add_columns(&emulator, keyspace, table, order).await;
        let body = got.1.clone();
        assert_failure(&case, got, DUPLICATE.0, DUPLICATE.1);
        assert_duplicate_message(&case, &body, &["dup"]);
    }

    // Nothing any of those refusals addressed was stored: the table still reports the
    // schema it was created with, and neither sibling left a resource behind.
    assert_schema_unchanged(&emulator, keyspace, table, "the siblings stored nothing").await;
    for index in 0..permutations(&create_entries).len() {
        let case = format!("no table was created, order {index}");
        assert_failure(
            &case,
            emulator
                .call(
                    "GetTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": format!("created_{index}"),
                    }),
                )
                .await,
            NOT_FOUND.0,
            NOT_FOUND.1,
        );
    }
    for index in 0..permutations(&field_entries).len() {
        let case = format!("no type was created, order {index}");
        assert_failure(
            &case,
            emulator
                .call(
                    "GetType",
                    json!({
                        "keyspaceName": keyspace,
                        "typeName": format!("created_{index}"),
                    }),
                )
                .await,
            NOT_FOUND.0,
            NOT_FOUND.1,
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `UpdateTable`'s `addColumns` type resolution, driven end to end over the service's own
//! `awsJson1_0` wire protocol through the shared [`crate::keyspace_table_lifecycle::Emulator`]
//! harness, so what is pinned is the emulator's observable behaviour and an added column is
//! only believed once `GetTable` reports it back.
//!
//! The rule under test, exercised over a SET of inputs rather than one example: a column
//! added to a table that already exists declares its type under exactly the rule a created
//! column does — every identifier in the type that names neither a CQL native type nor a
//! type constructor must name a user-defined type of the SAME keyspace in status `ACTIVE`,
//! and any other reference is the modelled `ResourceNotFoundException` (404) naming that
//! type, with nothing added to the stored schema.
//!
//! Its consequences, also pinned here: a refused update leaves the table exactly as it was
//! (same schema, still `ACTIVE`), and no stored schema can hold a reference to a type the
//! service never created — so a type created later is not held hostage by a phantom
//! referring table when it is deleted.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, Emulator,
};

/// Create `keyspace` and, in it, an `ACTIVE` table named `table` over
/// [`minimal_schema`] — the starting point of every case below, since a table can only be
/// updated from `ACTIVE`.
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
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
}

/// `GetTable` for one table, as a 200 body.
async fn get_table(emulator: &Emulator, keyspace: &str, table: &str) -> Value {
    assert_ok(
        table,
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    )
}

/// One `UpdateTable` naming `add_columns`, as its HTTP status and body.
async fn add_columns(
    emulator: &Emulator,
    keyspace: &str,
    table: &str,
    add_columns: Value,
) -> (u16, Value) {
    emulator
        .call(
            "UpdateTable",
            json!({
                "keyspaceName": keyspace,
                "tableName": table,
                "addColumns": add_columns,
            }),
        )
        .await
}

/// Assert `table` still reports `schema` and is still `ACTIVE`, which is what a REFUSED
/// update has to leave behind.
async fn assert_unchanged(
    emulator: &Emulator,
    keyspace: &str,
    table: &str,
    case: &str,
    schema: &Value,
) {
    let got = get_table(emulator, keyspace, table).await;
    assert_eq!(
        &got["schemaDefinition"], schema,
        "case {case:?}: a refused update must leave the stored schema alone, got {got}"
    );
    assert_eq!(
        got["status"],
        json!(keyspaces::TABLE_STATUS_ACTIVE),
        "case {case:?}: a refused update must not move the table's status, got {got}"
    );
}

/// The type expressions an added column may declare without naming any user-defined type.
///
/// The first rows are the service's own published catalog, read from the constants the
/// resolution is built on (`keyspaces::CQL_NATIVE_TYPES`, whose source is
/// `ColumnDefinition$type` -> Cassandra data type support in the Amazon Keyspaces Developer
/// Guide), so no entry is spelled twice here and the entries this issue never cited are
/// covered too. The rows that follow are written out, because a constructor has to be
/// exercised AT its own arity and nested, which no catalog states — so they are checked
/// below against `keyspaces::CQL_TYPE_CONSTRUCTORS` (same source) to span every entry of
/// it, and one row is spelled in upper case because an unquoted CQL identifier is
/// case-insensitive.
fn resolvable_type_expressions() -> Vec<String> {
    let constructed: [&str; 9] = [
        "list<text>",
        "set<int>",
        "map<text, int>",
        "tuple<int, text, uuid>",
        "frozen<list<text>>",
        "frozen<map<text, frozen<set<uuid>>>>",
        "vector<float, 3>",
        "MAP<TEXT,INT>",
        "Timestamp",
    ];
    // Every published constructor is covered by one of those rows, so a constructor added
    // to the catalog later cannot go silently untested.
    for constructor in keyspaces::CQL_TYPE_CONSTRUCTORS {
        let covered = constructed.iter().any(|row| {
            row.to_ascii_lowercase()
                .contains(&format!("{}<", constructor.to_ascii_lowercase()))
        });
        assert!(
            covered,
            "case {constructor:?}: every published type constructor must appear in a row, \
             but none of {constructed:?} uses it"
        );
    }

    let mut accepted: Vec<String> = keyspaces::CQL_NATIVE_TYPES
        .iter()
        .map(|native| (*native).to_string())
        .collect();
    accepted.extend(constructed.iter().map(|row| (*row).to_string()));
    accepted
}

/// THE RULE: every type expression built only from the published native types and type
/// constructors is added to the stored schema, and one whose identifiers include a name the
/// keyspace does not define is the modelled not-found naming that name, with nothing added.
#[tokio::test]
async fn adding_columns_resolves_every_published_type_and_refuses_an_unresolvable_one() {
    let emulator = Emulator::start();
    let keyspace = "add_column_types_ks";
    let table = "t1";
    active_table(&emulator, keyspace, table).await;

    // The whole accepted class in ONE update: `addColumns` is a list, so one call spans
    // every row and the schema it leaves behind carries them all, in order.
    let accepted = resolvable_type_expressions();
    let added: Vec<Value> = accepted
        .iter()
        .enumerate()
        .map(|(index, declared)| json!({"name": format!("col_{index}"), "type": declared}))
        .collect();
    assert!(
        added.len() < keyspaces::MAX_COLUMNS_PER_TABLE,
        "the accepted rows must fit under the column cap, got {} rows",
        added.len()
    );
    assert_ok(
        "every resolvable type expression",
        add_columns(&emulator, keyspace, table, json!(added)).await,
    );
    tokio::time::sleep(settle_wait()).await;

    let mut expected_columns = minimal_schema("pk")["allColumns"]
        .as_array()
        .expect("the minimal schema declares columns")
        .clone();
    expected_columns.extend(added.iter().cloned());
    let got = get_table(&emulator, keyspace, table).await;
    assert_eq!(
        got["schemaDefinition"]["allColumns"],
        json!(expected_columns),
        "the added columns must read back verbatim, in order, got {got}"
    );

    // From here the table is ACTIVE again and every row below is REFUSED, so none of them
    // changes this schema.
    let settled_schema = got["schemaDefinition"].clone();

    // An identifier that names neither a native type nor a constructor can only name a
    // user-defined type, and this keyspace defines none — bare, nested, and in each
    // position of a parameterised type. An unquoted name is reported case-folded, the form
    // every other operation takes it in.
    for (declared, unresolved) in [
        ("nosuchtype", "nosuchtype"),
        ("varcharr", "varcharr"),
        ("Address", "address"),
        ("list<address>", "address"),
        ("map<text, nested_udt>", "nested_udt"),
        ("map<missing_key, text>", "missing_key"),
        ("frozen<address>", "address"),
        ("tuple<int, missing_type>", "missing_type"),
        ("vector<missing_element, 3>", "missing_element"),
    ] {
        let case = format!("addColumns with type {declared:?}");
        let (status, body) =
            add_columns(&emulator, keyspace, table, json!([{"name": "junk", "type": declared}]))
                .await;
        assert_failure(&case, (status, body.clone()), 404, "ResourceNotFoundException");
        assert_eq!(
            body["message"],
            json!(format!("UDT {unresolved} does not exist")),
            "case {case:?}: expected the not-found to name {unresolved:?}, got {body}"
        );
        assert_unchanged(&emulator, keyspace, table, &case, &settled_schema).await;
    }

    // One unresolvable column among resolvable ones refuses the WHOLE update: the schema
    // the call would have left behind is never partly applied.
    let case = "one unresolvable column among resolvable ones";
    let (status, body) = add_columns(
        &emulator,
        keyspace,
        table,
        json!([
            {"name": "fine_one", "type": "text"},
            {"name": "broken", "type": "frozen<ghost_udt>"},
            {"name": "fine_two", "type": "int"},
        ]),
    )
    .await;
    assert_failure(case, (status, body.clone()), 404, "ResourceNotFoundException");
    assert_eq!(
        body["message"],
        json!("UDT ghost_udt does not exist"),
        "case {case:?}: expected the not-found to name the unresolvable type, got {body}"
    );
    assert_unchanged(&emulator, keyspace, table, case, &settled_schema).await;
}

/// THE RULE, its resolving half: an added column's type resolves against the user-defined
/// types of its OWN keyspace, only once such a type is usable (`ACTIVE`) — and the table
/// that then refers to it is what `GetType` reports as a referring table.
#[tokio::test]
async fn an_added_column_resolves_against_the_user_defined_types_of_its_own_keyspace() {
    let emulator = Emulator::start();
    let defining = "udt_add_column_ks";
    let other = "no_udt_add_column_ks";
    let table = "t1";
    active_table(&emulator, defining, table).await;
    active_table(&emulator, other, table).await;

    assert_ok(
        "create the type",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": defining,
                    "typeName": "address",
                    "fieldDefinitions": [
                        {"name": "street", "type": "text"},
                        {"name": "zip", "type": "int"},
                    ],
                }),
            )
            .await,
    );

    // A type whose creation has only just been accepted is `CREATING`, not usable yet:
    // "its status might not be ACTIVE" is the same not-found. The table itself is ACTIVE,
    // so what is refused is the reference and nothing about the table.
    let before = get_table(&emulator, defining, table).await["schemaDefinition"].clone();
    let case = "a type that is still CREATING";
    let (status, body) = add_columns(
        &emulator,
        defining,
        table,
        json!([{"name": "home", "type": "frozen<address>"}]),
    )
    .await;
    assert_failure(case, (status, body.clone()), 404, "ResourceNotFoundException");
    assert_eq!(
        body["message"],
        json!("UDT address does not exist"),
        "case {case:?}: expected the not-found to name the type, got {body}"
    );
    assert_unchanged(&emulator, defining, table, case, &before).await;

    // Once the clock has settled the type into ACTIVE, every form that refers to it
    // resolves — in one update, so the whole class is covered by one accepted call.
    tokio::time::sleep(settle_wait()).await;
    let forms: [&str; 6] = [
        "address",
        "Address",
        "frozen<address>",
        "list<address>",
        "map<text, frozen<address>>",
        "tuple<address, int>",
    ];
    let added: Vec<Value> = forms
        .iter()
        .enumerate()
        .map(|(index, declared)| json!({"name": format!("udt_col_{index}"), "type": declared}))
        .collect();
    assert_ok(
        "every form that refers to an ACTIVE type",
        add_columns(&emulator, defining, table, json!(added)).await,
    );
    tokio::time::sleep(settle_wait()).await;
    let got = get_table(&emulator, defining, table).await;
    let mut expected_columns = before["allColumns"]
        .as_array()
        .expect("the table declares columns")
        .clone();
    expected_columns.extend(added.iter().cloned());
    assert_eq!(
        got["schemaDefinition"]["allColumns"],
        json!(expected_columns),
        "the added columns must read back verbatim, in order, got {got}"
    );

    // The table is now a referring table of that type, derived from the schema the update
    // stored.
    let described = assert_ok(
        "read the type back",
        emulator
            .call(
                "GetType",
                json!({"keyspaceName": defining, "typeName": "address"}),
            )
            .await,
    );
    assert_eq!(
        described["directReferringTables"],
        json!([table]),
        "the updated table must be reported as a referring table, got {described}"
    );

    // The SAME forms in a keyspace that defines no type resolve against nothing: a type
    // is "contained in the same keyspace" or it does not resolve at all.
    let elsewhere = get_table(&emulator, other, table).await["schemaDefinition"].clone();
    for declared in forms {
        let case = format!("{declared:?} in a keyspace that defines no type");
        let (status, body) = add_columns(
            &emulator,
            other,
            table,
            json!([{"name": "home", "type": declared}]),
        )
        .await;
        assert_failure(&case, (status, body.clone()), 404, "ResourceNotFoundException");
        assert_eq!(
            body["message"],
            json!("UDT address does not exist"),
            "case {case:?}: expected the not-found to name the type, got {body}"
        );
        assert_unchanged(&emulator, other, table, &case, &elsewhere).await;
    }
}

/// A CONSEQUENCE of the rule: because a refused update stores nothing, no schema holds a
/// reference to a type the service never created — so a type of that name created later has
/// no phantom referring table and can be deleted.
#[tokio::test]
async fn a_refused_column_leaves_no_phantom_reference_for_a_type_created_later() {
    let emulator = Emulator::start();
    let keyspace = "phantom_ref_ks";
    let table = "t1";
    active_table(&emulator, keyspace, table).await;

    assert_failure(
        "a column whose type names a type that does not exist yet",
        add_columns(
            &emulator,
            keyspace,
            table,
            json!([{"name": "home", "type": "frozen<later_udt>"}]),
        )
        .await,
        404,
        "ResourceNotFoundException",
    );

    assert_ok(
        "create the type afterwards",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": keyspace,
                    "typeName": "later_udt",
                    "fieldDefinitions": [{"name": "street", "type": "text"}],
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    let described = assert_ok(
        "read the type back",
        emulator
            .call(
                "GetType",
                json!({"keyspaceName": keyspace, "typeName": "later_udt"}),
            )
            .await,
    );
    assert_eq!(
        described["directReferringTables"],
        json!([]),
        "no table may refer to a type the refused update never stored, got {described}"
    );

    // Nothing uses it, so it deletes — the refusal above left no reference behind.
    assert_ok(
        "delete the unused type",
        emulator
            .call(
                "DeleteType",
                json!({"keyspaceName": keyspace, "typeName": "later_udt"}),
            )
            .await,
    );
}

// SPDX-License-Identifier: CC0-1.0

//! In which ORDER this service grades the two faults a create can carry at once — the
//! name of the resource it would store is already taken, and a type expression in the
//! definition it carries names a user-defined type the keyspace cannot resolve — driven
//! end to end over the service's own `awsJson1_0` wire protocol through the shared
//! [`crate::keyspace_table_lifecycle::Emulator`] harness, so what is pinned is the
//! emulator's observable behaviour.
//!
//! The rule under test, exercised over a SET of inputs rather than one example: whether
//! the resource a request addresses can take the change at all — its name not already
//! taken, its stored status one that accepts the change — is settled BEFORE the type
//! expressions of the definition that request carries are resolved, because a request
//! that defines no resource has no definition to resolve. So a create whose name is
//! already taken answers the modelled `ConflictException` (HTTP 400, this RPC
//! service's client-error status) whatever its type
//! expressions say, and every operation that carries a type expression grades the pair
//! the same way: `CreateType` for a field's type, `CreateTable` for a column's, and
//! `UpdateTable` for an added column's (where the addressed resource's fault is a table
//! that is not `ACTIVE` yet).
//!
//! Its consequences, also pinned here: a request whose resource CAN exist still gets the
//! `ResourceNotFoundException` its unresolvable reference earns (the conflict orders
//! those faults, it does not swallow them), and a refused request changes nothing.

use serde_json::{json, Value};

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, error_code, minimal_schema, settle_wait, Emulator,
};

/// The modelled refusal a name already taken earns, on the client-error status every
/// refusal of this RPC service travels with
/// (`service_..._keyspacesservice::scaffolded::CLIENT_ERROR_STATUS`).
const CONFLICT: (u16, &str) = (400, "ConflictException");

/// The modelled refusal an unresolvable type reference earns.
const NOT_FOUND: (u16, &str) = (400, "ResourceNotFoundException");

/// One type expression that cannot be resolved, and the user-defined type name the
/// service must report as missing.
///
/// The rows span where the unresolvable reference sits and how it is written: bare, in
/// another casing (an unquoted name is folded, so the formatted name is what is
/// reported), and nested inside each type constructor the service recognises — the
/// catalog in `service_..._keyspacesservice::state::CQL_TYPE_CONSTRUCTORS`
/// (`frozen`, `list`, `map`, `set`, `tuple`, `vector`), including a `vector` whose
/// dimension is a number token and a `map` whose KEY carries the reference.
struct BadType {
    /// The case name, which also names the fresh resource the row creates.
    name: &'static str,
    /// The type expression the field or column carries.
    expression: &'static str,
    /// The type name the not-found refusal must name.
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
            name: "bare_mixed_case",
            expression: "NoSuchType",
            unresolvable: "nosuchtype",
        },
        BadType {
            name: "frozen",
            expression: "frozen<missing_udt>",
            unresolvable: "missing_udt",
        },
        BadType {
            name: "list_of_frozen",
            expression: "list<frozen<listed_udt>>",
            unresolvable: "listed_udt",
        },
        BadType {
            name: "set_of_frozen",
            expression: "set<frozen<setted_udt>>",
            unresolvable: "setted_udt",
        },
        BadType {
            name: "map_value",
            expression: "map<text, frozen<valued_udt>>",
            unresolvable: "valued_udt",
        },
        BadType {
            name: "map_key",
            expression: "map<frozen<keyed_udt>, text>",
            unresolvable: "keyed_udt",
        },
        BadType {
            name: "tuple_member",
            expression: "tuple<int, tupled_udt>",
            unresolvable: "tupled_udt",
        },
        BadType {
            name: "vector_element",
            expression: "vector<vectored_udt, 3>",
            unresolvable: "vectored_udt",
        },
    ]
}

/// The two creates that carry type expressions in the definition they would store.
#[derive(Clone, Copy, Debug)]
enum Sibling {
    CreateType,
    CreateTable,
}

impl Sibling {
    /// The operation name on the wire.
    fn operation(self) -> &'static str {
        match self {
            Sibling::CreateType => "CreateType",
            Sibling::CreateTable => "CreateTable",
        }
    }

    /// A create of `name` whose definition carries `type_expression`.
    fn request(self, keyspace: &str, name: &str, type_expression: &str) -> Value {
        match self {
            Sibling::CreateType => json!({
                "keyspaceName": keyspace,
                "typeName": name,
                "fieldDefinitions": [{"name": "f", "type": type_expression}],
            }),
            Sibling::CreateTable => json!({
                "keyspaceName": keyspace,
                "tableName": name,
                "schemaDefinition": {
                    "allColumns": [
                        {"name": "pk", "type": "text"},
                        {"name": "c", "type": type_expression},
                    ],
                    "partitionKeys": [{"name": "pk"}],
                },
            }),
        }
    }

    /// The message the service answers a name already taken with.
    fn conflict_message(self, keyspace: &str, name: &str) -> String {
        match self {
            Sibling::CreateType => {
                format!("Cannot add already existing UDT {name} to keyspace {keyspace}")
            }
            Sibling::CreateTable => format!("Table '{keyspace}.{name}' already exists."),
        }
    }

    /// Read one of this sibling's resources back through the operation that reports it.
    async fn read(self, emulator: &Emulator, keyspace: &str, name: &str) -> (u16, Value) {
        match self {
            Sibling::CreateType => {
                emulator
                    .call(
                        "GetType",
                        json!({"keyspaceName": keyspace, "typeName": name}),
                    )
                    .await
            }
            Sibling::CreateTable => {
                emulator
                    .call(
                        "GetTable",
                        json!({"keyspaceName": keyspace, "tableName": name}),
                    )
                    .await
            }
        }
    }
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

/// RULE: of the two faults, the name already taken is the one the caller is told about,
/// while the same definition on a FREE name still earns the not-found its unresolvable
/// reference deserves.
#[tokio::test]
async fn a_name_already_taken_is_graded_before_the_type_references_of_the_definition() {
    let emulator = Emulator::start();
    let keyspace = "grade_order_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    // The two names every row then re-uses, and the one resolvable user-defined type the
    // control rows refer to. Settled once, so both are ACTIVE for every case below.
    assert_ok(
        "create the type whose name is then taken",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": keyspace,
                    "typeName": "addr",
                    "fieldDefinitions": [{"name": "street", "type": "text"}],
                }),
            )
            .await,
    );
    assert_ok(
        "create the table whose name is then taken",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "tbl",
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    for sibling in [Sibling::CreateType, Sibling::CreateTable] {
        let taken = match sibling {
            Sibling::CreateType => "addr",
            Sibling::CreateTable => "tbl",
        };
        for bad in bad_types() {
            // The two faults together: the name is taken AND the definition names a type
            // that does not resolve.
            let case = format!("{} both faults, {}", sibling.operation(), bad.name);
            let got = emulator
                .call(
                    sibling.operation(),
                    sibling.request(keyspace, taken, bad.expression),
                )
                .await;
            let body = got.1.clone();
            assert_failure(&case, got, CONFLICT.0, CONFLICT.1);
            assert_message(&case, &body, &sibling.conflict_message(keyspace, taken));

            // The same definition on a name that is FREE: the conflict ordered the two
            // faults, it did not swallow the unresolvable reference.
            let fresh = format!("fresh_{}_{}", sibling.operation().to_lowercase(), bad.name);
            let case = format!("{} fresh name, {}", sibling.operation(), bad.name);
            let got = emulator
                .call(
                    sibling.operation(),
                    sibling.request(keyspace, &fresh, bad.expression),
                )
                .await;
            let body = got.1.clone();
            assert_failure(&case, got, NOT_FOUND.0, NOT_FOUND.1);
            assert_message(
                &case,
                &body,
                &format!("UDT {} does not exist", bad.unresolvable),
            );

            // Nothing was stored under the free name either.
            let (status, read_body) = sibling.read(&emulator, keyspace, &fresh).await;
            assert_eq!(
                (status, error_code(&read_body).as_str()),
                NOT_FOUND,
                "case {case:?}: a refused create must store nothing, got {status} {read_body}"
            );
        }

        // The control: the name taken with a definition that resolves perfectly is the
        // same conflict, so the refusal above is the name's and not the type's.
        for resolvable in ["int", "frozen<addr>", "list<frozen<addr>>"] {
            let case = format!("{} taken name, resolvable {resolvable}", sibling.operation());
            let got = emulator
                .call(
                    sibling.operation(),
                    sibling.request(keyspace, taken, resolvable),
                )
                .await;
            let body = got.1.clone();
            assert_failure(&case, got, CONFLICT.0, CONFLICT.1);
            assert_message(&case, &body, &sibling.conflict_message(keyspace, taken));
        }
    }

    // Every refused request left the two resources it addressed exactly as they were.
    let got = assert_ok(
        "the type is unchanged",
        emulator
            .call("GetType", json!({"keyspaceName": keyspace, "typeName": "addr"}))
            .await,
    );
    assert_eq!(
        got["fieldDefinitions"],
        json!([{"name": "street", "type": "text"}]),
        "case \"the type is unchanged\": the refused creates must not have touched its \
         fields, got {got}"
    );
    let got = assert_ok(
        "the table is unchanged",
        emulator
            .call("GetTable", json!({"keyspaceName": keyspace, "tableName": "tbl"}))
            .await,
    );
    assert_eq!(
        got["schemaDefinition"],
        minimal_schema("pk"),
        "case \"the table is unchanged\": the refused creates must not have touched its \
         schema, got {got}"
    );
}

/// The same rule where the unresolvable reference names a type that EXISTS but is not
/// `ACTIVE` yet: the name already taken still outranks it, and once the referenced type
/// has settled the very same definition is stored and reads back.
#[tokio::test]
async fn a_name_already_taken_outranks_a_reference_to_a_type_that_is_not_active_yet() {
    let emulator = Emulator::start();
    let keyspace = "grade_order_pending_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    assert_ok(
        "create the type whose name is then taken",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": keyspace,
                    "typeName": "addr",
                    "fieldDefinitions": [{"name": "street", "type": "text"}],
                }),
            )
            .await,
    );
    assert_ok(
        "create the table whose name is then taken",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "tbl",
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    // The reference every case below names: a type whose creation has just been accepted,
    // so it is in the transient status in which it cannot be referred to yet.
    assert_ok(
        "create the type that is still settling",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": keyspace,
                    "typeName": "pending",
                    "fieldDefinitions": [{"name": "zip", "type": "text"}],
                }),
            )
            .await,
    );

    for sibling in [Sibling::CreateType, Sibling::CreateTable] {
        let taken = match sibling {
            Sibling::CreateType => "addr",
            Sibling::CreateTable => "tbl",
        };
        let case = format!("{} taken name, reference to a settling type", sibling.operation());
        let got = emulator
            .call(
                sibling.operation(),
                sibling.request(keyspace, taken, "frozen<pending>"),
            )
            .await;
        let body = got.1.clone();
        assert_failure(&case, got, CONFLICT.0, CONFLICT.1);
        assert_message(&case, &body, &sibling.conflict_message(keyspace, taken));

        let fresh = format!("pending_{}", sibling.operation().to_lowercase());
        let case = format!("{} fresh name, reference to a settling type", sibling.operation());
        let got = emulator
            .call(
                sibling.operation(),
                sibling.request(keyspace, &fresh, "frozen<pending>"),
            )
            .await;
        let body = got.1.clone();
        assert_failure(&case, got, NOT_FOUND.0, NOT_FOUND.1);
        assert_message(&case, &body, "UDT pending does not exist");
    }

    // Once the referenced type is ACTIVE the same definition is accepted on a free name,
    // so the refusals above were about the status it was in and nothing else.
    tokio::time::sleep(settle_wait()).await;
    for sibling in [Sibling::CreateType, Sibling::CreateTable] {
        let fresh = format!("settled_{}", sibling.operation().to_lowercase());
        let case = format!("{} fresh name, reference to a settled type", sibling.operation());
        assert_ok(
            &case,
            emulator
                .call(
                    sibling.operation(),
                    sibling.request(keyspace, &fresh, "frozen<pending>"),
                )
                .await,
        );
        let (status, body) = sibling.read(&emulator, keyspace, &fresh).await;
        let stored = match sibling {
            Sibling::CreateType => body.pointer("/fieldDefinitions/0/type").cloned(),
            Sibling::CreateTable => body.pointer("/schemaDefinition/allColumns/1/type").cloned(),
        };
        assert_eq!(
            (status, stored),
            (200, Some(json!("frozen<pending>"))),
            "case {case:?}: the accepted definition must read back verbatim, got {body}"
        );
    }
}

/// The rule beyond the creates: `UpdateTable` resolves an added column's type only once
/// the table it addresses is one that can take the change, so a table still settling
/// answers the modelled conflict whatever `addColumns` says — and the same request
/// against the settled table earns the not-found its unresolvable reference deserves.
#[tokio::test]
async fn update_table_grades_a_table_that_cannot_take_the_change_before_the_added_types() {
    let emulator = Emulator::start();
    let keyspace = "grade_order_update_ks";
    let table = "settling_tbl";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    assert_ok(
        "create the table that is then still settling",
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

    let add_columns = |type_expression: &str| {
        json!({
            "keyspaceName": keyspace,
            "tableName": table,
            "addColumns": [{"name": "added", "type": type_expression}],
        })
    };

    // While the table is in its transient creation status, every row of the catalog is
    // the conflict: the table cannot take the change at all.
    for bad in bad_types() {
        let case = format!("UpdateTable while settling, {}", bad.name);
        let got = emulator
            .call("UpdateTable", add_columns(bad.expression))
            .await;
        assert_failure(&case, got, CONFLICT.0, CONFLICT.1);
    }

    tokio::time::sleep(settle_wait()).await;

    // Now the table can take the change, so the added column's type is resolved and the
    // unresolvable reference is what refuses the request.
    for bad in bad_types() {
        let case = format!("UpdateTable once ACTIVE, {}", bad.name);
        let got = emulator
            .call("UpdateTable", add_columns(bad.expression))
            .await;
        let body = got.1.clone();
        assert_failure(&case, got, NOT_FOUND.0, NOT_FOUND.1);
        assert_message(
            &case,
            &body,
            &format!("UDT {} does not exist", bad.unresolvable),
        );
    }

    // None of the refusals grew the schema.
    let got = assert_ok(
        "the schema is unchanged",
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    );
    assert_eq!(
        got["schemaDefinition"],
        minimal_schema("pk"),
        "case \"the schema is unchanged\": the refused updates must leave the schema as it \
         was, got {got}"
    );
}

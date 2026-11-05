// SPDX-License-Identifier: CC0-1.0

//! The sentence Amazon Keyspaces words a refused DROP of a keyspace with, and the one it
//! words a refused delete of a type something still uses with — driven end to end over
//! this service's own `awsJson1_0` wire protocol through the shared
//! [`crate::keyspace_table_lifecycle::Emulator`] harness, so what is pinned is the
//! emulator's observable reply rather than its internals.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. A keyspace that is not there is refused in the words of the fault the request hit,
//!    so the operation that DROPS a keyspace states a failed drop — `Cannot drop non
//!    existing keyspace '<keyspaceName>'`, the requested name in single quotes with no
//!    trailing period — where an operation that merely ACCESSES the keyspace states a
//!    failed lookup (`Keyspace <keyspaceName> does not exist`). The rows span the modelled
//!    `keyspaceName` shape (`len 1..48`, pattern `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`) —
//!    shortest, longest, digit-led, mixed case, a prefix of a name that IS held — and the
//!    two ways a keyspace comes to be absent (never created; dropped and settled), and the
//!    sentence carries nothing of the caller's credential scope.
//! 2. A delete of a user-defined type something still uses states the fault and nothing
//!    else: the FORMATTED type name plus the fixed clause about the kinds of user a type
//!    can have (`<typeName> is referenced by other UDTs or tables.`). The sentence
//!    therefore does not vary with which resources refer to the type or how many there
//!    are, and it discloses neither the keyspace nor any sibling resource name. The rows
//!    span the kind of user (a table column, another type's field, both at once), how many
//!    there are (one, two, three), the type expression the user reaches the type through,
//!    and the two forms a `typeName` takes (an unquoted name the service folds, a
//!    double-quoted one it preserves).

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::scaffolded;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, error_code, ensure_keyspace, keyspace_arn, minimal_schema,
    settle_wait, Emulator,
};
use crate::regional_partitioning::RegionalEmulator;

/// The `message` member of a failure body, as a string.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message member>")
        .to_string()
}

/// The sentence a failed DROP of `keyspace_name` carries.
fn drop_sentence(keyspace_name: &str) -> String {
    format!("Cannot drop non existing keyspace '{keyspace_name}'")
}

/// The sentence a failed LOOKUP of `keyspace_name` carries.
fn lookup_sentence(keyspace_name: &str) -> String {
    format!("Keyspace {keyspace_name} does not exist")
}

/// RULE 1: `DeleteKeyspace` on a name nothing holds answers the drop sentence, and an
/// operation that only reads the very same name answers the lookup sentence, so the two
/// faults are told apart.
#[tokio::test]
async fn a_drop_of_a_keyspace_nothing_holds_is_refused_as_a_failed_drop() {
    let emulator = Emulator::start();
    let held = "absence_holder_ks";
    ensure_keyspace(&emulator, held).await;
    tokio::time::sleep(settle_wait()).await;

    let longest = format!("absent_{}", "n".repeat(41));
    // The rows span the published `keyspaceName` bounds and forms; none of them names a
    // keyspace this emulator holds.
    let rows: Vec<(&str, String)> = vec![
        ("shortest", "z".to_string()),
        ("longest", longest.clone()),
        ("digit_led", "9absent_ks".to_string()),
        ("mixed_case", "Absent_Ks9".to_string()),
        ("underscores", "absent__ks".to_string()),
        // A prefix of a name that IS held is still a name nothing holds.
        ("prefix_of_a_held_name", "absence_holder".to_string()),
    ];

    for (case, name) in &rows {
        let (status, body) = emulator
            .call("DeleteKeyspace", json!({"keyspaceName": name}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ResourceNotFoundException"),
            "case {case:?}: expected the modelled not-found, got {status} {body}"
        );
        let refusal = message(&body);
        assert_eq!(
            refusal,
            drop_sentence(name),
            "case {case:?}: expected the drop sentence, got {refusal:?}"
        );
        // The modelled member that says WHICH resource was looked for is still filled in.
        assert_eq!(
            body["resourceArn"],
            json!(keyspace_arn(name)),
            "case {case:?}: expected the keyspace ARN the drop looked for, got {body}"
        );

        // The read path words the SAME absence as a failed lookup, so a caller can tell a
        // failed drop from a failed read.
        let (status, read_body) = emulator
            .call("GetKeyspace", json!({"keyspaceName": name}))
            .await;
        assert_eq!(
            (status, error_code(&read_body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ResourceNotFoundException"),
            "case {case:?}: expected the modelled not-found, got {status} {read_body}"
        );
        let read_refusal = message(&read_body);
        assert_eq!(
            read_refusal,
            lookup_sentence(name),
            "case {case:?}: expected the lookup sentence, got {read_refusal:?}"
        );
        assert_ne!(
            refusal, read_refusal,
            "case {case:?}: the drop and the lookup must not share one sentence, got \
             {refusal:?}"
        );
    }

    // The keyspace that IS held was not touched by any of the refusals above.
    let reported = assert_ok(held, emulator.call("GetKeyspace", json!({"keyspaceName": held})).await);
    assert_eq!(
        reported["keyspaceName"],
        json!(held),
        "case {held:?}: the refused drops must leave a held keyspace alone, got {reported}"
    );
}

/// RULE 1, for the other way a keyspace comes to be absent: a keyspace that WAS held and
/// whose drop has completed is refused in the same drop sentence, which is the re-delete
/// boundary a client reads to learn the keyspace is already gone.
#[tokio::test]
async fn a_re_drop_after_the_cascade_completed_is_refused_as_a_failed_drop() {
    let emulator = Emulator::start();
    // The rows differ in what the keyspace held when it was dropped, because the sentence
    // is about the keyspace being gone and not about what went with it.
    let rows: [(&str, &str, &[&str]); 3] = [
        ("an empty keyspace", "redrop_empty_ks", &[]),
        ("a keyspace holding one table", "redrop_one_table_ks", &["tbl_one"]),
        (
            "a keyspace holding several tables",
            "redrop_many_tables_ks",
            &["tbl_a", "tbl_b", "tbl_c"],
        ),
    ];

    for (case, keyspace, tables) in rows {
        ensure_keyspace(&emulator, keyspace).await;
        for table in tables {
            assert_ok(
                case,
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
        }
        tokio::time::sleep(settle_wait()).await;

        assert_ok(
            case,
            emulator
                .call("DeleteKeyspace", json!({"keyspaceName": keyspace}))
                .await,
        );
        // The clock completes the cascade: neither the keyspace nor any table it held is
        // there any more.
        tokio::time::sleep(settle_wait()).await;
        assert_failure(
            case,
            emulator
                .call("GetKeyspace", json!({"keyspaceName": keyspace}))
                .await,
            scaffolded::CLIENT_ERROR_STATUS,
            "ResourceNotFoundException",
        );
        for table in tables {
            assert_failure(
                case,
                emulator
                    .call(
                        "GetTable",
                        json!({"keyspaceName": keyspace, "tableName": table}),
                    )
                    .await,
                scaffolded::CLIENT_ERROR_STATUS,
                "ResourceNotFoundException",
            );
        }

        let (status, body) = emulator
            .call("DeleteKeyspace", json!({"keyspaceName": keyspace}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ResourceNotFoundException"),
            "case {case:?}: expected the modelled not-found, got {status} {body}"
        );
        let refusal = message(&body);
        assert_eq!(
            refusal,
            drop_sentence(keyspace),
            "case {case:?}: expected the drop sentence, got {refusal:?}"
        );
    }
}

/// RULE 1, on what the sentence may carry: the names of the REQUEST and nothing of the
/// caller's credential scope, so the same drop refused from three Regions is refused in
/// the same words.
#[tokio::test]
async fn the_drop_sentence_carries_nothing_of_the_credential_scope() {
    let emulator = RegionalEmulator::start();
    let name = "scope_free_drop_ks";
    for region in ["us-west-2", "eu-central-1", "ap-southeast-1"] {
        let (status, body) = emulator
            .call(region, "DeleteKeyspace", json!({"keyspaceName": name}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ResourceNotFoundException"),
            "case {region:?}: expected the modelled not-found, got {status} {body}"
        );
        let refusal = message(&body);
        assert_eq!(
            refusal,
            drop_sentence(name),
            "case {region:?}: the sentence must not depend on the Region, got {refusal:?}"
        );
        assert!(
            !refusal.contains(region),
            "case {region:?}: the sentence must carry nothing of the credential scope, got \
             {refusal:?}"
        );
    }
}

/// One resource of the keyspace that refers to the type under test.
struct Referrer {
    /// The table the row creates, with the column type it reaches the type through.
    table: Option<(&'static str, String)>,
    /// The type the row creates, with the field type it reaches the type through.
    parent_type: Option<(&'static str, String)>,
}

/// RULE 2: the still-referenced refusal is the formatted type name plus the fixed clause,
/// whichever and however many resources refer to the type.
#[tokio::test]
async fn a_type_something_still_uses_is_refused_with_the_fault_and_nothing_else() {
    /// One row: the `typeName` the request supplies and the resources that refer to it.
    struct Row {
        case: &'static str,
        /// The `typeName` exactly as the request supplies it.
        submitted: &'static str,
        referrers: Vec<Referrer>,
    }

    let emulator = Emulator::start();
    let ks = "referenced_type_ks";
    ensure_keyspace(&emulator, ks).await;

    // The rows differ in the kind of user, the number of users, the type expression each
    // user reaches the type through, and the form of the `typeName` the request supplies.
    let rows: Vec<Row> = vec![
        Row {
            case: "one table column",
            submitted: "used_by_a_column",
            referrers: vec![Referrer {
                table: Some(("one_column_tbl", "frozen<used_by_a_column>".to_string())),
                parent_type: None,
            }],
        },
        Row {
            case: "one parent type's field",
            submitted: "used_by_a_field",
            referrers: vec![Referrer {
                table: None,
                parent_type: Some(("one_field_type", "frozen<used_by_a_field>".to_string())),
            }],
        },
        Row {
            case: "a table and a parent type at once",
            submitted: "used_by_both",
            referrers: vec![Referrer {
                table: Some(("both_tbl", "map<text, frozen<used_by_both>>".to_string())),
                parent_type: Some(("both_type", "list<frozen<used_by_both>>".to_string())),
            }],
        },
        Row {
            case: "three users, so the count of them could show",
            submitted: "used_by_three",
            referrers: vec![
                Referrer {
                    table: Some(("three_first_tbl", "frozen<used_by_three>".to_string())),
                    parent_type: None,
                },
                Referrer {
                    table: Some((
                        "three_second_tbl",
                        "set<frozen<used_by_three>>".to_string(),
                    )),
                    parent_type: Some((
                        "three_parent_type",
                        "frozen<used_by_three>".to_string(),
                    )),
                },
            ],
        },
        Row {
            // An unquoted name is folded, so the sentence carries the FORMATTED name the
            // keyspace holds the type under rather than the letters the request supplied.
            case: "a name the service folds",
            submitted: "UsedByAFoldedName",
            referrers: vec![Referrer {
                table: Some(("folded_tbl", "frozen<UsedByAFoldedName>".to_string())),
                parent_type: None,
            }],
        },
        Row {
            // A double-quoted name is preserved verbatim, quotes included.
            case: "a double-quoted name",
            submitted: "\"UsedQuoted\"",
            referrers: vec![Referrer {
                table: Some(("quoted_tbl", "frozen<\"UsedQuoted\">".to_string())),
                parent_type: None,
            }],
        },
    ];

    for row in &rows {
        assert_ok(
            row.case,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": ks,
                        "typeName": row.submitted,
                        "fieldDefinitions": [{"name": "f", "type": "text"}],
                    }),
                )
                .await,
        );
    }
    // A column or field type may only name a type that is `ACTIVE`.
    tokio::time::sleep(settle_wait()).await;
    for row in &rows {
        for referrer in &row.referrers {
            if let Some((table, column_type)) = &referrer.table {
                assert_ok(
                    row.case,
                    emulator
                        .call(
                            "CreateTable",
                            json!({
                                "keyspaceName": ks,
                                "tableName": table,
                                "schemaDefinition": {
                                    "allColumns": [
                                        {"name": "pk", "type": "text"},
                                        {"name": "value", "type": column_type},
                                    ],
                                    "partitionKeys": [{"name": "pk"}],
                                },
                            }),
                        )
                        .await,
                );
            }
            if let Some((parent, field_type)) = &referrer.parent_type {
                assert_ok(
                    row.case,
                    emulator
                        .call(
                            "CreateType",
                            json!({
                                "keyspaceName": ks,
                                "typeName": parent,
                                "fieldDefinitions": [{"name": "value", "type": field_type}],
                            }),
                        )
                        .await,
                );
            }
        }
    }
    tokio::time::sleep(settle_wait()).await;

    for row in &rows {
        let formatted = keyspaces::format_type_name(row.submitted)
            .expect("every submitted name above is a form a typeName may take");
        let (status, body) = emulator
            .call(
                "DeleteType",
                json!({"keyspaceName": ks, "typeName": row.submitted}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ConflictException"),
            "case {:?}: expected the modelled conflict, got {status} {body}",
            row.case
        );
        let refusal = message(&body);
        assert_eq!(
            refusal,
            format!("{formatted} is referenced by other UDTs or tables."),
            "case {:?}: unexpected refusal sentence, got {refusal:?}",
            row.case
        );
        // The sentence names neither the keyspace nor any resource that refers to the
        // type, so it discloses no sibling name and does not grow with the users.
        assert!(
            !refusal.contains(ks),
            "case {:?}: the sentence must not name the keyspace, got {refusal:?}",
            row.case
        );
        for referrer in &row.referrers {
            for name in [
                referrer.table.as_ref().map(|(table, _)| *table),
                referrer.parent_type.as_ref().map(|(parent, _)| *parent),
            ]
            .into_iter()
            .flatten()
            {
                assert!(
                    !refusal.contains(name),
                    "case {:?}: the sentence must not name the user {name:?}, got {refusal:?}",
                    row.case
                );
            }
        }

        // The refusal wrote nothing: the type is still there and still `ACTIVE`.
        let reported = assert_ok(
            row.case,
            emulator
                .call(
                    "GetType",
                    json!({"keyspaceName": ks, "typeName": row.submitted}),
                )
                .await,
        );
        assert_eq!(
            reported["status"],
            json!(keyspaces::TYPE_STATUS_ACTIVE),
            "case {:?}: a refused delete must leave the type {}, got {reported}",
            row.case,
            keyspaces::TYPE_STATUS_ACTIVE
        );
    }

    // And the refusal really was about the references: with every user dropped, the very
    // same request is accepted and the type reads back on its way out.
    for row in &rows {
        for referrer in &row.referrers {
            if let Some((table, _)) = &referrer.table {
                assert_ok(
                    row.case,
                    emulator
                        .call(
                            "DeleteTable",
                            json!({"keyspaceName": ks, "tableName": table}),
                        )
                        .await,
                );
            }
            if let Some((parent, _)) = &referrer.parent_type {
                assert_ok(
                    row.case,
                    emulator
                        .call(
                            "DeleteType",
                            json!({"keyspaceName": ks, "typeName": parent}),
                        )
                        .await,
                );
            }
        }
        let accepted = assert_ok(
            row.case,
            emulator
                .call(
                    "DeleteType",
                    json!({"keyspaceName": ks, "typeName": row.submitted}),
                )
                .await,
        );
        let formatted = keyspaces::format_type_name(row.submitted)
            .expect("every submitted name above is a form a typeName may take");
        assert_eq!(
            accepted["typeName"],
            json!(formatted),
            "case {:?}: expected the deleted type's formatted name, got {accepted}",
            row.case
        );
        let reported = assert_ok(
            row.case,
            emulator
                .call(
                    "GetType",
                    json!({"keyspaceName": ks, "typeName": row.submitted}),
                )
                .await,
        );
        assert_eq!(
            reported["status"],
            json!(keyspaces::TYPE_STATUS_DELETING),
            "case {:?}: the accepted delete must put the type in {}, got {reported}",
            row.case,
            keyspaces::TYPE_STATUS_DELETING
        );
    }
}

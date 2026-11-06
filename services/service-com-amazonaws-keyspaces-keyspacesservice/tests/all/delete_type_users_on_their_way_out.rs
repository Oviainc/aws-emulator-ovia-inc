// SPDX-License-Identifier: CC0-1.0

//! `DeleteType` and a user of the type whose OWN deletion the service has already
//! accepted, driven end to end over this service's `awsJson1_0` wire protocol through the
//! shared [`crate::keyspace_table_lifecycle::Emulator`] harness.
//!
//! The rule under test: "You can only delete a type that is not used in a table or
//! another UDT", where the users of a type are the resources of its keyspace that refer
//! to it AND are not themselves on their way out. A resource whose deletion this service
//! has already accepted holds the transient `DELETING` status, takes no further change
//! and can never be referred to again, so the reference it carries has stopped counting
//! and it no longer blocks the deletion of a type it refers to. A user in any OTHER
//! status still blocks it, and what the keyspace HOLDS is unchanged: `GetType` keeps
//! reporting a departing user under `directReferringTables` / `directParentTypes`.
//!
//! Each rule is exercised over a SET of rows: both kinds of user (a table column and
//! another type's field), several type expressions through which a user reaches the type,
//! one user and several at once, and — for the refusal — every other status a user can be
//! in.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::delete_type::{create_type, get_type};
use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, ensure_keyspace, keyspace_arn, settle_wait, Emulator,
};

/// One resource of the keyspace that refers to the type under test.
#[derive(Clone, Copy, Debug)]
enum User {
    /// A table one of whose columns carries the type expression.
    Table {
        name: &'static str,
        column_type: &'static str,
    },
    /// Another user-defined type one of whose fields carries it.
    Type {
        name: &'static str,
        field_type: &'static str,
    },
}

impl User {
    /// This user's own resource name, which is how its own reader addresses it.
    fn name(self) -> &'static str {
        match self {
            Self::Table { name, .. } | Self::Type { name, .. } => name,
        }
    }

    /// The `GetType` member a report of the used type names this user under.
    fn reported_under(self) -> &'static str {
        match self {
            Self::Table { .. } => "directReferringTables",
            Self::Type { .. } => "directParentTypes",
        }
    }

    /// The status this user holds once its own deletion has been accepted.
    fn deleting_status(self) -> &'static str {
        match self {
            Self::Table { .. } => keyspaces::TABLE_STATUS_DELETING,
            Self::Type { .. } => keyspaces::TYPE_STATUS_DELETING,
        }
    }

    /// Create this user in `ks`, referring to the type under test through its own type
    /// expression.
    async fn create(self, emulator: &Emulator, case: &str, ks: &str) {
        let created = match self {
            Self::Table { name, column_type } => emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": ks,
                        "tableName": name,
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
            Self::Type { name, field_type } => emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": ks,
                        "typeName": name,
                        "fieldDefinitions": [{"name": "value", "type": field_type}],
                    }),
                )
                .await,
        };
        assert_ok(case, created);
    }

    /// Ask the service to delete this user. That only ACCEPTS the deletion: the resource
    /// stays in the keyspace, in `DELETING`, until the clock removes it.
    async fn delete(self, emulator: &Emulator, case: &str, ks: &str) {
        let requested = match self {
            Self::Table { name, .. } => emulator
                .call(
                    "DeleteTable",
                    json!({"keyspaceName": ks, "tableName": name}),
                )
                .await,
            Self::Type { name, .. } => emulator
                .call("DeleteType", json!({"keyspaceName": ks, "typeName": name}))
                .await,
        };
        assert_ok(case, requested);
    }

    /// This user's status, as its own reader reports it.
    async fn status(self, emulator: &Emulator, case: &str, ks: &str) -> Value {
        let reported = match self {
            Self::Table { name, .. } => assert_ok(
                case,
                emulator
                    .call("GetTable", json!({"keyspaceName": ks, "tableName": name}))
                    .await,
            ),
            Self::Type { name, .. } => assert_ok(case, get_type(emulator, ks, name).await),
        };
        reported["status"].clone()
    }
}

/// Call `DeleteType` for `type_name`.
async fn delete_type(emulator: &Emulator, ks: &str, type_name: &str) -> (u16, Value) {
    emulator
        .call(
            "DeleteType",
            json!({"keyspaceName": ks, "typeName": type_name}),
        )
        .await
}

/// The `message` of a failure body, for a refusal that has to name what blocked it.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message member>")
        .to_string()
}

/// RULE: every user of the type having had its own deletion accepted, the type is no
/// longer used and `DeleteType` is accepted — inside the span those users are still on
/// their way out in, and without the keyspace's report of the type changing.
#[tokio::test]
async fn delete_type_accepts_a_type_whose_users_are_already_being_deleted() {
    /// One row: the type under test and the resources that refer to it.
    struct Case {
        case: &'static str,
        used: &'static str,
        users: Vec<User>,
    }

    // The rows differ in the KIND of user, in the type expression through which it
    // reaches the type (directly frozen, inside a map, a list or a set) and in how many
    // users there are, because the rule is about a user being on its way out and not
    // about the shape of the reference or the number of them.
    let cases: Vec<Case> = vec![
        Case {
            case: "a table column of the type",
            used: "left_by_a_column",
            users: vec![User::Table {
                name: "departing_column_tbl",
                column_type: "frozen<left_by_a_column>",
            }],
        },
        Case {
            case: "a table column holding the type in a collection",
            used: "left_by_a_collection",
            users: vec![User::Table {
                name: "departing_collection_tbl",
                column_type: "map<text, frozen<left_by_a_collection>>",
            }],
        },
        Case {
            case: "another type's field of the type",
            used: "left_by_a_field",
            users: vec![User::Type {
                name: "departing_field_type",
                field_type: "frozen<left_by_a_field>",
            }],
        },
        Case {
            case: "another type's field holding the type in a collection",
            used: "left_by_a_nested_field",
            users: vec![User::Type {
                name: "departing_nested_type",
                field_type: "list<frozen<left_by_a_nested_field>>",
            }],
        },
        Case {
            case: "a table and another type at once",
            used: "left_by_both",
            users: vec![
                User::Table {
                    name: "departing_both_tbl",
                    column_type: "set<frozen<left_by_both>>",
                },
                User::Type {
                    name: "departing_both_type",
                    field_type: "frozen<left_by_both>",
                },
            ],
        },
    ];

    let emulator = Emulator::start();
    let ks = "delete_type_departing_users";
    ensure_keyspace(&emulator, ks).await;

    for case in &cases {
        create_type(&emulator, case.case, ks, case.used).await;
    }
    // A type can only be referred to once it is ACTIVE.
    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        for user in &case.users {
            user.create(&emulator, case.case, ks).await;
        }
    }
    tokio::time::sleep(settle_wait()).await;

    for case in &cases {
        // While its users are as they were created, the type IS used and the delete is
        // refused: the only thing that changes below is that their deletions have been
        // accepted.
        let (status, body) = delete_type(&emulator, ks, case.used).await;
        assert_failure(case.case, (status, body), 400, "ConflictException");

        for user in &case.users {
            user.delete(&emulator, case.case, ks).await;
            let reported = user.status(&emulator, case.case, ks).await;
            assert_eq!(
                reported,
                json!(user.deleting_status()),
                "case {:?}: the accepted deletion of {:?} must leave it in {}, got {reported}",
                case.case,
                user.name(),
                user.deleting_status()
            );
        }

        // What the keyspace HOLDS has not changed: a resource in DELETING is still a
        // resource of the keyspace, so the report of the type still names every user.
        let reported = assert_ok(case.case, get_type(&emulator, ks, case.used).await);
        for user in &case.users {
            let listed = reported[user.reported_under()].clone();
            assert!(
                listed
                    .as_array()
                    .is_some_and(|names| names.contains(&json!(user.name()))),
                "case {:?}: {} must still report the departing {:?}, got {reported}",
                case.case,
                user.reported_under(),
                user.name()
            );
        }

        // Nothing uses the type any more, so the very same request is now accepted —
        // inside the span its users are still on their way out in.
        let deleted = assert_ok(case.case, delete_type(&emulator, ks, case.used).await);
        assert_eq!(
            deleted["typeName"],
            json!(case.used),
            "case {:?}: expected the deleted type's name, got {deleted}",
            case.case
        );
        assert_eq!(
            deleted["keyspaceArn"],
            json!(keyspace_arn(ks)),
            "case {:?}: expected the published keyspace ARN, got {deleted}",
            case.case
        );
        let reported = assert_ok(case.case, get_type(&emulator, ks, case.used).await);
        assert_eq!(
            reported["status"],
            json!(keyspaces::TYPE_STATUS_DELETING),
            "case {:?}: the accepted delete must put the type in {}, got {reported}",
            case.case,
            keyspaces::TYPE_STATUS_DELETING
        );
    }

    // The clock completes every removal that was accepted, so none of the types or their
    // users is a resource of the keyspace any more.
    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        assert_failure(
            case.case,
            get_type(&emulator, ks, case.used).await,
            400,
            "ResourceNotFoundException",
        );
    }
}

/// RULE, the other half: a user that is NOT on its way out still blocks the deletion,
/// whatever status it is in, and the refusal states the fault about the type alone.
#[tokio::test]
async fn delete_type_refuses_a_type_whose_user_is_not_on_its_way_out() {
    /// What a row leaves its user in before the delete is attempted.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Leave {
        /// Settled: the user's own creation has completed.
        Settled,
        /// The user's own creation is still in flight.
        Creating,
        /// A change to the settled user is still in flight.
        Updating,
    }

    /// One row: the type under test, the user that blocks its deletion, how the row
    /// leaves that user, and the status it is then in.
    struct Row {
        case: &'static str,
        used: &'static str,
        user: User,
        leave: Leave,
        status: &'static str,
    }

    // The rows span every status a user can be in other than the one that stops it
    // counting: settled, its own creation in flight, and a change to it in flight.
    let rows: Vec<Row> = vec![
        Row {
            case: "a table that has settled",
            used: "used_by_a_settled_table",
            user: User::Table {
                name: "settled_user_tbl",
                column_type: "frozen<used_by_a_settled_table>",
            },
            leave: Leave::Settled,
            status: keyspaces::TABLE_STATUS_ACTIVE,
        },
        Row {
            case: "a table whose own creation is still in flight",
            used: "used_by_a_creating_table",
            user: User::Table {
                name: "creating_user_tbl",
                column_type: "list<frozen<used_by_a_creating_table>>",
            },
            leave: Leave::Creating,
            status: keyspaces::TABLE_STATUS_CREATING,
        },
        Row {
            case: "a table whose own update is still in flight",
            used: "used_by_an_updating_table",
            user: User::Table {
                name: "updating_user_tbl",
                column_type: "frozen<used_by_an_updating_table>",
            },
            leave: Leave::Updating,
            status: keyspaces::TABLE_STATUS_UPDATING,
        },
        Row {
            case: "another type that has settled",
            used: "used_by_a_settled_type",
            user: User::Type {
                name: "settled_user_type",
                field_type: "frozen<used_by_a_settled_type>",
            },
            leave: Leave::Settled,
            status: keyspaces::TYPE_STATUS_ACTIVE,
        },
        Row {
            case: "another type whose own creation is still in flight",
            used: "used_by_a_creating_type",
            user: User::Type {
                name: "creating_user_type",
                field_type: "map<text, frozen<used_by_a_creating_type>>",
            },
            leave: Leave::Creating,
            status: keyspaces::TYPE_STATUS_CREATING,
        },
    ];

    let emulator = Emulator::start();
    let ks = "delete_type_blocking_users";
    ensure_keyspace(&emulator, ks).await;

    for row in &rows {
        create_type(&emulator, row.case, ks, row.used).await;
    }
    tokio::time::sleep(settle_wait()).await;

    // A user that must be settled — or settled before a change is started on it — is
    // created first and waited out.
    for row in &rows {
        if row.leave != Leave::Creating {
            row.user.create(&emulator, row.case, ks).await;
        }
    }
    tokio::time::sleep(settle_wait()).await;

    // What leaves each remaining row's user mid-change, which nothing waits out.
    for row in &rows {
        match row.leave {
            Leave::Settled => {}
            Leave::Creating => row.user.create(&emulator, row.case, ks).await,
            Leave::Updating => {
                assert_ok(
                    row.case,
                    emulator
                        .call(
                            "UpdateTable",
                            json!({
                                "keyspaceName": ks,
                                "tableName": row.user.name(),
                                "addColumns": [{"name": "extra", "type": "int"}],
                            }),
                        )
                        .await,
                );
            }
        }
    }

    for row in &rows {
        // The row's premise: the user really is in the status the row claims.
        let reported = row.user.status(&emulator, row.case, ks).await;
        assert_eq!(
            reported,
            json!(row.status),
            "case {:?}: expected the user to be in {}, got {reported}",
            row.case,
            row.status
        );

        let (status, body) = delete_type(&emulator, ks, row.used).await;
        assert_failure(row.case, (status, body.clone()), 400, "ConflictException");
        // The sentence states the fault and nothing else: the type the request named,
        // never the user that blocked it nor the keyspace it lives in.
        let refusal = message(&body);
        assert_eq!(
            refusal,
            format!("{} is referenced by other UDTs or tables.", row.used),
            "case {:?}: unexpected refusal sentence, got {refusal:?}",
            row.case
        );
        assert!(
            !refusal.contains(row.user.name()) && !refusal.contains(ks),
            "case {:?}: the refusal must disclose neither the user {:?} nor the keyspace, \
             got {refusal:?}",
            row.case,
            row.user.name()
        );

        // The refused delete wrote nothing: the type is still there and still usable.
        let reported = assert_ok(row.case, get_type(&emulator, ks, row.used).await);
        assert_eq!(
            reported["status"],
            json!(keyspaces::TYPE_STATUS_ACTIVE),
            "case {:?}: a refused delete must leave the type {}, got {reported}",
            row.case,
            keyspaces::TYPE_STATUS_ACTIVE
        );
    }
}

/// RULE, per user: the filter is per resource rather than all-or-nothing — a type with
/// several users is refused while ANY of them is not on its way out, in the same words
/// whichever and however many they are, and it is accepted once the last one's deletion is
/// accepted too.
#[tokio::test]
async fn delete_type_counts_each_user_on_its_own() {
    let departing = [
        User::Table {
            name: "mixed_departing_tbl",
            column_type: "frozen<used_by_four>",
        },
        User::Type {
            name: "mixed_departing_type",
            field_type: "frozen<used_by_four>",
        },
    ];
    let remaining = [
        User::Table {
            name: "mixed_remaining_tbl",
            column_type: "map<text, frozen<used_by_four>>",
        },
        User::Type {
            name: "mixed_remaining_type",
            field_type: "list<frozen<used_by_four>>",
        },
    ];

    let case = "four users, two of them on their way out";
    let emulator = Emulator::start();
    let ks = "delete_type_mixed_users";
    ensure_keyspace(&emulator, ks).await;
    create_type(&emulator, case, ks, "used_by_four").await;
    tokio::time::sleep(settle_wait()).await;
    for user in departing.iter().chain(remaining.iter()) {
        user.create(&emulator, case, ks).await;
    }
    tokio::time::sleep(settle_wait()).await;

    for user in &departing {
        user.delete(&emulator, case, ks).await;
    }

    // Two users are on their way out and two are not, so the type is still used — and the
    // refusal is the same one-user sentence, naming no user at all however many there are.
    let (status, body) = delete_type(&emulator, ks, "used_by_four").await;
    assert_failure(case, (status, body.clone()), 400, "ConflictException");
    let refusal = message(&body);
    assert_eq!(
        refusal,
        "used_by_four is referenced by other UDTs or tables.",
        "case {case:?}: unexpected refusal sentence, got {refusal:?}"
    );
    for user in departing.iter().chain(remaining.iter()) {
        assert!(
            !refusal.contains(user.name()),
            "case {case:?}: the refusal must not name the user {:?}, got {refusal:?}",
            user.name()
        );
    }

    // The last two deletions accepted, nothing uses the type and the same request is
    // accepted.
    for user in &remaining {
        user.delete(&emulator, case, ks).await;
    }
    let deleted = assert_ok(case, delete_type(&emulator, ks, "used_by_four").await);
    assert_eq!(
        deleted["typeName"],
        json!("used_by_four"),
        "case {case:?}: expected the deleted type's name, got {deleted}"
    );
}

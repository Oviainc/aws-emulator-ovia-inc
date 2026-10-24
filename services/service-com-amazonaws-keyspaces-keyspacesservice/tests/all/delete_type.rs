// SPDX-License-Identifier: CC0-1.0

//! `DeleteType`, driven end to end over the service's own `awsJson1_0` wire protocol
//! through the shared [`crate::keyspace_table_lifecycle::Emulator`] harness.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. `DeleteType` deletes the type its FORMATTED name addresses and answers with that
//!    name and the ARN of the keyspace that held it: the type enters the transient
//!    `DELETING` status the clock settles into absence, after which neither `GetType`
//!    nor `ListTypes` reports it.
//! 2. "You can only delete a type that is not used in a table or another UDT": a type
//!    something still uses is refused with the modelled `ConflictException` and is left
//!    exactly as it was, and the same request succeeds once nothing uses it. The users
//!    are derived from the keyspace's current contents, however a column or field type
//!    reaches the type.
//! 3. A type with an action of its own already in progress — the creation `CreateType`
//!    started, or a deletion this operation started — is refused with the modelled
//!    `ConflictException`, and the action in flight is not disturbed.
//! 4. A name that addresses no type is refused with the error the model declares for it:
//!    404 `ResourceNotFoundException` for an absent keyspace or type, 400
//!    `ValidationException` for a name that is neither form a `typeName` may take, and
//!    for a service-managed keyspace, which holds none of the caller's types.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::TYPE_STATUS_VALUES;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, keyspace_arn, settle_wait, Emulator, REGION,
};

/// The ARN a `ResourceNotFoundException` names for a type that could not be found.
fn type_arn(keyspace: &str, type_name: &str) -> String {
    format!(
        "arn:aws:cassandra:{REGION}:{}:/keyspace/{keyspace}/type/{type_name}",
        engine::emulator_account_id()
    )
}

/// Create a keyspace and hand its name back.
async fn keyspace(emulator: &Emulator, name: &'static str) -> &'static str {
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": name}))
            .await,
    );
    name
}

/// Create one type with a single scalar field.
async fn create_type(emulator: &Emulator, case: &str, ks: &str, type_name: &str) {
    assert_ok(
        case,
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": ks,
                    "typeName": type_name,
                    "fieldDefinitions": [{"name": "value", "type": "int"}],
                }),
            )
            .await,
    );
}

/// `GetType`'s report of one type, or the failure it answered with.
async fn get_type(emulator: &Emulator, ks: &str, type_name: &str) -> (u16, Value) {
    emulator
        .call("GetType", json!({"keyspaceName": ks, "typeName": type_name}))
        .await
}

/// RULE 1: every name form a UDT may take is deleted under its formatted name, reports
/// `DELETING` while the deletion is in flight, and is then gone from both readers.
#[tokio::test]
async fn delete_type_settles_every_name_form_into_absence() {
    /// One row: the `typeName` as submitted, and the formatted name the type is stored,
    /// deleted and answered under.
    struct Case {
        submitted: String,
        formatted: String,
    }

    // The rows span `CreateType$typeName`'s two documented forms and the modelled
    // `TypeName` length bounds (`len 1..48`): the unquoted form (lower case, mixed case,
    // digits and underscores, the minimum and the maximum length) and the double-quoted
    // form (upper casing preserved, a special character, an escaped double quote).
    let longest = "T".repeat(48);
    let cases: Vec<Case> = vec![
        Case {
            submitted: "address".to_string(),
            formatted: "address".to_string(),
        },
        Case {
            // "Amazon Keyspaces converts upper case characters automatically into lower
            // case characters."
            submitted: "Address_V2".to_string(),
            formatted: "address_v2".to_string(),
        },
        Case {
            // The minimum modelled length.
            submitted: "t".to_string(),
            formatted: "t".to_string(),
        },
        Case {
            // The maximum modelled length.
            submitted: longest.clone(),
            formatted: longest.to_ascii_lowercase(),
        },
        Case {
            // "When declaring a UDT name inside double quotes, Amazon Keyspaces
            // preserves upper casing and allows special characters."
            submitted: "\"MixedCase\"".to_string(),
            formatted: "\"MixedCase\"".to_string(),
        },
        Case {
            submitted: "\"with space!\"".to_string(),
            formatted: "\"with space!\"".to_string(),
        },
        Case {
            // An interior double quote, escaped with an additional one.
            submitted: "\"quo\"\"ted\"".to_string(),
            formatted: "\"quo\"\"ted\"".to_string(),
        },
    ];

    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "delete_type_names").await;
    for case in &cases {
        create_type(&emulator, &case.submitted, ks, &case.submitted).await;
    }
    // A type is deletable once the creation the clock owed has completed.
    tokio::time::sleep(settle_wait()).await;

    // `DELETING` is a member of the modelled `TypeStatus` value set, so the status a
    // deletion in flight reports is one the model declares rather than an invention.
    assert!(
        TYPE_STATUS_VALUES.contains(&keyspaces::TYPE_STATUS_DELETING),
        "case {:?}: the status a deletion in flight reports must be a modelled TypeStatus, \
         and the modelled set is {TYPE_STATUS_VALUES:?}",
        keyspaces::TYPE_STATUS_DELETING
    );

    for case in &cases {
        // The deletion is addressed by the SUBMITTED name and answers with the FORMATTED
        // one, beside the ARN of the keyspace the type was deleted from.
        let deleted = assert_ok(
            &case.submitted,
            emulator
                .call(
                    "DeleteType",
                    json!({"keyspaceName": ks, "typeName": case.submitted}),
                )
                .await,
        );
        assert_eq!(
            deleted["typeName"],
            json!(case.formatted),
            "case {:?}: expected the formatted type name {:?}, got {deleted}",
            case.submitted,
            case.formatted
        );
        assert_eq!(
            deleted["keyspaceArn"],
            json!(keyspace_arn(ks)),
            "case {:?}: expected the published keyspace ARN, got {deleted}",
            case.submitted
        );

        // The deletion is in flight, not instantaneous: the type is still there and
        // reports the transient status.
        let reported = assert_ok(
            &case.submitted,
            get_type(&emulator, ks, &case.submitted).await,
        );
        assert_eq!(
            reported["status"],
            json!(keyspaces::TYPE_STATUS_DELETING),
            "case {:?}: expected status {} while the deletion is in flight, got {reported}",
            case.submitted,
            keyspaces::TYPE_STATUS_DELETING
        );
    }

    tokio::time::sleep(settle_wait()).await;

    for case in &cases {
        let (status, body) = get_type(&emulator, ks, &case.submitted).await;
        assert_failure(
            &case.submitted,
            (status, body.clone()),
            404,
            "ResourceNotFoundException",
        );
        assert_eq!(
            body["resourceArn"],
            json!(type_arn(ks, &case.formatted)),
            "case {:?}: the exception must name the type that was looked for: {body}",
            case.submitted
        );
    }

    // The deleted types are gone from the listing too, so one copy of the keyspace's
    // types backs both readers.
    let listed = assert_ok(
        "the keyspace once every type is deleted",
        emulator
            .call("ListTypes", json!({"keyspaceName": ks}))
            .await,
    );
    assert_eq!(
        listed["types"],
        json!([]),
        "case {:?}: expected no types left in the keyspace, got {listed}",
        ks
    );
}

/// RULE 2: a type something still uses is refused and left alone, and the very same
/// request succeeds once the user is gone.
#[tokio::test]
async fn delete_type_refuses_a_type_a_table_or_another_type_uses() {
    /// What uses the type under test.
    enum User {
        /// A table one of whose columns has the given type expression.
        Table(&'static str),
        /// Another user-defined type one of whose fields has it.
        Type(&'static str),
    }

    /// One row: the type under test, what uses it, and the type expression through which
    /// the user reaches it.
    struct Case {
        case: &'static str,
        used: &'static str,
        user: User,
        reaches_it_by: &'static str,
    }

    // The rows differ in HOW the use reaches the type — a table column or another type's
    // field, directly, frozen, or inside a collection — because the rule is about the
    // type being used at all, not about the shape of the expression that uses it.
    let cases: Vec<Case> = vec![
        Case {
            case: "a table column of the type",
            used: "used_by_a_column",
            user: User::Table("uses_a_column"),
            reaches_it_by: "frozen<used_by_a_column>",
        },
        Case {
            case: "a table column holding the type in a collection",
            used: "used_in_a_collection",
            user: User::Table("uses_a_collection"),
            reaches_it_by: "map<text, frozen<used_in_a_collection>>",
        },
        Case {
            case: "another type's field of the type",
            used: "used_by_a_field",
            user: User::Type("uses_a_field"),
            reaches_it_by: "frozen<used_by_a_field>",
        },
        Case {
            case: "another type's field holding the type in a collection",
            used: "used_by_a_nested_field",
            user: User::Type("uses_a_nested_field"),
            reaches_it_by: "list<frozen<used_by_a_nested_field>>",
        },
    ];

    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "delete_type_in_use").await;

    for case in &cases {
        create_type(&emulator, case.case, ks, case.used).await;
    }
    // A type can only be referred to once it is ACTIVE.
    tokio::time::sleep(settle_wait()).await;

    for case in &cases {
        let created = match case.user {
            User::Table(table_name) => emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": ks,
                        "tableName": table_name,
                        "schemaDefinition": {
                            "allColumns": [
                                {"name": "pk", "type": "text"},
                                {"name": "value", "type": case.reaches_it_by},
                            ],
                            "partitionKeys": [{"name": "pk"}],
                        },
                    }),
                )
                .await,
            User::Type(type_name) => emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": ks,
                        "typeName": type_name,
                        "fieldDefinitions": [{"name": "value", "type": case.reaches_it_by}],
                    }),
                )
                .await,
        };
        assert_ok(case.case, created);
    }
    tokio::time::sleep(settle_wait()).await;

    for case in &cases {
        assert_failure(
            case.case,
            emulator
                .call(
                    "DeleteType",
                    json!({"keyspaceName": ks, "typeName": case.used}),
                )
                .await,
            409,
            "ConflictException",
        );
        // The refused delete wrote nothing: the type is still there and still usable.
        let reported = assert_ok(case.case, get_type(&emulator, ks, case.used).await);
        assert_eq!(
            reported["status"],
            json!(keyspaces::TYPE_STATUS_ACTIVE),
            "case {:?}: a refused delete must leave the type {}, got {reported}",
            case.case,
            keyspaces::TYPE_STATUS_ACTIVE
        );
    }

    // Deleting the user only STARTS its removal, so while that removal is in flight the
    // user still exists and still uses the type.
    for case in &cases {
        let requested = match case.user {
            User::Table(table_name) => emulator
                .call(
                    "DeleteTable",
                    json!({"keyspaceName": ks, "tableName": table_name}),
                )
                .await,
            User::Type(type_name) => emulator
                .call(
                    "DeleteType",
                    json!({"keyspaceName": ks, "typeName": type_name}),
                )
                .await,
        };
        assert_ok(case.case, requested);
        assert_failure(
            case.case,
            emulator
                .call(
                    "DeleteType",
                    json!({"keyspaceName": ks, "typeName": case.used}),
                )
                .await,
            409,
            "ConflictException",
        );
    }
    tokio::time::sleep(settle_wait()).await;

    // Nothing uses the types any more, so the very same request now succeeds: what
    // refused it was the use and nothing else.
    for case in &cases {
        let deleted = assert_ok(
            case.case,
            emulator
                .call(
                    "DeleteType",
                    json!({"keyspaceName": ks, "typeName": case.used}),
                )
                .await,
        );
        assert_eq!(
            deleted["typeName"],
            json!(case.used),
            "case {:?}: expected the deleted type's name, got {deleted}",
            case.case
        );
    }
}

/// RULE 3: a type with an action of its own in progress is refused, and that action is
/// not disturbed by the refusal.
#[tokio::test]
async fn delete_type_refuses_a_type_whose_own_action_is_in_progress() {
    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "delete_type_in_flight").await;

    // A creation in flight: the type is CREATING, so it is not deletable yet.
    create_type(&emulator, "a creation in flight", ks, "still_creating").await;
    assert_failure(
        "a creation in flight",
        emulator
            .call(
                "DeleteType",
                json!({"keyspaceName": ks, "typeName": "still_creating"}),
            )
            .await,
        409,
        "ConflictException",
    );
    let reported = assert_ok(
        "a creation in flight",
        get_type(&emulator, ks, "still_creating").await,
    );
    assert_eq!(
        reported["status"],
        json!(keyspaces::TYPE_STATUS_CREATING),
        "case {:?}: the refused delete must leave the creation in flight, got {reported}",
        "a creation in flight"
    );

    // Once that creation has completed the same request succeeds.
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "the creation has completed",
        emulator
            .call(
                "DeleteType",
                json!({"keyspaceName": ks, "typeName": "still_creating"}),
            )
            .await,
    );

    // A deletion in flight: the second call is refused rather than postponing the
    // removal the clock already owes.
    assert_failure(
        "a deletion in flight",
        emulator
            .call(
                "DeleteType",
                json!({"keyspaceName": ks, "typeName": "still_creating"}),
            )
            .await,
        409,
        "ConflictException",
    );
    tokio::time::sleep(settle_wait()).await;
    assert_failure(
        "the deletion has completed",
        get_type(&emulator, ks, "still_creating").await,
        404,
        "ResourceNotFoundException",
    );
    // And a type that is gone is gone: the delete that answered 409 while the removal was
    // in flight does not become a success later.
    assert_failure(
        "a type the deletion already removed",
        emulator
            .call(
                "DeleteType",
                json!({"keyspaceName": ks, "typeName": "still_creating"}),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );
}

/// RULE 4: a name that addresses no type of the caller's is refused with the error the
/// model declares for it, never with a 501 and never by deleting something else.
#[tokio::test]
async fn delete_type_refuses_a_name_that_addresses_no_type() {
    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "delete_type_refusals").await;
    create_type(&emulator, "the surviving type", ks, "survivor").await;
    tokio::time::sleep(settle_wait()).await;

    // An absent keyspace and an absent type are both "a keyspace, table, or type that
    // doesn't exist", and the exception names the resource that was looked for.
    for (case, request, expected_arn) in [
        (
            "an unknown keyspace",
            json!({"keyspaceName": "no_such_ks", "typeName": "survivor"}),
            keyspace_arn("no_such_ks"),
        ),
        (
            "an unknown type",
            json!({"keyspaceName": ks, "typeName": "never_created"}),
            type_arn(ks, "never_created"),
        ),
        (
            // The formatted name is what addresses a type, so a double-quoted name is
            // NOT the unquoted type of the same letters.
            "a quoted name for an unquoted type",
            json!({"keyspaceName": ks, "typeName": "\"survivor\""}),
            type_arn(ks, "\"survivor\""),
        ),
    ] {
        let (status, body) = emulator.call("DeleteType", request).await;
        assert_failure(case, (status, body.clone()), 404, "ResourceNotFoundException");
        assert_eq!(
            body["resourceArn"],
            json!(expected_arn),
            "case {case:?}: the exception must name the resource that was looked for: {body}"
        );
    }

    // A `typeName` that is neither form a UDT name may take names no type at all, and is
    // refused as invalid rather than as absent.
    for (case, name) in [
        ("a leading digit", "9name"),
        ("a leading underscore", "_name"),
        ("a space", "has space"),
        ("a hyphen", "dash-name"),
        ("a dot", "dot.name"),
        ("an unclosed quote", "\"unclosed"),
        ("an empty quoted name", "\"\""),
        ("an unescaped interior quote", "\"a\"b\""),
    ] {
        assert_failure(
            case,
            emulator
                .call("DeleteType", json!({"keyspaceName": ks, "typeName": name}))
                .await,
            400,
            "ValidationException",
        );
    }

    // One past each published `TypeName` bound (`len 1..48`) is refused by the boundary
    // validator the model drives.
    for (case, name) in [
        ("under the minimum length", String::new()),
        ("one past the maximum length", "t".repeat(49)),
    ] {
        assert_failure(
            case,
            emulator
                .call("DeleteType", json!({"keyspaceName": ks, "typeName": name}))
                .await,
            400,
            "ValidationException",
        );
    }

    // A service-managed keyspace holds none of the caller's types and is not DDL-able:
    // every one of them is refused the same way, and none of them is a 404.
    for (keyspace_name, _tables) in keyspaces::SERVICE_MANAGED_KEYSPACES {
        assert_failure(
            keyspace_name,
            emulator
                .call(
                    "DeleteType",
                    json!({"keyspaceName": keyspace_name, "typeName": "any_type"}),
                )
                .await,
            400,
            "ValidationException",
        );
    }

    // None of the refusals above touched the type that does exist.
    let reported = assert_ok("the surviving type", get_type(&emulator, ks, "survivor").await);
    assert_eq!(
        reported["status"],
        json!(keyspaces::TYPE_STATUS_ACTIVE),
        "case {:?}: the refused deletes must leave the existing type alone, got {reported}",
        "the surviving type"
    );
}

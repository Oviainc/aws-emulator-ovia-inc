// SPDX-License-Identifier: CC0-1.0

//! The Amazon Keyspaces user-defined type (UDT) lifecycle, driven end to end over the
//! service's own `awsJson1_0` wire protocol through the shared
//! [`crate::keyspace_table_lifecycle::Emulator`] harness.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. `CreateType` stores the type under its FORMATTED name and answers with that name
//!    and the ARN of the keyspace that holds it: an unquoted name is folded to lower
//!    case, a double-quoted one keeps its casing and special characters, and a name that
//!    is neither form is refused with the modelled `ValidationException`.
//! 2. A type reads back through `GetType` exactly as it was created — the field
//!    definitions verbatim, in order.
//! 3. `CREATING` is a transient status the clock settles into `ACTIVE`.
//! 4. `GetType`'s `directParentTypes`, `directReferringTables` and `maxNestingDepth` are
//!    DERIVED from the keyspace: they answer for whatever types and tables refer to this
//!    type at the instant of the read.
//! 5. A field type may refer only to a type of the same keyspace that is usable, so a
//!    reference to an absent or not-yet-`ACTIVE` type is the modelled
//!    `ResourceNotFoundException` and stores nothing.

use std::time::SystemTime;

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

/// Create a keyspace and hand back its name.
async fn keyspace(emulator: &Emulator, name: &'static str) -> &'static str {
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": name}))
            .await,
    );
    name
}

/// RULES 1, 2 and 3: every name form a UDT may take is stored under its formatted name,
/// reads back with its fields verbatim, and settles from `CREATING` into `ACTIVE`.
#[tokio::test]
async fn create_type_formats_the_name_and_round_trips_through_get_type() {
    /// One row: the `typeName` as submitted, the formatted name it must be stored and
    /// answered under, and the fields it is created with.
    struct Case {
        submitted: String,
        formatted: String,
        fields: Value,
    }

    // The rows span `CreateType$typeName`'s two documented forms and the modelled
    // `TypeName` length bounds (`len 1..48`): the unquoted form (lower-cased, mixed
    // case, digits, underscores, the minimum and the maximum length) and the
    // double-quoted form (upper casing preserved, a special character, and a double
    // quote escaped with an additional double quote).
    let longest = "T".repeat(48);
    let cases: Vec<Case> = vec![
        Case {
            submitted: "address".to_string(),
            formatted: "address".to_string(),
            fields: json!([{"name": "street", "type": "text"}]),
        },
        Case {
            // "Amazon Keyspaces converts upper case characters automatically into
            // lower case characters."
            submitted: "Address_V2".to_string(),
            formatted: "address_v2".to_string(),
            fields: json!([
                {"name": "street", "type": "text"},
                {"name": "number", "type": "int"},
                {"name": "tags", "type": "map<text, text>"},
                // A parameterised type whose argument is a NUMBER rather than a type:
                // the `3` names no user-defined type, so this field refers to none.
                {"name": "embedding", "type": "vector<float, 3>"},
            ]),
        },
        Case {
            // The minimum modelled length, and the one-character identifier.
            submitted: "t".to_string(),
            formatted: "t".to_string(),
            fields: json!([{"name": "value", "type": "blob"}]),
        },
        Case {
            // The maximum modelled length, upper case throughout.
            submitted: longest.clone(),
            formatted: longest.to_ascii_lowercase(),
            fields: json!([{"name": "value", "type": "uuid"}]),
        },
        Case {
            // "When declaring a UDT name inside double quotes, Amazon Keyspaces
            // preserves upper casing and allows special characters."
            submitted: "\"MixedCase\"".to_string(),
            formatted: "\"MixedCase\"".to_string(),
            fields: json!([{"name": "Field", "type": "timeuuid"}]),
        },
        Case {
            submitted: "\"with space!\"".to_string(),
            formatted: "\"with space!\"".to_string(),
            fields: json!([{"name": "value", "type": "decimal"}]),
        },
        Case {
            // "you must escape each double quote character with an additional double
            // quote character."
            submitted: "\"quo\"\"ted\"".to_string(),
            formatted: "\"quo\"\"ted\"".to_string(),
            fields: json!([{"name": "value", "type": "varint"}]),
        },
    ];

    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "type_round_trip_ks").await;

    let before_create = SystemTime::now();
    for case in &cases {
        let created = assert_ok(
            &case.submitted,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": ks,
                        "typeName": case.submitted,
                        "fieldDefinitions": case.fields,
                    }),
                )
                .await,
        );
        assert_eq!(
            (&created["typeName"], &created["keyspaceArn"]),
            (&json!(case.formatted), &json!(keyspace_arn(ks))),
            "case {:?}: CreateType must answer the formatted name and the keyspace ARN, \
             got {created}",
            case.submitted
        );

        // RULE 2: the formatted name reads back the type that was created.
        let got = assert_ok(
            &case.formatted,
            emulator
                .call(
                    "GetType",
                    json!({"keyspaceName": ks, "typeName": case.formatted}),
                )
                .await,
        );
        assert_eq!(
            (
                &got["keyspaceName"],
                &got["typeName"],
                &got["keyspaceArn"],
                &got["fieldDefinitions"]
            ),
            (
                &json!(ks),
                &json!(case.formatted),
                &json!(keyspace_arn(ks)),
                &case.fields
            ),
            "case {:?}: GetType must report the type CreateType made, fields verbatim: \
             {got}",
            case.submitted
        );
        // A flat type nests nothing, and nothing refers to it yet.
        assert_eq!(
            (
                &got["maxNestingDepth"],
                &got["directParentTypes"],
                &got["directReferringTables"]
            ),
            (&json!(0), &json!([]), &json!([])),
            "case {:?}: a flat type nests nothing and has no referrers: {got}",
            case.submitted
        );

        // RULE 3: the status is a modelled `TypeStatus`, and it is the transient one.
        let status = got["status"].as_str().unwrap_or_default().to_string();
        assert!(
            TYPE_STATUS_VALUES.contains(&status.as_str()),
            "case {:?}: status {status:?} is not a modelled TypeStatus value",
            case.submitted
        );
        assert_eq!(
            status,
            keyspaces::TYPE_STATUS_CREATING,
            "case {:?}: a type whose creation was just accepted is CREATING: {got}",
            case.submitted
        );

        // `lastModifiedTimestamp` is when the definition was written, as epoch seconds
        // on this protocol.
        let modified_at = got["lastModifiedTimestamp"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {:?}: lastModifiedTimestamp must be a timestamp: {got}",
                case.submitted
            )
        });
        let floor = before_create
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_secs_f64();
        let ceiling = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_secs_f64();
        assert!(
            modified_at >= floor - 1.0 && modified_at <= ceiling + 1.0,
            "case {:?}: lastModifiedTimestamp {modified_at} is outside the window \
             [{floor}, {ceiling}] in which CreateType ran",
            case.submitted
        );

        // The same name again is the modelled conflict, not a silent overwrite.
        assert_failure(
            &case.submitted,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": ks,
                        "typeName": case.submitted,
                        "fieldDefinitions": json!([{"name": "other", "type": "int"}]),
                    }),
                )
                .await,
            409,
            "ConflictException",
        );
    }

    // An unquoted name is case-insensitive, so the spelling that was submitted finds the
    // same type as the formatted one: the store holds ONE copy.
    for case in &cases {
        if case.submitted.starts_with('"') {
            continue;
        }
        let got = assert_ok(
            &case.submitted,
            emulator
                .call(
                    "GetType",
                    json!({"keyspaceName": ks, "typeName": case.submitted}),
                )
                .await,
        );
        assert_eq!(
            got["typeName"],
            json!(case.formatted),
            "case {:?}: an unquoted name must resolve to its formatted form: {got}",
            case.submitted
        );
    }

    // RULE 3: the transition the clock owed has become due, so every type is now ACTIVE
    // — and the fields it reads back are still the ones that were submitted.
    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        let got = assert_ok(
            &case.formatted,
            emulator
                .call(
                    "GetType",
                    json!({"keyspaceName": ks, "typeName": case.formatted}),
                )
                .await,
        );
        assert_eq!(
            (&got["status"], &got["fieldDefinitions"]),
            (&json!(keyspaces::TYPE_STATUS_ACTIVE), &case.fields),
            "case {:?}: the creation span has elapsed, so the type is ACTIVE and its \
             fields are untouched: {got}",
            case.formatted
        );
    }
}

/// A `typeName` that is neither documented form is refused with the modelled
/// `ValidationException`, and stores nothing.
#[tokio::test]
async fn a_type_name_in_neither_documented_form_is_a_modelled_validation_failure() {
    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "bad_name_ks").await;

    // Each row breaks one half of `CreateType$typeName`'s rule: the unquoted form must
    // begin with an alphabetic character and hold only alphanumerics and underscores;
    // the quoted form must be closed and must double every interior quote.
    let illegal: [(&str, &str); 8] = [
        ("a leading underscore", "_leading"),
        ("a leading digit", "9lead"),
        ("a space", "has space"),
        ("a hyphen", "dash-name"),
        ("a dot", "dot.name"),
        ("an unclosed quote", "\"unclosed"),
        ("an empty quoted name", "\"\""),
        ("an unescaped interior quote", "\"a\"b\""),
    ];
    for (case, name) in illegal {
        assert_failure(
            case,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": ks,
                        "typeName": name,
                        "fieldDefinitions": [{"name": "value", "type": "int"}],
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
        // GetType is refused for the same reason, so a malformed name never names a type.
        assert_failure(
            case,
            emulator
                .call("GetType", json!({"keyspaceName": ks, "typeName": name}))
                .await,
            400,
            "ValidationException",
        );
    }

    // One past the modelled `TypeName` length bounds is refused by the boundary
    // validator, with the status and code the model declares.
    for (case, name) in [
        ("under the minimum length", String::new()),
        ("one past the maximum length", "t".repeat(49)),
    ] {
        assert_failure(
            case,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": ks,
                        "typeName": name,
                        "fieldDefinitions": [{"name": "value", "type": "int"}],
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
    }

    // A type holds each field name once, under the same identifier rule the type name
    // follows: `Id` and `id` are one field name, so the pair is refused.
    for (case, fields) in [
        (
            "a field declared twice",
            json!([
                {"name": "value", "type": "int"},
                {"name": "value", "type": "text"},
            ]),
        ),
        (
            "a field declared twice in different casing",
            json!([
                {"name": "Id", "type": "int"},
                {"name": "id", "type": "text"},
            ]),
        ),
    ] {
        assert_failure(
            case,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": ks,
                        "typeName": "twice",
                        "fieldDefinitions": fields,
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
    }
    // A quoted name is its own identifier, so the same spelling in quotes is a second
    // field and the type is accepted.
    assert_ok(
        "a quoted field name beside its unquoted spelling",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": ks,
                    "typeName": "quoted_field_names",
                    "fieldDefinitions": [
                        {"name": "id", "type": "int"},
                        {"name": "\"Id\"", "type": "text"},
                    ],
                }),
            )
            .await,
    );
    assert_failure(
        "the refused type was not stored",
        emulator
            .call("GetType", json!({"keyspaceName": ks, "typeName": "twice"}))
            .await,
        404,
        "ResourceNotFoundException",
    );

    // `fieldDefinitions` is modelled `len 1..` with no maximum, so the emulator's own
    // cap is what refuses a field list past it — with the quota error the model declares,
    // and nothing stored.
    let one_too_many: Vec<Value> = (0..=keyspaces::MAX_FIELDS_PER_TYPE)
        .map(|index| json!({"name": format!("f{index}"), "type": "int"}))
        .collect();
    assert_failure(
        "one field past the per-type cap",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": ks,
                    "typeName": "too_many_fields",
                    "fieldDefinitions": one_too_many,
                }),
            )
            .await,
        402,
        "ServiceQuotaExceededException",
    );
    assert_failure(
        "the type refused at the field cap was not stored",
        emulator
            .call(
                "GetType",
                json!({"keyspaceName": ks, "typeName": "too_many_fields"}),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );
    // Exactly at the cap is accepted, so the bound is the published one and not one off.
    let at_the_cap: Vec<Value> = (0..keyspaces::MAX_FIELDS_PER_TYPE)
        .map(|index| json!({"name": format!("f{index}"), "type": "int"}))
        .collect();
    assert_ok(
        "exactly the per-type field cap",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": ks,
                    "typeName": "at_the_field_cap",
                    "fieldDefinitions": at_the_cap,
                }),
            )
            .await,
    );
}

/// RULE 4: nesting depth, parent types and referring tables are derived from the
/// keyspace, over a set of field types that reach a UDT in different ways.
#[tokio::test]
async fn get_type_derives_nesting_depth_parents_and_referring_tables() {
    /// One row: the type to create, the field types it is created with, the nesting
    /// depth that must follow from them, and the types that must report as its parents
    /// (stated as literals, in the total order the store keeps its types in, so the
    /// expectation does not depend on the implementation that derives it).
    struct Case {
        name: &'static str,
        field_types: &'static [&'static str],
        depth: i64,
        parents: &'static [&'static str],
    }

    // The rows differ in HOW a field reaches another type — directly, frozen, inside a
    // list, as a map value, inside a tuple, the same type twice, and through two
    // different parents (a diamond, where a memoised walk must still count each level
    // once) — and in how deep the chain is.
    let cases: [Case; 7] = [
        Case {
            name: "leaf",
            field_types: &["text", "int"],
            depth: 0,
            parents: &[
                "refers_to_one_type_twice",
                "wraps_in_a_collection",
                "wraps_leaf_directly",
                "wraps_leaf_frozen",
            ],
        },
        Case {
            name: "wraps_leaf_directly",
            field_types: &["leaf"],
            depth: 1,
            parents: &["diamond"],
        },
        Case {
            name: "wraps_leaf_frozen",
            field_types: &["frozen<leaf>", "text"],
            depth: 1,
            parents: &["diamond", "wraps_in_a_collection"],
        },
        Case {
            name: "wraps_in_a_collection",
            field_types: &["list<frozen<wraps_leaf_frozen>>", "map<text, frozen<leaf>>"],
            depth: 2,
            parents: &["wraps_in_a_tuple"],
        },
        Case {
            name: "wraps_in_a_tuple",
            field_types: &["tuple<int, frozen<wraps_in_a_collection>>"],
            depth: 3,
            parents: &[],
        },
        Case {
            name: "refers_to_one_type_twice",
            field_types: &["map<frozen<leaf>, frozen<leaf>>"],
            depth: 1,
            parents: &[],
        },
        Case {
            name: "diamond",
            field_types: &["frozen<wraps_leaf_directly>", "frozen<wraps_leaf_frozen>"],
            depth: 2,
            parents: &[],
        },
    ];

    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "nesting_ks").await;

    for case in &cases {
        let fields: Vec<Value> = case
            .field_types
            .iter()
            .enumerate()
            .map(|(index, field_type)| json!({"name": format!("f{index}"), "type": field_type}))
            .collect();
        assert_ok(
            case.name,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": ks,
                        "typeName": case.name,
                        "fieldDefinitions": fields,
                    }),
                )
                .await,
        );
        // A type is usable once it is ACTIVE, which is what the next row refers to.
        tokio::time::sleep(settle_wait()).await;
    }

    // A table whose column uses one of the types: `leaf` gains a referring table,
    // nothing else does.
    assert_ok(
        "create the referring table",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": ks,
                    "tableName": "uses_leaf",
                    "schemaDefinition": {
                        "allColumns": [
                            {"name": "pk", "type": "text"},
                            {"name": "address", "type": "frozen<leaf>"},
                        ],
                        "partitionKeys": [{"name": "pk"}],
                    },
                }),
            )
            .await,
    );

    for case in &cases {
        let got = assert_ok(
            case.name,
            emulator
                .call(
                    "GetType",
                    json!({"keyspaceName": ks, "typeName": case.name}),
                )
                .await,
        );
        assert_eq!(
            got["maxNestingDepth"],
            json!(case.depth),
            "case {:?}: expected nesting depth {}, got {got}",
            case.name,
            case.depth
        );

        // The parents of a type are exactly the types whose field types reach it, in the
        // one total order the store keeps them in.
        assert_eq!(
            got["directParentTypes"],
            json!(case.parents),
            "case {:?}: expected the parents {:?}, got {got}",
            case.name,
            case.parents
        );

        let expected_tables = if case.name == "leaf" {
            json!(["uses_leaf"])
        } else {
            json!([])
        };
        assert_eq!(
            got["directReferringTables"], expected_tables,
            "case {:?}: expected the referring tables {expected_tables}, got {got}",
            case.name
        );
    }
}

/// RULE 5, and the other modelled refusals: a reference to a type that is not usable,
/// an unknown keyspace, and an unknown type.
#[tokio::test]
async fn referring_to_a_type_that_is_not_usable_is_a_modelled_not_found() {
    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "reference_ks").await;

    // Every row names a type that is not usable at the moment of the call, reached the
    // way a field type reaches one. `still_creating` exists but has not settled into
    // ACTIVE, so it is a type whose "status might not be ACTIVE".
    assert_ok(
        "create the type that is still CREATING",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": ks,
                    "typeName": "still_creating",
                    "fieldDefinitions": [{"name": "value", "type": "int"}],
                }),
            )
            .await,
    );

    let cases: [(&str, &str, &str); 5] = [
        ("an absent type", "no_such_type", "no_such_type"),
        ("an absent type, frozen", "frozen<absent_frozen>", "absent_frozen"),
        (
            "an absent type inside a collection",
            "map<text, frozen<absent_in_map>>",
            "absent_in_map",
        ),
        (
            "an absent type, quoted",
            "frozen<\"Absent Quoted\">",
            "\"Absent Quoted\"",
        ),
        (
            "a type that is still CREATING",
            "frozen<still_creating>",
            "still_creating",
        ),
    ];

    for (case, field_type, missing) in cases {
        let (status, body) = emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": ks,
                    "typeName": "refers_to_the_unusable",
                    "fieldDefinitions": [{"name": "value", "type": field_type}],
                }),
            )
            .await;
        assert_failure(case, (status, body.clone()), 404, "ResourceNotFoundException");
        assert_eq!(
            body["resourceArn"],
            json!(type_arn(ks, missing)),
            "case {case:?}: the exception must name the type that was looked for: {body}"
        );
        // The message is built from the request: the Cassandra-style form Amazon
        // Keyspaces answers an unresolvable UDT name with.
        assert_eq!(
            body["message"],
            json!(format!("UDT {missing} does not exist")),
            "case {case:?}: the message must name the unresolved type: {body}"
        );
        // Nothing was stored: the refused create is not half-applied.
        assert_failure(
            case,
            emulator
                .call(
                    "GetType",
                    json!({"keyspaceName": ks, "typeName": "refers_to_the_unusable"}),
                )
                .await,
            404,
            "ResourceNotFoundException",
        );
    }

    // Once the referenced type is ACTIVE the very same request succeeds, so what refused
    // it was the status and nothing else.
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "the reference once the type is ACTIVE",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": ks,
                    "typeName": "refers_to_the_unusable",
                    "fieldDefinitions": [{"name": "value", "type": "frozen<still_creating>"}],
                }),
            )
            .await,
    );

    // An unknown keyspace, and an unknown type in a known keyspace.
    for (case, operation, request, expected_arn) in [
        (
            "CreateType in an unknown keyspace",
            "CreateType",
            json!({
                "keyspaceName": "no_such_ks",
                "typeName": "orphan",
                "fieldDefinitions": [{"name": "value", "type": "int"}],
            }),
            keyspace_arn("no_such_ks"),
        ),
        (
            "GetType in an unknown keyspace",
            "GetType",
            json!({"keyspaceName": "no_such_ks", "typeName": "orphan"}),
            keyspace_arn("no_such_ks"),
        ),
        (
            "GetType of an unknown type",
            "GetType",
            json!({"keyspaceName": ks, "typeName": "never_created"}),
            type_arn(ks, "never_created"),
        ),
    ] {
        let (status, body) = emulator.call(operation, request).await;
        assert_failure(case, (status, body.clone()), 404, "ResourceNotFoundException");
        assert_eq!(
            body["resourceArn"],
            json!(expected_arn),
            "case {case:?}: the exception must name the resource that was looked for: {body}"
        );
    }
}

/// The store is bounded: past the emulator's own cap `CreateType` answers the quota error
/// the model declares, rather than accepting another process-lifetime entry.
#[tokio::test]
async fn creating_types_past_the_store_cap_answers_the_modelled_quota_error() {
    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "capped_type_ks").await;

    for index in 0..keyspaces::MAX_TYPES_PER_KEYSPACE {
        assert_ok(
            "fill the keyspace to its type cap",
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": ks,
                        "typeName": format!("t{index}"),
                        "fieldDefinitions": [{"name": "value", "type": "int"}],
                    }),
                )
                .await,
        );
    }
    assert_failure(
        "one type past the cap",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": ks,
                    "typeName": "one_too_many",
                    "fieldDefinitions": [{"name": "value", "type": "int"}],
                }),
            )
            .await,
        402,
        "ServiceQuotaExceededException",
    );
    assert_failure(
        "the type refused at the cap was not stored",
        emulator
            .call(
                "GetType",
                json!({"keyspaceName": ks, "typeName": "one_too_many"}),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );
    // A full keyspace does not change what a name already taken answers.
    assert_failure(
        "a type name already taken, with the keyspace full",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": ks,
                    "typeName": "t0",
                    "fieldDefinitions": [{"name": "value", "type": "int"}],
                }),
            )
            .await,
        409,
        "ConflictException",
    );
}

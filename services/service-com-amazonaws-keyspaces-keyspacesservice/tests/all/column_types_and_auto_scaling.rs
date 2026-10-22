// SPDX-License-Identifier: CC0-1.0

//! `CreateTable`'s column-type resolution, its auto scaling settings, and the form of a
//! table not-found — all driven end to end over the service's own `awsJson1_0` wire
//! protocol through the shared [`crate::keyspace_table_lifecycle::Emulator`] harness, so
//! what is pinned is the emulator's observable behaviour and a create is only believed
//! once a read reports it back.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. A column's type may refer only to a user-defined type of the same keyspace that is
//!    usable, exactly as a UDT field's type may: every identifier in it that names neither
//!    a CQL native type nor a type constructor must be a type of that keyspace in status
//!    `ACTIVE`, and a reference to an absent or not-yet-`ACTIVE` type is the modelled
//!    `ResourceNotFoundException` (404) naming it, with no table stored.
//! 2. `autoScalingSpecification` is a modelled, optional input of a table in provisioned
//!    capacity mode: it is stored as given and reported back by
//!    `GetTableAutoScalingSettings`, and a request that breaks one of the documented
//!    rules for it is the modelled `ValidationException` — never a 501.
//! 3. A table that is not in state is the modelled `ResourceNotFoundException` whose
//!    message is built from the request as `unconfigured table <tableName>`: the bare
//!    table name, with no keyspace qualifier and no quoting. A keyspace lookup keeps its
//!    own form.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, keyspace_arn, minimal_schema, settle_wait, table_arn, Emulator,
};

/// A schema whose partition key is a `text` column and whose one regular column declares
/// `column_type`.
fn schema_with_type(column_type: &str) -> Value {
    json!({
        "allColumns": [
            {"name": "pk", "type": "text"},
            {"name": "val", "type": column_type},
        ],
        "partitionKeys": [{"name": "pk"}],
    })
}

/// Compare one member of a response body, by JSON pointer, tolerating the integer and the
/// double spelling of the same number.
fn assert_member(case: &str, body: &Value, pointer: &str, expected: &Value) {
    let actual = body.pointer(pointer).unwrap_or(&Value::Null);
    let equal = match (actual.as_f64(), expected.as_f64()) {
        (Some(actual), Some(expected)) => (actual - expected).abs() < 1e-9,
        _ => actual == expected,
    };
    assert!(
        equal,
        "case {case:?}: {pointer} expected {expected}, got {actual} in {body}"
    );
}

/// RULE 1: a column type built only from the CQL native types and type constructors
/// resolves and the table is stored, while one whose identifiers include a name the
/// keyspace does not define is the modelled not-found, naming that type.
///
/// The accepted rows are the service's own published catalog, read from the constants the
/// resolution is built on (`keyspaces::CQL_NATIVE_TYPES` / `keyspaces::CQL_TYPE_CONSTRUCTORS`,
/// whose source is `ColumnDefinition$type` / `FieldDefinition$type` -> Cassandra data type
/// support in the Amazon Keyspaces Developer Guide), so no entry is spelled twice here;
/// the constructors are then exercised at each of their arities, nested, and in upper
/// case — an unquoted CQL identifier is case-insensitive.
#[tokio::test]
async fn create_table_resolves_a_native_column_type_and_refuses_an_unresolvable_one() {
    let emulator = Emulator::start();
    let keyspace = "column_types_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    // The declared type, and the type name the refusal must report, or `None` when the
    // type resolves and the table is created.
    let mut rows: Vec<(String, Option<String>)> = Vec::new();
    // Every native type in the published catalog, including the entries this issue never
    // cited.
    for native in keyspaces::CQL_NATIVE_TYPES {
        rows.push(((*native).to_string(), None));
    }
    // Each type constructor at its own arity, nested, and spelled in upper case.
    for accepted in [
        "list<text>",
        "set<int>",
        "map<text, int>",
        "tuple<int, text, uuid>",
        "frozen<list<text>>",
        "frozen<map<text, frozen<set<uuid>>>>",
        "vector<float, 3>",
        "MAP<TEXT,INT>",
        "Text",
    ] {
        rows.push((accepted.to_string(), None));
    }
    // An identifier that names neither a native type nor a constructor can only name a
    // user-defined type, and this keyspace defines none — bare, nested, and in each
    // position of a parameterised type. An unquoted name is reported case-folded, the
    // form every other operation takes it in.
    for (declared, unresolved) in [
        ("not_a_real_cql_type", "not_a_real_cql_type"),
        ("varcharr", "varcharr"),
        ("Address", "address"),
        ("list<address>", "address"),
        ("map<text, nested_udt>", "nested_udt"),
        ("map<missing_key, text>", "missing_key"),
        ("frozen<address>", "address"),
        ("tuple<int, missing_type>", "missing_type"),
    ] {
        rows.push((declared.to_string(), Some(unresolved.to_string())));
    }

    for (index, (declared, unresolved)) in rows.iter().enumerate() {
        let table = format!("ct_{index}");
        let case = format!("{declared:?} as {table}");
        let answer = emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": schema_with_type(declared),
                }),
            )
            .await;
        let read_back = json!({"keyspaceName": keyspace, "tableName": table});
        match unresolved {
            None => {
                let created = assert_ok(&case, answer);
                assert_member(
                    &case,
                    &created,
                    "/resourceArn",
                    &json!(table_arn(keyspace, &table)),
                );
                // A create is only believed once the reader reports it back, schema
                // verbatim.
                let got = assert_ok(&case, emulator.call("GetTable", read_back).await);
                assert_eq!(
                    got["schemaDefinition"],
                    schema_with_type(declared),
                    "case {case:?}: the stored schema must read back verbatim, got {got}"
                );
            }
            Some(unresolved) => {
                let (status, body) = answer;
                assert_failure(&case, (status, body.clone()), 404, "ResourceNotFoundException");
                assert_member(
                    &case,
                    &body,
                    "/message",
                    &json!(format!("UDT {unresolved} does not exist")),
                );
                // Refused, so nothing was stored under that name.
                assert_failure(
                    &case,
                    emulator.call("GetTable", read_back).await,
                    404,
                    "ResourceNotFoundException",
                );
            }
        }
    }
}

/// RULE 1, the resolving half: a column type that refers to a type of its own keyspace is
/// accepted once that type is usable, is refused while the type is still `CREATING`, and
/// is refused in a keyspace that does not define it. The table that refers to it is what
/// `GetType` then reports as a referring table.
#[tokio::test]
async fn a_column_type_resolves_against_the_user_defined_types_of_its_own_keyspace() {
    let emulator = Emulator::start();
    let defining = "udt_column_ks";
    let other = "no_udt_column_ks";
    for keyspace in [defining, other] {
        assert_ok(
            keyspace,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
                .await,
        );
    }
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
    // "its status might not be ACTIVE" is the same not-found.
    let (status, body) = emulator
        .call(
            "CreateTable",
            json!({
                "keyspaceName": defining,
                "tableName": "too_early_tbl",
                "schemaDefinition": schema_with_type("frozen<address>"),
            }),
        )
        .await;
    assert_failure(
        "a type that is still CREATING",
        (status, body.clone()),
        404,
        "ResourceNotFoundException",
    );
    assert_member(
        "a type that is still CREATING",
        &body,
        "/message",
        &json!("UDT address does not exist"),
    );

    // Once the clock has settled the type into ACTIVE, every form that refers to it
    // resolves.
    tokio::time::sleep(settle_wait()).await;
    let rows: [(&str, &str); 6] = [
        ("address", "address"),
        ("Address", "address"),
        ("frozen<address>", "address"),
        ("list<address>", "address"),
        ("map<text, frozen<address>>", "address"),
        ("tuple<address, int>", "address"),
    ];
    for (index, (declared, unresolved)) in rows.iter().enumerate() {
        let table = format!("udt_{index}");
        let case = format!("{declared:?} as {table}");
        let created = assert_ok(
            &case,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": defining,
                        "tableName": table,
                        "schemaDefinition": schema_with_type(declared),
                    }),
                )
                .await,
        );
        assert_member(
            &case,
            &created,
            "/resourceArn",
            &json!(table_arn(defining, &table)),
        );
        let got = assert_ok(
            &case,
            emulator
                .call(
                    "GetTable",
                    json!({"keyspaceName": defining, "tableName": table}),
                )
                .await,
        );
        assert_eq!(
            got["schemaDefinition"],
            schema_with_type(declared),
            "case {case:?}: the stored schema must read back verbatim, got {got}"
        );

        // The same type name in a keyspace that defines no type: unresolved.
        let (status, body) = emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": other,
                    "tableName": table,
                    "schemaDefinition": schema_with_type(declared),
                }),
            )
            .await;
        assert_failure(&case, (status, body.clone()), 404, "ResourceNotFoundException");
        assert_member(
            &case,
            &body,
            "/message",
            &json!(format!("UDT {unresolved} does not exist")),
        );
    }

    // Every table that was accepted refers to the type, which is what `GetType` derives
    // its `directReferringTables` from.
    let got = assert_ok(
        "the type's referring tables",
        emulator
            .call(
                "GetType",
                json!({"keyspaceName": defining, "typeName": "address"}),
            )
            .await,
    );
    let referring = got["directReferringTables"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for index in 0..rows.len() {
        let table = json!(format!("udt_{index}"));
        assert!(
            referring.contains(&table),
            "case {table}: a table whose column refers to the type must be reported as a \
             referring table, got {got}"
        );
    }
}

/// RULE 2: `autoScalingSpecification` is accepted on a provisioned table, stored, and
/// reported back by `GetTableAutoScalingSettings`.
///
/// The rows differ along which capacity is configured, whether auto scaling is turned on,
/// the unit bounds, and the target value — including each bound of the published
/// `targetValue` range (20..=90).
#[tokio::test]
async fn auto_scaling_settings_are_stored_by_create_table_and_reported_back() {
    /// One row: the `autoScalingSpecification` to create the table with, and the members
    /// `GetTableAutoScalingSettings` must report, by JSON pointer under
    /// `autoScalingSpecification`.
    struct Case {
        name: &'static str,
        specification: Value,
        checks: Vec<(&'static str, Value)>,
    }

    let enabled = |minimum: i64, maximum: i64, target: f64| {
        json!({
            "autoScalingDisabled": false,
            "minimumUnits": minimum,
            "maximumUnits": maximum,
            "scalingPolicy": {
                "targetTrackingScalingPolicyConfiguration": {"targetValue": target},
            },
        })
    };

    let cases: Vec<Case> = vec![
        Case {
            name: "read_and_write_both_enabled",
            specification: json!({
                "readCapacityAutoScaling": enabled(5, 50, 60.0),
                "writeCapacityAutoScaling": enabled(5, 50, 60.0),
            }),
            checks: vec![
                ("/readCapacityAutoScaling/minimumUnits", json!(5)),
                ("/readCapacityAutoScaling/maximumUnits", json!(50)),
                ("/readCapacityAutoScaling/autoScalingDisabled", json!(false)),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(60.0),
                ),
                ("/writeCapacityAutoScaling/minimumUnits", json!(5)),
                ("/writeCapacityAutoScaling/maximumUnits", json!(50)),
                (
                    "/writeCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(60.0),
                ),
            ],
        },
        Case {
            name: "read_only",
            specification: json!({"readCapacityAutoScaling": enabled(1, 40000, 20.0)}),
            checks: vec![
                ("/readCapacityAutoScaling/minimumUnits", json!(1)),
                ("/readCapacityAutoScaling/maximumUnits", json!(40000)),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(20.0),
                ),
                ("/writeCapacityAutoScaling", Value::Null),
            ],
        },
        Case {
            name: "write_only_at_the_top_of_the_target_range",
            specification: json!({"writeCapacityAutoScaling": enabled(7, 7, 90.0)}),
            checks: vec![
                ("/writeCapacityAutoScaling/minimumUnits", json!(7)),
                ("/writeCapacityAutoScaling/maximumUnits", json!(7)),
                (
                    "/writeCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(90.0),
                ),
                ("/readCapacityAutoScaling", Value::Null),
            ],
        },
        Case {
            // "This optional parameter enables auto scaling for the table if set to
            // false": turned off, so it needs no bounds and no policy.
            name: "explicitly_disabled",
            specification: json!({
                "readCapacityAutoScaling": {"autoScalingDisabled": true},
                "writeCapacityAutoScaling": {"autoScalingDisabled": true},
            }),
            checks: vec![
                ("/readCapacityAutoScaling/autoScalingDisabled", json!(true)),
                ("/writeCapacityAutoScaling/autoScalingDisabled", json!(true)),
            ],
        },
        Case {
            name: "scale_in_and_cooldowns_supplied",
            specification: json!({
                "readCapacityAutoScaling": {
                    "autoScalingDisabled": false,
                    "minimumUnits": 10,
                    "maximumUnits": 100,
                    "scalingPolicy": {
                        "targetTrackingScalingPolicyConfiguration": {
                            "targetValue": 45.5,
                            "disableScaleIn": true,
                            "scaleInCooldown": 120,
                            "scaleOutCooldown": 60,
                        },
                    },
                },
            }),
            checks: vec![
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(45.5),
                ),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/disableScaleIn",
                    json!(true),
                ),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/scaleInCooldown",
                    json!(120),
                ),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/scaleOutCooldown",
                    json!(60),
                ),
            ],
        },
    ];

    let emulator = Emulator::start();
    let keyspace = "auto_scaling_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    for case in &cases {
        let table = case.name;
        assert_ok(
            case.name,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "schemaDefinition": minimal_schema("pk"),
                        "capacitySpecification": {
                            "throughputMode": "PROVISIONED",
                            "readCapacityUnits": 5,
                            "writeCapacityUnits": 5,
                        },
                        "autoScalingSpecification": case.specification,
                    }),
                )
                .await,
        );

        let got = assert_ok(
            case.name,
            emulator
                .call(
                    "GetTableAutoScalingSettings",
                    json!({"keyspaceName": keyspace, "tableName": table}),
                )
                .await,
        );
        assert_member(case.name, &got, "/keyspaceName", &json!(keyspace));
        assert_member(case.name, &got, "/tableName", &json!(table));
        assert_member(
            case.name,
            &got,
            "/resourceArn",
            &json!(table_arn(keyspace, table)),
        );
        for (pointer, expected) in &case.checks {
            assert_member(
                case.name,
                &got,
                &format!("/autoScalingSpecification{pointer}"),
                expected,
            );
        }
        // Accepting the member must not disturb the rest of the table.
        let table_report = assert_ok(
            case.name,
            emulator
                .call(
                    "GetTable",
                    json!({"keyspaceName": keyspace, "tableName": table}),
                )
                .await,
        );
        assert_member(
            case.name,
            &table_report,
            "/capacitySpecification/throughputMode",
            &json!("PROVISIONED"),
        );
    }
}

/// RULE 2's refusals: each documented rule for `autoScalingSpecification` is answered with
/// the modelled error for it, and the request that satisfies the rule is accepted — so the
/// refusal is the rule, not a blanket no. And a table with no auto scaling settings has
/// none to report.
#[tokio::test]
async fn auto_scaling_requests_that_break_a_documented_rule_are_refused() {
    let emulator = Emulator::start();
    let keyspace = "auto_scaling_rules_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    let provisioned = json!({
        "throughputMode": "PROVISIONED",
        "readCapacityUnits": 5,
        "writeCapacityUnits": 5,
    });
    let pay_per_request = json!({"throughputMode": "PAY_PER_REQUEST"});
    let enabled = |minimum: i64, maximum: i64, target: f64| {
        json!({
            "readCapacityAutoScaling": {
                "autoScalingDisabled": false,
                "minimumUnits": minimum,
                "maximumUnits": maximum,
                "scalingPolicy": {
                    "targetTrackingScalingPolicyConfiguration": {"targetValue": target},
                },
            },
        })
    };

    // Each row: the case, its capacitySpecification (or null for none), its
    // autoScalingSpecification, and the answer expected.
    let rows: Vec<(&str, Value, Value, u16, &str)> = vec![
        (
            // "The optional auto scaling settings for a table in provisioned capacity
            // mode" — the member belongs to PROVISIONED and to nothing else.
            "pay_per_request_with_auto_scaling",
            pay_per_request.clone(),
            enabled(5, 50, 60.0),
            400,
            "ValidationException",
        ),
        (
            // The default mode is PAY_PER_REQUEST, so omitting the capacity is the same
            // rule.
            "default_capacity_mode_with_auto_scaling",
            Value::Null,
            enabled(5, 50, 60.0),
            400,
            "ValidationException",
        ),
        (
            "provisioned_with_auto_scaling",
            provisioned.clone(),
            enabled(5, 50, 60.0),
            200,
            "",
        ),
        (
            // "A double between 20 and 90": each published bound, and one past it.
            "target_value_at_the_bottom_of_the_range",
            provisioned.clone(),
            enabled(5, 50, 20.0),
            200,
            "",
        ),
        (
            "target_value_at_the_top_of_the_range",
            provisioned.clone(),
            enabled(5, 50, 90.0),
            200,
            "",
        ),
        (
            "target_value_below_the_range",
            provisioned.clone(),
            enabled(5, 50, 19.9),
            400,
            "ValidationException",
        ),
        (
            "target_value_above_the_range",
            provisioned.clone(),
            enabled(5, 50, 90.1),
            400,
            "ValidationException",
        ),
        (
            // "capacity never goes below the minimum or above the maximum range".
            "minimum_above_maximum",
            provisioned.clone(),
            enabled(50, 5, 60.0),
            400,
            "ValidationException",
        ),
        (
            "minimum_equal_to_maximum",
            provisioned.clone(),
            enabled(50, 50, 60.0),
            200,
            "",
        ),
        (
            // "To turn on auto scaling ... you must specify ... minimumUnits,
            // maximumUnits, scalingPolicy".
            "enabled_without_units",
            provisioned.clone(),
            json!({
                "readCapacityAutoScaling": {
                    "scalingPolicy": {
                        "targetTrackingScalingPolicyConfiguration": {"targetValue": 50.0},
                    },
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "enabled_without_a_scaling_policy",
            provisioned.clone(),
            json!({
                "readCapacityAutoScaling": {"minimumUnits": 5, "maximumUnits": 50},
            }),
            400,
            "ValidationException",
        ),
        (
            // Modelled: `minimumUnits`/`maximumUnits` have `range min 1`, so one past
            // that bound is refused at the boundary with the same modelled error.
            "minimum_units_below_the_modelled_range",
            provisioned.clone(),
            enabled(0, 50, 60.0),
            400,
            "ValidationException",
        ),
    ];

    for (case, capacity, specification, expected_status, expected_code) in rows {
        let mut request = json!({
            "keyspaceName": keyspace,
            "tableName": case,
            "schemaDefinition": minimal_schema("pk"),
            "autoScalingSpecification": specification,
        });
        if !capacity.is_null() {
            request["capacitySpecification"] = capacity;
        }
        let answer = emulator.call("CreateTable", request).await;
        if expected_status == 200 {
            assert_ok(case, answer);
            let got = assert_ok(
                case,
                emulator
                    .call(
                        "GetTableAutoScalingSettings",
                        json!({"keyspaceName": keyspace, "tableName": case}),
                    )
                    .await,
            );
            assert!(
                got["autoScalingSpecification"]["readCapacityAutoScaling"].is_object(),
                "case {case:?}: an accepted specification must read back, got {got}"
            );
        } else {
            assert_failure(case, answer, expected_status, expected_code);
            // A refused create stored nothing.
            assert_failure(
                case,
                emulator
                    .call(
                        "GetTable",
                        json!({"keyspaceName": keyspace, "tableName": case}),
                    )
                    .await,
                404,
                "ResourceNotFoundException",
            );
        }
    }

    // A table created without the member has no auto scaling settings to report: the
    // settings are a resource of their own, and this one does not exist.
    assert_ok(
        "create a table with no auto scaling",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "no_auto_scaling",
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    let (status, body) = emulator
        .call(
            "GetTableAutoScalingSettings",
            json!({"keyspaceName": keyspace, "tableName": "no_auto_scaling"}),
        )
        .await;
    assert_failure(
        "no_auto_scaling",
        (status, body.clone()),
        404,
        "ResourceNotFoundException",
    );
    assert_member(
        "no_auto_scaling",
        &body,
        "/resourceArn",
        &json!(table_arn(keyspace, "no_auto_scaling")),
    );

    // And an unknown keyspace is the keyspace-level not-found, naming the keyspace.
    let (status, body) = emulator
        .call(
            "GetTableAutoScalingSettings",
            json!({"keyspaceName": "no_such_ks", "tableName": "whatever"}),
        )
        .await;
    assert_failure(
        "unknown keyspace",
        (status, body.clone()),
        404,
        "ResourceNotFoundException",
    );
    assert_member(
        "unknown keyspace",
        &body,
        "/resourceArn",
        &json!(keyspace_arn("no_such_ks")),
    );
}

/// RULE 3: every operation that looks a table up and does not find it answers the
/// request-derived `unconfigured table <tableName>` message — the bare name, for any name
/// the model accepts, whether the table was never created or has finished deleting. A
/// keyspace lookup keeps its own, keyspace-naming form.
#[tokio::test]
async fn a_table_that_is_not_in_state_carries_the_request_derived_message() {
    let emulator = Emulator::start();
    let keyspace = "not_found_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    // Names spanning the modelled `TableName` shape (`len 1..48`, pattern
    // `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`): the shortest, a digit-led one, mixed case, and
    // the longest.
    let longest = "t".repeat(48);
    let names: [&str; 5] = ["a", "9", "missing_tbl", "Mixed_Case_9", longest.as_str()];
    for name in names {
        for operation in ["GetTable", "DeleteTable", "GetTableAutoScalingSettings"] {
            let case = format!("{operation} on {name:?}");
            let (status, body) = emulator
                .call(
                    operation,
                    json!({"keyspaceName": keyspace, "tableName": name}),
                )
                .await;
            assert_failure(&case, (status, body.clone()), 404, "ResourceNotFoundException");
            assert_member(
                &case,
                &body,
                "/message",
                &json!(format!("unconfigured table {name}")),
            );
            assert_member(
                &case,
                &body,
                "/resourceArn",
                &json!(table_arn(keyspace, name)),
            );
        }
    }

    // A table that existed and finished deleting is not in state either, so a second
    // DeleteTable reports it the same way.
    let deleted = "deleted_tbl";
    assert_ok(
        "create the table to delete",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": deleted,
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    // The creation span has to elapse before the table is ACTIVE and can be deleted, and
    // the deletion span before it is gone.
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "delete the table",
        emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": deleted}),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
    let (status, body) = emulator
        .call(
            "DeleteTable",
            json!({"keyspaceName": keyspace, "tableName": deleted}),
        )
        .await;
    assert_failure(
        "DeleteTable on an already-deleted table",
        (status, body.clone()),
        404,
        "ResourceNotFoundException",
    );
    assert_member(
        "DeleteTable on an already-deleted table",
        &body,
        "/message",
        &json!(format!("unconfigured table {deleted}")),
    );

    // The keyspace-level not-found is a different lookup and keeps its own form: it names
    // the keyspace, not a table.
    let (status, body) = emulator
        .call(
            "GetTable",
            json!({"keyspaceName": "no_such_ks", "tableName": "missing_tbl"}),
        )
        .await;
    assert_failure(
        "unknown keyspace",
        (status, body.clone()),
        404,
        "ResourceNotFoundException",
    );
    let message = body["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains("no_such_ks") && !message.contains("unconfigured table"),
        "case \"unknown keyspace\": a keyspace lookup keeps its own message form, got \
         {message:?}"
    );
    assert_member(
        "unknown keyspace",
        &body,
        "/resourceArn",
        &json!(keyspace_arn("no_such_ks")),
    );
}

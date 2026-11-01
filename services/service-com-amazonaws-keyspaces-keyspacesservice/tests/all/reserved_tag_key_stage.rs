// SPDX-License-Identifier: CC0-1.0

//! WHEN Amazon Keyspaces decides a tag refusal that the request alone determines, driven
//! end to end over the service's own `awsJson1_0` wire protocol through the harness in
//! [`crate::keyspace_table_lifecycle`].
//!
//! The rule under test, exercised over a SET of inputs rather than one example:
//!
//! A refusal the REQUEST ALONE determines — a tag key carrying the reserved `aws:` prefix,
//! which "the user cannot assign" (`Tag`) — is decided before any refusal that depends on
//! stored state, so one and the same key is refused with the same 400 `ValidationException`
//! whatever the store holds: whether the resource the request names exists, is in a status
//! that takes no change, or is one of the service's own. The rows therefore span the
//! state-dependent answer the request would have got with an ordinary key (a 200, a
//! `ResourceNotFoundException`, a `ConflictException`, a refusal of another kind), and the
//! same rows are driven with ordinary and with merely reserved-LOOKING keys, which must
//! still get that state-dependent answer.
//!
//! Every operation this service models that takes caller-supplied tags is covered:
//! `TagResource` and `UntagResource` (`tags`), `UpdateTable` (`cdcSpecification.tags`) and
//! `RestoreTable` (`tagsOverride`). The creates (`CreateKeyspace`, `CreateTable`) resolve
//! their tags before they reach the store already, and are pinned by
//! `tests/all/resource_tagging.rs` and `tests/all/cdc_streams.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, error_code, keyspace_arn, minimal_schema, settle_wait, table_arn,
    Emulator, REGION,
};
use super::restore_table::tags_of;

/// The keyspace and table the cases below address.
const KEYSPACE: &str = "stage_ks";
const TABLE: &str = "stage_tbl";

/// Keys a caller may not supply, built from the service's own prefix constant so a row
/// cannot drift from the rule: the prefix alone, a key that merely starts with it, and the
/// two Amazon Web Services-assigned forms the `Tag` documentation describes
/// (`smithy_query(query="shape",shape="Tag")`).
fn reserved_keys() -> Vec<String> {
    let prefix = keyspaces::RESERVED_TAG_KEY_PREFIX;
    vec![
        prefix.to_string(),
        format!("{prefix}x"),
        format!("{prefix}createdBy"),
        format!("{prefix}cloudformation:stack-name"),
    ]
}

/// Keys the rule does NOT reach: an ordinary key, and three that only LOOK reserved
/// (the prefix is the whole rule, and it is case sensitive).
fn allowed_keys() -> Vec<String> {
    ["env", "awsome", "AWS:upper", "notaws:prefixed"]
        .iter()
        .map(|key| (*key).to_string())
        .collect()
}

/// A `TagList` request member carrying one key, plus an ordinary key beside it: a request
/// is graded on the key at fault wherever in the list it sits.
fn tags_json(key: &str) -> Value {
    json!([
        {"key": "team", "value": "orders"},
        {"key": key, "value": "v"},
    ])
}

/// The `message` of a failure body, whichever casing the protocol serialized it under.
///
/// Shared with `tests/all/tag_count_limit_refusal.rs`, so there is ONE reader of a
/// refusal's sentence for the suites that assert one rather than a copy per file.
pub(crate) fn message_of(body: &Value) -> String {
    ["message", "Message"]
        .iter()
        .find_map(|member| body.get(*member).and_then(Value::as_str))
        .unwrap_or("<no message member>")
        .to_string()
}

/// Assert a call was refused for the reserved key it carried: the modelled 400
/// `ValidationException`, whose message names the key the caller may not assign and the
/// prefix that makes it one.
fn assert_reserved_key_refusal(case: &str, got: (u16, Value), key: &str) {
    let (status, body) = got;
    assert_failure(case, (status, body.clone()), 400, "ValidationException");
    let message = message_of(&body);
    assert!(
        message.contains(key) && message.contains(keyspaces::RESERVED_TAG_KEY_PREFIX),
        "case {case:?}: the refusal must name the key {key:?} and the reserved prefix, got \
         {message:?}"
    );
}

/// What a call is expected to answer: a 200, or the modelled failure of a case the store's
/// contents decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// A 200.
    Ok,
    /// The modelled failure: its HTTP status and its wire error code.
    Failure(u16, &'static str),
}

/// Assert a call answered `expected`, naming the case.
fn assert_expected(case: &str, got: (u16, Value), expected: Expected) {
    match expected {
        Expected::Ok => {
            assert_ok(case, got);
        }
        Expected::Failure(status, code) => assert_failure(case, got, status, code),
    }
}

/// One `resourceArn` row: what the ARN names, and what the SAME request answers when its
/// keys are ones the caller may supply — the state-dependent answer a reserved key has to
/// pre-empt.
struct ArnRow {
    /// What the ARN names, for the assertion messages.
    name: &'static str,
    /// The `resourceArn` member the call carries.
    resource_arn: String,
    /// What the call answers with keys the caller may supply.
    without_reserved: Expected,
}

/// Create the keyspace and table the ARN rows address.
async fn with_resources(emulator: &Emulator) {
    assert_ok(
        "create keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": KEYSPACE}))
            .await,
    );
    assert_ok(
        "create table",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": KEYSPACE,
                    "tableName": TABLE,
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
}

/// `TagResource` and `UntagResource` both decide the reserved key on the request alone, so
/// the two siblings grade one and the same key identically — whatever the ARN names.
#[tokio::test]
async fn tagging_refuses_a_reserved_key_before_it_resolves_the_arn() {
    let emulator = Emulator::start();
    with_resources(&emulator).await;
    let account = engine::emulator_account_id();
    // The service-managed keyspace every Region holds, taken from the service's own
    // catalog so the row cannot drift from it.
    let (managed_keyspace, managed_tables) = keyspaces::SERVICE_MANAGED_KEYSPACES[0];
    let managed_table = managed_tables[0];

    let rows: Vec<ArnRow> = vec![
        ArnRow {
            name: "a keyspace this store holds",
            resource_arn: keyspace_arn(KEYSPACE),
            without_reserved: Expected::Ok,
        },
        ArnRow {
            name: "a table this store holds",
            resource_arn: table_arn(KEYSPACE, TABLE),
            without_reserved: Expected::Ok,
        },
        ArnRow {
            name: "an unknown keyspace",
            resource_arn: keyspace_arn("no_such_ks"),
            without_reserved: Expected::Failure(400, "ResourceNotFoundException"),
        },
        ArnRow {
            name: "an unknown table in a keyspace that exists",
            resource_arn: table_arn(KEYSPACE, "no_such_tbl"),
            without_reserved: Expected::Failure(400, "ResourceNotFoundException"),
        },
        ArnRow {
            name: "the keyspace that exists, in another Region",
            resource_arn: format!("arn:aws:cassandra:eu-central-1:{account}:/keyspace/{KEYSPACE}/"),
            without_reserved: Expected::Failure(400, "ResourceNotFoundException"),
        },
        ArnRow {
            name: "a resource form this service does not mint",
            resource_arn: format!("arn:aws:cassandra:{REGION}:{account}:/cluster/{KEYSPACE}/"),
            without_reserved: Expected::Failure(400, "ResourceNotFoundException"),
        },
        ArnRow {
            name: "a service-managed keyspace, which takes no change from a caller",
            resource_arn: keyspace_arn(managed_keyspace),
            without_reserved: Expected::Failure(400, "ValidationException"),
        },
        ArnRow {
            name: "a system table of a service-managed keyspace",
            resource_arn: table_arn(managed_keyspace, managed_table),
            without_reserved: Expected::Failure(400, "ValidationException"),
        },
    ];

    for row in &rows {
        for key in reserved_keys() {
            // Both siblings, and their answers compared: the reserved key is a property of
            // the request, so neither the operation nor the store's contents may change
            // how it is graded.
            let mut answers: Vec<(u16, String, String)> = Vec::new();
            for operation in ["TagResource", "UntagResource"] {
                let case = format!("{operation} {key:?} on {}", row.name);
                let (status, body) = emulator
                    .call(
                        operation,
                        json!({"resourceArn": row.resource_arn, "tags": tags_json(&key)}),
                    )
                    .await;
                assert_reserved_key_refusal(&case, (status, body.clone()), &key);
                answers.push((status, error_code(&body), message_of(&body)));
            }
            assert_eq!(
                answers[0], answers[1],
                "case {key:?} on {}: TagResource and UntagResource must grade the same \
                 reserved key identically, got {answers:?}",
                row.name
            );
        }
    }

    // The same rows with keys the caller MAY supply get the answer the store's contents
    // decide — so what the rows above pin is the stage of the reserved-key rule, not a
    // handler that refuses everything.
    for row in &rows {
        for key in allowed_keys() {
            let case = format!("TagResource {key:?} on {}", row.name);
            assert_expected(
                &case,
                emulator
                    .call(
                        "TagResource",
                        json!({"resourceArn": row.resource_arn, "tags": tags_json(&key)}),
                    )
                    .await,
                row.without_reserved,
            );
            if row.without_reserved == Expected::Ok {
                let held = tags_of(&case, &emulator, &row.resource_arn).await;
                assert!(
                    held.iter().any(|(stored, _)| stored == &key),
                    "case {case:?}: expected the accepted key to be held, got {held:?}"
                );
            }
            let case = format!("UntagResource {key:?} on {}", row.name);
            assert_expected(
                &case,
                emulator
                    .call(
                        "UntagResource",
                        json!({"resourceArn": row.resource_arn, "tags": tags_json(&key)}),
                    )
                    .await,
                row.without_reserved,
            );
            if row.without_reserved == Expected::Ok {
                let held = tags_of(&case, &emulator, &row.resource_arn).await;
                assert!(
                    !held.iter().any(|(stored, _)| stored == &key),
                    "case {case:?}: expected the removed key to be gone, got {held:?}"
                );
            }
        }
    }

    // No refused call stored anything: the resources that exist hold no reserved key.
    for resource_arn in [keyspace_arn(KEYSPACE), table_arn(KEYSPACE, TABLE)] {
        let held = tags_of("nothing reserved was stored", &emulator, &resource_arn).await;
        assert!(
            !held
                .iter()
                .any(|(key, _)| key.starts_with(keyspaces::RESERVED_TAG_KEY_PREFIX)),
            "case {resource_arn:?}: expected no reserved key to have been stored, got {held:?}"
        );
    }
}

/// One `UpdateTable` row: the table a `cdcSpecification` is applied to, whether it is
/// created immediately before the call (so it is still `CREATING` when the call lands),
/// and what the same request answers with a key the caller may supply.
struct UpdateRow {
    /// What the request names, for the assertion messages.
    name: &'static str,
    /// The `keyspaceName` member.
    keyspace: String,
    /// The `tableName` member, or the stem of it for a `fresh` row.
    table: String,
    /// Whether the table is created immediately before each call.
    fresh: bool,
    /// What the call answers with keys the caller may supply.
    without_reserved: Expected,
}

/// The tags a `cdcSpecification` applies to the table's stream follow the same rule at the
/// same stage: decided on the request, before the table is resolved.
#[tokio::test]
async fn update_table_refuses_a_reserved_stream_tag_key_before_it_resolves_the_table() {
    let emulator = Emulator::start();
    with_resources(&emulator).await;
    let (managed_keyspace, managed_tables) = keyspaces::SERVICE_MANAGED_KEYSPACES[0];

    let rows: Vec<UpdateRow> = vec![
        UpdateRow {
            name: "a table this store holds, in ACTIVE",
            keyspace: KEYSPACE.to_string(),
            table: TABLE.to_string(),
            fresh: false,
            without_reserved: Expected::Ok,
        },
        UpdateRow {
            name: "an unknown keyspace",
            keyspace: "no_such_ks".to_string(),
            table: TABLE.to_string(),
            fresh: false,
            without_reserved: Expected::Failure(400, "ResourceNotFoundException"),
        },
        UpdateRow {
            name: "an unknown table in a keyspace that exists",
            keyspace: KEYSPACE.to_string(),
            table: "no_such_tbl".to_string(),
            fresh: false,
            without_reserved: Expected::Failure(400, "ResourceNotFoundException"),
        },
        UpdateRow {
            name: "a table still CREATING, which takes no update yet",
            keyspace: KEYSPACE.to_string(),
            table: "fresh_tbl".to_string(),
            fresh: true,
            without_reserved: Expected::Failure(400, "ConflictException"),
        },
        UpdateRow {
            name: "a system table of a service-managed keyspace",
            keyspace: managed_keyspace.to_string(),
            table: managed_tables[0].to_string(),
            fresh: false,
            without_reserved: Expected::Failure(400, "ValidationException"),
        },
    ];

    // A table name used once per call, so a row that needs a table still in `CREATING`
    // gets a freshly created one for every call rather than racing the clock.
    let mut minted = 0usize;
    for row in &rows {
        for key in reserved_keys().iter().chain(allowed_keys().iter()) {
            let reserved = key.starts_with(keyspaces::RESERVED_TAG_KEY_PREFIX);
            let table = match row.fresh {
                false => row.table.clone(),
                true => {
                    minted += 1;
                    let table = format!("{}_{minted}", row.table);
                    assert_ok(
                        &table,
                        emulator
                            .call(
                                "CreateTable",
                                json!({
                                    "keyspaceName": row.keyspace,
                                    "tableName": table,
                                    "schemaDefinition": minimal_schema("pk"),
                                }),
                            )
                            .await,
                    );
                    table
                }
            };
            let case = format!("UpdateTable {key:?} on {}", row.name);
            let answer = emulator
                .call(
                    "UpdateTable",
                    json!({
                        "keyspaceName": row.keyspace,
                        "tableName": table,
                        "cdcSpecification": {
                            "status": "ENABLED",
                            "tags": tags_json(key),
                        },
                    }),
                )
                .await;
            match reserved {
                true => assert_reserved_key_refusal(&case, answer, key),
                false => assert_expected(&case, answer, row.without_reserved),
            }
            // An accepted update leaves the table `UPDATING` for a span, which takes no
            // further update: let it settle before the next row reaches the same table.
            if !reserved && row.without_reserved == Expected::Ok {
                tokio::time::sleep(settle_wait()).await;
            }
        }
    }
}

/// One `RestoreTable` row: the source table a `tagsOverride` is restored from, and what
/// the same request answers with a key the caller may supply.
struct RestoreRow {
    /// What the request names, for the assertion messages.
    name: &'static str,
    /// The `sourceKeyspaceName` member.
    source_keyspace: String,
    /// The `sourceTableName` member.
    source_table: String,
    /// What the call answers with keys the caller may supply.
    without_reserved: Expected,
}

/// `tagsOverride` is the whole tag set of the table a restore creates, so both of its
/// per-resource rules are the request's alone — and the reserved one is decided before the
/// source table is resolved.
#[tokio::test]
async fn restore_table_refuses_a_reserved_tags_override_key_before_it_resolves_the_source() {
    let emulator = Emulator::start();
    let source_keyspace = "stage_restore_src_ks";
    let target_keyspace = "stage_restore_tgt_ks";
    for keyspace in [source_keyspace, target_keyspace] {
        assert_ok(
            keyspace,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
                .await,
        );
    }
    // A table point-in-time recovery is enabled on has a restorable window; one left at
    // the modelled default has none at all, which is the 400 that row expects.
    for (table, recovery) in [
        ("restorable", keyspaces::POINT_IN_TIME_RECOVERY_ENABLED),
        (
            "not_restorable",
            keyspaces::DEFAULT_POINT_IN_TIME_RECOVERY_STATUS,
        ),
    ] {
        assert_ok(
            table,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": source_keyspace,
                        "tableName": table,
                        "schemaDefinition": minimal_schema("pk"),
                        "pointInTimeRecovery": {"status": recovery},
                    }),
                )
                .await,
        );
    }
    tokio::time::sleep(settle_wait()).await;
    let (managed_keyspace, managed_tables) = keyspaces::SERVICE_MANAGED_KEYSPACES[0];

    let rows: Vec<RestoreRow> = vec![
        RestoreRow {
            name: "a source table this store holds, in ACTIVE",
            source_keyspace: source_keyspace.to_string(),
            source_table: "restorable".to_string(),
            without_reserved: Expected::Ok,
        },
        RestoreRow {
            name: "an unknown source keyspace",
            source_keyspace: "no_such_ks".to_string(),
            source_table: "restorable".to_string(),
            without_reserved: Expected::Failure(400, "ResourceNotFoundException"),
        },
        RestoreRow {
            name: "an unknown source table in a keyspace that exists",
            source_keyspace: source_keyspace.to_string(),
            source_table: "no_such_tbl".to_string(),
            without_reserved: Expected::Failure(400, "ResourceNotFoundException"),
        },
        RestoreRow {
            name: "a source table with no restorable point in time",
            source_keyspace: source_keyspace.to_string(),
            source_table: "not_restorable".to_string(),
            without_reserved: Expected::Failure(400, "ValidationException"),
        },
        RestoreRow {
            name: "a source table of a service-managed keyspace",
            source_keyspace: managed_keyspace.to_string(),
            source_table: managed_tables[0].to_string(),
            without_reserved: Expected::Failure(400, "ValidationException"),
        },
    ];

    // One target name per call: a restore that is accepted creates a table, and a name
    // already taken would be a conflict of its own.
    let mut minted = 0usize;
    for row in &rows {
        for key in reserved_keys().iter().chain(allowed_keys().iter()) {
            let reserved = key.starts_with(keyspaces::RESERVED_TAG_KEY_PREFIX);
            minted += 1;
            let target_table = format!("restored_{minted}");
            let case = format!("RestoreTable {key:?} from {}", row.name);
            let answer = emulator
                .call(
                    "RestoreTable",
                    json!({
                        "sourceKeyspaceName": row.source_keyspace,
                        "sourceTableName": row.source_table,
                        "targetKeyspaceName": target_keyspace,
                        "targetTableName": target_table,
                        "tagsOverride": tags_json(key),
                    }),
                )
                .await;
            match reserved {
                true => {
                    assert_reserved_key_refusal(&case, answer, key);
                    // A refused restore created no table to carry the key.
                    assert_failure(
                        &case,
                        emulator
                            .call(
                                "GetTable",
                                json!({
                                    "keyspaceName": target_keyspace,
                                    "tableName": target_table,
                                }),
                            )
                            .await,
                        400,
                        "ResourceNotFoundException",
                    );
                }
                false => {
                    assert_expected(&case, answer, row.without_reserved);
                    if row.without_reserved == Expected::Ok {
                        // The accepted override IS the restored table's tag set.
                        let held = tags_of(
                            &case,
                            &emulator,
                            &table_arn(target_keyspace, &target_table),
                        )
                        .await;
                        assert!(
                            held.iter().any(|(stored, _)| stored == key),
                            "case {case:?}: expected the restored table to hold the \
                             accepted key, got {held:?}"
                        );
                        // At most `keyspaces::MAX_CONCURRENT_RESTORES` restores run at
                        // once, so let this one finish before the next call.
                        tokio::time::sleep(settle_wait()).await;
                    }
                }
            }
        }
        // A restore in flight counts against the account's concurrency: let anything this
        // row accepted finish before the next row's calls.
        tokio::time::sleep(settle_wait()).await;
    }
}

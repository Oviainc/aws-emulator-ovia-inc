// SPDX-License-Identifier: CC0-1.0

//! WHAT Amazon Keyspaces answers a call that would leave a resource holding more tags
//! than one resource may hold, driven end to end over the service's own `awsJson1_0` wire
//! protocol through the harnesses in [`crate::keyspace_table_lifecycle`] and
//! [`crate::cdc_streams`].
//!
//! The rule under test, exercised over a SET of inputs rather than one example:
//!
//! A call whose RESULTING tag set would carry a resource past the published
//! per-resource tag limit ("You can add up to 50 tags to a single Amazon Keyspaces
//! resource", `smithy_query(query="shape",shape="Tag")`) is refused as an invalid
//! REQUEST — the modelled `ValidationException` — carrying the service's own sentence for
//! it, and stores nothing. It is NOT the service-quota failure: a caller past this limit
//! has to send fewer tags, not ask for more quota, so the error class a client branches on
//! is the malformed-request one.
//!
//! The sentence is FIXED text in full: it reports neither the resource, nor how many tags
//! it already holds, nor the count the call would have left behind. The only value in it
//! is the published limit, which the rows take from the service's own constant so they
//! cannot drift from it.
//!
//! Every operation this service models that takes caller-supplied tags is covered:
//! `TagResource` (`tags`), `CreateKeyspace` and `CreateTable` (`tags`), `RestoreTable`
//! (`tagsOverride`) and `UpdateTable` (`cdcSpecification.tags`, which lands on the CDC
//! stream's own tag set). The reserved-key refusal the same classifier also serves keeps
//! its own sentence, which one row pins.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::cdc_streams::Cdc;
use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, ensure_keyspace, keyspace_arn, minimal_schema, settle_wait,
    table_arn, Emulator,
};
use super::reserved_tag_key_stage::message_of;
use super::resource_tagging::tags_json;
use super::restore_table::tags_of;

/// The keyspace the cases below tag resources in.
const KEYSPACE: &str = "tag_count_ks";

/// The published per-resource tag limit, from the service's own constant.
fn limit() -> usize {
    keyspaces::MAX_TAGS_PER_RESOURCE
}

/// The sentence the service words this refusal with, with the published limit in it.
fn expected_message() -> String {
    format!(
        "Tag count exceed the imposed limit, The resultant tag set must not have more than {} \
         user tags.",
        limit()
    )
}

/// `count` distinct tags whose keys carry `prefix`, so one case's keys cannot collide with
/// another's and the key order is the creation order.
fn distinct(prefix: &str, count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("{prefix}{index:03}"), "v".to_string()))
        .collect()
}

/// Assert a call was refused for the tag count it would have left behind: the modelled 400
/// `ValidationException`, with the service's sentence for it in full.
fn assert_count_refusal(case: &str, got: (u16, Value)) {
    let (status, body) = got;
    assert_failure(case, (status, body.clone()), 400, "ValidationException");
    let message = message_of(&body);
    assert_eq!(
        message,
        expected_message(),
        "case {case:?}: expected the message {:?}, got {message:?}",
        expected_message()
    );
}

/// RULE, at `TagResource`: a merge that would carry the resource past the limit is the
/// malformed-request refusal, the resource keeps exactly the tags it had, and the room the
/// refused call did not consume is still there afterwards.
///
/// The rows differ along what the rule generalises over: whether the limit is breached by
/// one call or across calls, how far past it the resulting set would be, how many tags the
/// resource already holds, and whether the refused call's keys overlap the held ones. Both
/// resource forms this service mints an ARN for are driven with every row.
#[tokio::test]
async fn tagging_past_the_per_resource_tag_limit_is_an_invalid_request() {
    /// One row: the calls that must be accepted, then the one that must be refused, and
    /// how many tags the resource holds once the refusal is in.
    struct Case {
        name: String,
        accepted: Vec<Vec<(String, String)>>,
        refused: Vec<(String, String)>,
        held: usize,
    }

    let limit = limit();
    let cases = vec![
        Case {
            name: format!("{} tags in one call, one past the limit", limit + 1),
            accepted: vec![],
            refused: distinct("one_past", limit + 1),
            held: 0,
        },
        Case {
            // The modelled `TagList` maximum is 60, so a request ten past the limit is
            // still inside the request bound: what refuses it is the resulting set.
            name: format!("{} tags in one call, ten past the limit", limit + 10),
            accepted: vec![],
            refused: distinct("ten_past", limit + 10),
            held: 0,
        },
        Case {
            name: "a resource at the limit, then one further key".to_string(),
            accepted: vec![distinct("held", limit)],
            refused: vec![("one_more".to_string(), "v".to_string())],
            held: limit,
        },
        Case {
            name: "a resource ten below the limit, then eleven further keys".to_string(),
            accepted: vec![distinct("held", limit - 10)],
            refused: distinct("eleven", 11),
            held: limit - 10,
        },
        Case {
            name: format!("a resource holding one tag, then {limit} further keys"),
            accepted: vec![vec![("solo".to_string(), "v".to_string())]],
            refused: distinct("batch", limit),
            held: 1,
        },
        Case {
            name: "a resource at the limit, then a batch of held keys with one new one"
                .to_string(),
            accepted: vec![distinct("held", limit)],
            refused: {
                let mut tags = distinct("held", limit);
                tags.push(("extra".to_string(), "w".to_string()));
                tags
            },
            held: limit,
        },
    ];

    for case in &cases {
        // A fresh emulator per row, so each row starts from resources holding no tags.
        let emulator = Emulator::start();
        ensure_keyspace(&emulator, KEYSPACE).await;
        assert_ok(
            &case.name,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": KEYSPACE,
                        "tableName": "tbl",
                        "schemaDefinition": minimal_schema("pk"),
                    }),
                )
                .await,
        );
        tokio::time::sleep(settle_wait()).await;

        for resource_arn in [keyspace_arn(KEYSPACE), table_arn(KEYSPACE, "tbl")] {
            let case_name = format!("{} on {resource_arn}", case.name);
            for call in &case.accepted {
                assert_ok(
                    &case_name,
                    emulator
                        .call(
                            "TagResource",
                            json!({"resourceArn": resource_arn, "tags": tags_json(call)}),
                        )
                        .await,
                );
            }
            let before = tags_of(&case_name, &emulator, &resource_arn).await;
            assert_eq!(
                before.len(),
                case.held,
                "case {case_name:?}: expected {} tags before the refused call, got {before:?}",
                case.held
            );

            assert_count_refusal(
                &case_name,
                emulator
                    .call(
                        "TagResource",
                        json!({"resourceArn": resource_arn, "tags": tags_json(&case.refused)}),
                    )
                    .await,
            );

            let after = tags_of(&case_name, &emulator, &resource_arn).await;
            assert_eq!(
                after, before,
                "case {case_name:?}: a refused call must leave the tag set as it was, expected \
                 {before:?}, got {after:?}"
            );

            // The refused call consumed nothing, so the room it did not take is still
            // there: filling it exactly to the limit is accepted.
            let room = limit - case.held;
            if room > 0 {
                assert_ok(
                    &case_name,
                    emulator
                        .call(
                            "TagResource",
                            json!({
                                "resourceArn": resource_arn,
                                "tags": tags_json(&distinct("room", room)),
                            }),
                        )
                        .await,
                );
            }
            let filled = tags_of(&case_name, &emulator, &resource_arn).await;
            assert_eq!(
                filled.len(),
                limit,
                "case {case_name:?}: expected the resource to fill to exactly {limit} tags, got \
                 {} ({filled:?})",
                filled.len()
            );
        }
    }
}

/// RULE, at the operations that take a tag set for a resource they create: the same
/// refusal, with the same sentence, and no resource created to carry the tags; a set
/// landing exactly on the limit is accepted and read back.
///
/// The rows span the operations and the member that carries the tags, and how far past the
/// limit the request is (one, five and ten, each inside the modelled `TagList` maximum of
/// 60).
#[tokio::test]
async fn a_create_refuses_an_over_limit_tag_set_the_same_way() {
    let limit = limit();
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, KEYSPACE).await;
    // `RestoreTable` reads its source table in `ACTIVE`, and only a table point-in-time
    // recovery is enabled on has a restorable point at all.
    assert_ok(
        "source table",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": KEYSPACE,
                    "tableName": "src",
                    "schemaDefinition": minimal_schema("pk"),
                    "pointInTimeRecovery": {"status": keyspaces::POINT_IN_TIME_RECOVERY_ENABLED},
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    for over in [1usize, 5, 10] {
        let over_limit = tags_json(&distinct("over", limit + over));
        let at_limit = tags_json(&distinct("at", limit));

        // CreateKeyspace(tags): refused, and no keyspace created to hold them.
        let refused_keyspace = format!("over_ks{over}");
        let case = format!("CreateKeyspace {} tags", limit + over);
        assert_count_refusal(
            &case,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({"keyspaceName": refused_keyspace, "tags": over_limit}),
                )
                .await,
        );
        assert_failure(
            &case,
            emulator
                .call("GetKeyspace", json!({"keyspaceName": refused_keyspace}))
                .await,
            400,
            "ResourceNotFoundException",
        );
        let accepted_keyspace = format!("at_ks{over}");
        assert_ok(
            &case,
            emulator
                .call(
                    "CreateKeyspace",
                    json!({"keyspaceName": accepted_keyspace, "tags": at_limit}),
                )
                .await,
        );
        let held = tags_of(&case, &emulator, &keyspace_arn(&accepted_keyspace)).await;
        assert_eq!(
            held.len(),
            limit,
            "case {case:?}: expected the created keyspace to hold exactly {limit} tags, got \
             {} ({held:?})",
            held.len()
        );

        // CreateTable(tags): refused, and no table created to hold them.
        let refused_table = format!("over_tbl{over}");
        let case = format!("CreateTable {} tags", limit + over);
        assert_count_refusal(
            &case,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": KEYSPACE,
                        "tableName": refused_table,
                        "schemaDefinition": minimal_schema("pk"),
                        "tags": over_limit,
                    }),
                )
                .await,
        );
        assert_failure(
            &case,
            emulator
                .call(
                    "GetTable",
                    json!({"keyspaceName": KEYSPACE, "tableName": refused_table}),
                )
                .await,
            400,
            "ResourceNotFoundException",
        );
        let accepted_table = format!("at_tbl{over}");
        assert_ok(
            &case,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": KEYSPACE,
                        "tableName": accepted_table,
                        "schemaDefinition": minimal_schema("pk"),
                        "tags": at_limit,
                    }),
                )
                .await,
        );
        let held = tags_of(&case, &emulator, &table_arn(KEYSPACE, &accepted_table)).await;
        assert_eq!(
            held.len(),
            limit,
            "case {case:?}: expected the created table to hold exactly {limit} tags, got {} \
             ({held:?})",
            held.len()
        );

        // RestoreTable(tagsOverride): the whole tag set of the table a restore creates.
        let refused_restore = format!("over_restored{over}");
        let case = format!("RestoreTable {} tagsOverride", limit + over);
        assert_count_refusal(
            &case,
            emulator
                .call(
                    "RestoreTable",
                    json!({
                        "sourceKeyspaceName": KEYSPACE,
                        "sourceTableName": "src",
                        "targetKeyspaceName": KEYSPACE,
                        "targetTableName": refused_restore,
                        "tagsOverride": over_limit,
                    }),
                )
                .await,
        );
        assert_failure(
            &case,
            emulator
                .call(
                    "GetTable",
                    json!({"keyspaceName": KEYSPACE, "tableName": refused_restore}),
                )
                .await,
            400,
            "ResourceNotFoundException",
        );
        let accepted_restore = format!("at_restored{over}");
        assert_ok(
            &case,
            emulator
                .call(
                    "RestoreTable",
                    json!({
                        "sourceKeyspaceName": KEYSPACE,
                        "sourceTableName": "src",
                        "targetKeyspaceName": KEYSPACE,
                        "targetTableName": accepted_restore,
                        "tagsOverride": at_limit,
                    }),
                )
                .await,
        );
        // A restore in flight counts against the account's concurrency, so let it finish
        // before the next row's.
        tokio::time::sleep(settle_wait()).await;
        let held = tags_of(&case, &emulator, &table_arn(KEYSPACE, &accepted_restore)).await;
        assert_eq!(
            held.len(),
            limit,
            "case {case:?}: expected the restored table to hold exactly {limit} tags, got {} \
             ({held:?})",
            held.len()
        );
    }

    // The OTHER refusal the same classifier serves is unchanged: a key the caller may not
    // assign keeps its own sentence, so the two are still told apart.
    let (status, body) = emulator
        .call(
            "CreateKeyspace",
            json!({
                "keyspaceName": "reserved_ks",
                "tags": [{"key": format!("{}owner", keyspaces::RESERVED_TAG_KEY_PREFIX), "value": "v"}],
            }),
        )
        .await;
    assert_failure(
        "a reserved key keeps its own refusal",
        (status, body.clone()),
        400,
        "ValidationException",
    );
    let message = message_of(&body);
    assert_ne!(
        message,
        expected_message(),
        "case \"a reserved key keeps its own refusal\": the reserved-key refusal must keep its \
         own sentence, got {message:?}"
    );
}

/// RULE, at `UpdateTable`'s `cdcSpecification.tags`: the stream is a resource of its own,
/// so the tags a specification applies to it are counted against the same limit — whether
/// they all come from the specification or partly from the table's own tags the
/// specification asks to inherit — and an over-limit set is the same refusal, with no
/// stream created to carry it.
#[tokio::test]
async fn a_cdc_specification_refuses_an_over_limit_stream_tag_set_the_same_way() {
    let limit = limit();
    for (case, specification_tags, table_tags, propagate) in [
        ("one_past_the_limit", limit + 1, 0usize, false),
        ("five_past_the_limit", limit + 5, 0, false),
        // The table's own tags fill the set to the limit, so one further key from the
        // specification is one past it: the resulting set counts both sources.
        ("inherited_plus_one", 1, limit, true),
    ] {
        let emulator = Cdc::start();
        let keyspace = "cdc_tag_count_ks";
        let table = "tbl";
        emulator.create_keyspace(keyspace).await;
        let mut create = json!({
            "keyspaceName": keyspace,
            "tableName": table,
            "schemaDefinition": minimal_schema("pk"),
        });
        if table_tags > 0 {
            create["tags"] = tags_json(&distinct("table", table_tags));
        }
        assert_ok(case, emulator.keyspaces("CreateTable", create).await);
        tokio::time::sleep(settle_wait()).await;

        let mut specification = json!({
            "status": "ENABLED",
            "tags": tags_json(&distinct("stream", specification_tags)),
        });
        if propagate {
            specification["propagateTags"] = json!(keyspaces::CDC_PROPAGATE_TAGS_TABLE);
        }
        assert_count_refusal(
            case,
            emulator
                .keyspaces(
                    "UpdateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "cdcSpecification": specification,
                    }),
                )
                .await,
        );
        // Nothing was written: no stream exists to carry the refused tags.
        let listed = emulator.listed_stream_arns(case, keyspace, table).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused specification must create no stream, got {listed:?}"
        );

        // A specification landing exactly on the limit is accepted, and the stream reports
        // exactly that tag set back.
        let accepted = match propagate {
            true => json!({
                "status": "ENABLED",
                "propagateTags": keyspaces::CDC_PROPAGATE_TAGS_TABLE,
            }),
            false => json!({
                "status": "ENABLED",
                "tags": tags_json(&distinct("stream", limit)),
            }),
        };
        assert_ok(
            case,
            emulator
                .keyspaces(
                    "UpdateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "cdcSpecification": accepted,
                    }),
                )
                .await,
        );
        tokio::time::sleep(settle_wait()).await;
        let listed = emulator.listed_stream_arns(case, keyspace, table).await;
        let [stream_arn] = listed.as_slice() else {
            panic!("case {case:?}: expected exactly one stream, got {listed:?}");
        };
        let reported = assert_ok(
            case,
            emulator
                .keyspaces(
                    "ListTagsForResource",
                    json!({"resourceArn": stream_arn}),
                )
                .await,
        );
        let held = reported["tags"].as_array().map(Vec::len).unwrap_or_default();
        assert_eq!(
            held, limit,
            "case {case:?}: expected the stream to hold exactly {limit} tags, got {held} \
             ({reported})"
        );
    }
}

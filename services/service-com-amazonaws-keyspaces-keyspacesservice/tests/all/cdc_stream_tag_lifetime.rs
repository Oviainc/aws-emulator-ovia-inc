// SPDX-License-Identifier: CC0-1.0

//! A CDC stream's TAG SET lives exactly as long as the stream itself, over the wire.
//!
//! The rule under test: a stream's tags belong to the stream, so the call that forgets a
//! stream in the CDC stream store drops that stream's tags in the same call — whichever
//! control-plane call forgets it, and however many streams it forgets at once. From that
//! instant on, every operation that addresses the stream by ARN answers the modelled 404
//! for it: `GetStream` on the KeyspacesStreams surface and `ListTagsForResource` /
//! `TagResource` / `UntagResource` on the Keyspaces surface alike. A stream the call did
//! NOT forget keeps its tags.
//!
//! The assertions are made IMMEDIATELY after the forgetting call, which is inside the
//! transient `DELETING` span the keyspace and its tables are reported in — the span in
//! which the two surfaces could drift — and again once that span has elapsed.
//!
//! The rows span what the rule generalises over: which call forgets the streams
//! (`DeleteTable`, `DeleteKeyspace`), how many tables the keyspace holds, and how many
//! streams one table holds (a table keeps its disabled streams beside the live one, each
//! with its own tag set).

use serde_json::{json, Value};

use super::cdc_streams::Cdc;
use super::keyspace_table_lifecycle::{assert_failure, assert_ok, minimal_schema, settle_wait};

/// Which control-plane call forgets the streams of the keyspace under test.
#[derive(Debug, Clone, Copy)]
enum Forget {
    /// `DeleteTable` of the keyspace's FIRST table: only that table's streams go.
    FirstTable,
    /// `DeleteKeyspace`: every stream of every table of the keyspace goes.
    WholeKeyspace,
}

/// One row: a keyspace shaped as `tables` describes — `(table name, how many streams it
/// holds)` — and the call that forgets streams in it.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    tables: &'static [(&'static str, usize)],
    forget: Forget,
}

/// The tag key every stream in this suite carries, so a tag set is recognisable whatever
/// stream it belongs to.
const STREAM_TAG_KEY: &str = "stream_tag";

/// Create `table` in `keyspace` with change data capture enabled and a tag of its own on
/// the stream, and (for `streams_held` of 2) turn CDC off and on again so the table holds
/// the disabled stream beside the live one, each with its own tag set.
///
/// Answers with the ARNs of the streams the table then holds, having checked that the
/// control plane answers tags for every one of them: that is the state the forgetting
/// call below has to undo.
async fn table_holding_streams(
    emulator: &Cdc,
    case: &str,
    keyspace: &str,
    table: &str,
    streams_held: usize,
) -> Vec<String> {
    assert_ok(
        case,
        emulator
            .keyspaces(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": minimal_schema("pk"),
                    "cdcSpecification": {
                        "status": "ENABLED",
                        "tags": [{"key": STREAM_TAG_KEY, "value": table}],
                    },
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    if streams_held > 1 {
        // Turning CDC off leaves the stream in the store (`StreamStatus` models
        // `DISABLED`), and turning it on again mints a NEW one: the table then holds two
        // streams, each with the tags its own specification carried.
        assert_ok(
            case,
            emulator
                .keyspaces(
                    "UpdateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "cdcSpecification": {"status": "DISABLED"},
                    }),
                )
                .await,
        );
        tokio::time::sleep(settle_wait()).await;
        assert_ok(
            case,
            emulator
                .keyspaces(
                    "UpdateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "cdcSpecification": {
                            "status": "ENABLED",
                            "tags": [{"key": STREAM_TAG_KEY, "value": format!("{table}_again")}],
                        },
                    }),
                )
                .await,
        );
        tokio::time::sleep(settle_wait()).await;
    }

    let held = emulator.listed_stream_arns(case, keyspace, table).await;
    assert_eq!(
        held.len(),
        streams_held,
        "case {case:?}: table {table:?} must hold {streams_held} stream(s), got {held:?}"
    );
    for stream_arn in &held {
        assert_tags_answered(emulator, case, stream_arn).await;
    }
    held
}

/// Assert the Keyspaces surface answers tags for `stream_arn`, and that the stream the
/// ARN names is readable on the KeyspacesStreams surface: the two halves that must stop
/// answering together.
async fn assert_tags_answered(emulator: &Cdc, case: &str, stream_arn: &str) {
    let listed = assert_ok(
        case,
        emulator
            .keyspaces("ListTagsForResource", json!({"resourceArn": stream_arn}))
            .await,
    );
    let keys: Vec<&str> = listed["tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| tag["key"].as_str())
                .collect::<Vec<&str>>()
        })
        .unwrap_or_default();
    assert!(
        keys.contains(&STREAM_TAG_KEY),
        "case {case:?}: the stream {stream_arn} must answer with its own tags, got {listed}"
    );
    assert_ok(
        case,
        emulator
            .streams("GetStream", json!({"streamArn": stream_arn}))
            .await,
    );
}

/// Assert NOTHING answers for `stream_arn` any more: the stream is gone from the stream
/// store, and so is its tag set — on the read AND on both writes.
async fn assert_nothing_answers(emulator: &Cdc, case: &str, stream_arn: &str) {
    assert_failure(
        case,
        emulator
            .streams("GetStream", json!({"streamArn": stream_arn}))
            .await,
        404,
        "ResourceNotFoundException",
    );
    assert_failure(
        case,
        emulator
            .keyspaces("ListTagsForResource", json!({"resourceArn": stream_arn}))
            .await,
        404,
        "ResourceNotFoundException",
    );
    assert_failure(
        case,
        emulator
            .keyspaces(
                "TagResource",
                json!({
                    "resourceArn": stream_arn,
                    "tags": [{"key": "late_tag", "value": "v"}],
                }),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );
    assert_failure(
        case,
        emulator
            .keyspaces(
                "UntagResource",
                json!({
                    "resourceArn": stream_arn,
                    "tags": [{"key": STREAM_TAG_KEY, "value": "v"}],
                }),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );
}

#[tokio::test]
async fn a_forgotten_cdc_stream_takes_its_tag_set_with_it() {
    let rows: [Row; 5] = [
        Row {
            case: "delete_table_one_stream",
            tables: &[("dropped", 1), ("kept", 1)],
            forget: Forget::FirstTable,
        },
        Row {
            case: "delete_table_two_streams",
            tables: &[("dropped", 2), ("kept", 1)],
            forget: Forget::FirstTable,
        },
        Row {
            case: "delete_keyspace_one_table",
            tables: &[("only", 1)],
            forget: Forget::WholeKeyspace,
        },
        Row {
            case: "delete_keyspace_three_tables",
            tables: &[("one", 1), ("two", 1), ("three", 1)],
            forget: Forget::WholeKeyspace,
        },
        Row {
            case: "delete_keyspace_two_streams_on_one_table",
            tables: &[("busy", 2), ("quiet", 1)],
            forget: Forget::WholeKeyspace,
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Cdc::start();

        // A keyspace the forgetting call does not name at all: its stream, and its
        // stream's tags, must still be there afterwards. That is what makes the rule a
        // rule about the streams FORGOTTEN rather than a blanket wipe.
        let bystander_keyspace = "bystander_ks";
        emulator.create_keyspace(bystander_keyspace).await;
        let bystander =
            table_holding_streams(&emulator, case, bystander_keyspace, "bystander_tbl", 1).await;

        let keyspace = format!("tags_{case}");
        emulator.create_keyspace(&keyspace).await;
        let mut held: Vec<(&str, Vec<String>)> = Vec::new();
        for (table, streams_held) in row.tables {
            held.push((
                table,
                table_holding_streams(&emulator, case, &keyspace, table, *streams_held).await,
            ));
        }

        // The forgetting call, and what it is expected to forget.
        let (forgotten, survivors): (Vec<String>, Vec<String>) = match row.forget {
            Forget::FirstTable => {
                let (table, _) = held[0];
                assert_ok(
                    case,
                    emulator
                        .keyspaces(
                            "DeleteTable",
                            json!({"keyspaceName": keyspace, "tableName": table}),
                        )
                        .await,
                );
                (
                    held[0].1.clone(),
                    held[1..]
                        .iter()
                        .flat_map(|(_, arns)| arns.clone())
                        .chain(bystander.clone())
                        .collect(),
                )
            }
            Forget::WholeKeyspace => {
                assert_ok(
                    case,
                    emulator
                        .keyspaces("DeleteKeyspace", json!({"keyspaceName": keyspace}))
                        .await,
                );
                (
                    held.iter().flat_map(|(_, arns)| arns.clone()).collect(),
                    bystander.clone(),
                )
            }
        };

        // IMMEDIATELY: the keyspace and its tables are still reported in their transient
        // `DELETING` status, and the stream store has already forgotten the streams — so
        // nothing may answer for them here either.
        assert!(
            !forgotten.is_empty(),
            "case {case:?}: the row must forget at least one stream"
        );
        for stream_arn in &forgotten {
            assert_nothing_answers(&emulator, case, stream_arn).await;
        }
        for stream_arn in &survivors {
            assert_tags_answered(&emulator, case, stream_arn).await;
        }

        // And once the transient span has elapsed and the records themselves are gone,
        // the answers are unchanged.
        tokio::time::sleep(settle_wait()).await;
        for stream_arn in &forgotten {
            assert_nothing_answers(&emulator, case, stream_arn).await;
        }
        for stream_arn in &survivors {
            assert_tags_answered(&emulator, case, stream_arn).await;
        }
    }
}

/// The same rule from the other side: a stream the control plane has NOT forgotten keeps
/// answering for its tags through a disable — `DISABLED` is a status a caller can still
/// read, so the tag set stays with it — and a write to those tags still lands.
#[tokio::test]
async fn a_disabled_cdc_stream_keeps_its_tag_set() {
    let emulator = Cdc::start();
    let keyspace = "tags_disabled_ks";
    emulator.create_keyspace(keyspace).await;
    let held = table_holding_streams(&emulator, "disabled", keyspace, "tbl", 1).await;
    let stream_arn = held
        .first()
        .cloned()
        .expect("the created table holds one stream");

    assert_ok(
        "disabled",
        emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "tbl",
                    "cdcSpecification": {"status": "DISABLED"},
                }),
            )
            .await,
    );
    assert_tags_answered(&emulator, "disabled", &stream_arn).await;

    assert_ok(
        "disabled",
        emulator
            .keyspaces(
                "TagResource",
                json!({
                    "resourceArn": stream_arn,
                    "tags": [{"key": "added_after_disable", "value": "v"}],
                }),
            )
            .await,
    );
    let listed = assert_ok(
        "disabled",
        emulator
            .keyspaces("ListTagsForResource", json!({"resourceArn": stream_arn}))
            .await,
    );
    let mut keys: Vec<String> = listed["tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|tag: &Value| tag["key"].as_str().map(str::to_string))
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["added_after_disable".to_string(), STREAM_TAG_KEY.to_string()],
        "case \"disabled\": a disabled stream's tag set takes a write, got {listed}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! The tags an `UpdateTable` `cdcSpecification` carries, end to end over the wire.
//!
//! The rule under test: the tags a `cdcSpecification` carries are applied to the stream
//! the table has once the call is through — merged onto the tag set that stream already
//! holds, under the same per-resource tag rules as any other tag write — whatever status
//! the specification asks for. A status asking for a capturing stream keeps the table's
//! live stream (or mints a fresh one); a status asking for none STOPS that stream, which
//! stays the table's stream, so the tags still belong to it. Only a table left with no
//! stream at all gives the tags no resource to belong to.
//!
//! The rows span what the rule generalises over: the whole modelled `CdcStatus` value set
//! (the service's own generated catalog `ops::types::CDC_STATUS_VALUES`, so the rows
//! cannot drift from the model), `propagateTags` omitted and both of its modelled values
//! (`ops::types::CDC_PROPAGATE_TAGS_VALUES`), and the shape of the tag list itself — a key
//! the stream does not hold yet, a key it already holds (an update of its value, not a
//! second tag), more than one tag at once, and the member omitted.
//!
//! Everything is asserted through the HTTP surface a real client uses:
//! `ListTagsForResource` for the stream's own ARN, `GetTable` for the stream the table
//! reports, and `GetStream` on the KeyspacesStreams surface for the stream itself.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::{
    CDC_PROPAGATE_TAGS_VALUES, CDC_STATUS_VALUES,
};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::cdc_streams::{reported_cdc, Cdc};
use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, table_arn,
};

/// The tag every table under test is created with, so a propagated tag is recognisable.
const TABLE_TAG_KEY: &str = "table_tag";
/// The tag the stream under test is created with: the base a later specification merges
/// onto.
const STREAM_TAG_KEY: &str = "stream_tag";
/// The value that base tag carries when the stream is created.
const STREAM_TAG_CREATED: &str = "created";

/// How long a case waits before expecting both the table's and its stream's transient
/// statuses to have settled: the longer of the two spans this service stores, plus a
/// margin for a loaded test machine.
fn settled() -> Duration {
    settle_wait().max(keyspaces::CDC_STATUS_TRANSITION_DELAY + Duration::from_millis(250))
}

/// One tag-list shape a specification can carry, exercised against every status.
#[derive(Debug, Clone, Copy)]
struct Shape {
    name: &'static str,
    /// `propagateTags`, or `None` for the member omitted.
    propagate: Option<&'static str>,
    /// The `tags` the specification carries, or `None` for the member omitted.
    tags: Option<&'static [(&'static str, &'static str)]>,
}

/// The tag-list shapes every status is exercised with.
const SHAPES: [Shape; 3] = [
    // A key the stream does not hold yet, with `propagateTags` omitted.
    Shape {
        name: "one_new_key",
        propagate: None,
        tags: Some(&[("applied_tag", "v")]),
    },
    // A key the stream already holds (an update of its value, not a second tag) beside a
    // second new one, with the table's own tags inherited on top.
    Shape {
        name: "updates_and_adds",
        propagate: Some(keyspaces::CDC_PROPAGATE_TAGS_TABLE),
        tags: Some(&[(STREAM_TAG_KEY, "updated"), ("second_tag", "2")]),
    },
    // No tags at all: the stream keeps exactly the set it held.
    Shape {
        name: "no_tags",
        propagate: Some("NONE"),
        tags: None,
    },
];

/// The tag set `ListTagsForResource` reports for one resource, as a map.
pub(crate) async fn listed_tags(emulator: &Cdc, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
    let listed = assert_ok(
        case,
        emulator
            .keyspaces("ListTagsForResource", json!({"resourceArn": resource_arn}))
            .await,
    );
    listed["tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|tag: &Value| {
            Some((
                tag["key"].as_str()?.to_string(),
                tag["value"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// Create `table` in `keyspace` with a tag of its own and a CDC stream carrying one tag,
/// wait for both to settle, and answer with the stream's ARN.
async fn table_with_a_tagged_stream(
    emulator: &Cdc,
    case: &str,
    keyspace: &str,
    table: &str,
) -> String {
    assert_ok(
        case,
        emulator
            .keyspaces(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": minimal_schema("pk"),
                    "tags": [{"key": TABLE_TAG_KEY, "value": case}],
                    "cdcSpecification": {
                        "status": "ENABLED",
                        "tags": [{"key": STREAM_TAG_KEY, "value": STREAM_TAG_CREATED}],
                    },
                }),
            )
            .await,
    );
    tokio::time::sleep(settled()).await;
    let reported = emulator.get_table(case, keyspace, table).await;
    reported_cdc(&reported).2.unwrap_or_else(|| {
        panic!("case {case:?}: the created table must report a stream, got {reported}")
    })
}

/// RULE: a `cdcSpecification`'s tags are applied to the stream the table has once the
/// call is through, merged onto the tags that stream already holds, whatever status the
/// specification asks for.
#[tokio::test]
async fn an_update_applies_its_cdc_specifications_tags_whatever_status_it_asks_for() {
    // Every modelled `CdcStatus` value against every tag-list shape.
    let rows: Vec<(String, &'static str, Shape)> = CDC_STATUS_VALUES
        .iter()
        .flat_map(|status| {
            SHAPES.iter().map(move |shape| {
                (
                    format!("{}_{}", status.to_lowercase(), shape.name),
                    *status,
                    *shape,
                )
            })
        })
        .collect();
    // Both modelled `propagateTags` values must be among the rows, so the catalog cannot
    // drift away from the shapes above.
    for propagate in CDC_PROPAGATE_TAGS_VALUES {
        assert!(
            rows.iter()
                .any(|(_, _, shape)| shape.propagate == Some(propagate)),
            "the rows must cover propagateTags {propagate:?}"
        );
    }

    let emulator = Cdc::start();
    let keyspace = "cdc_update_tags_ks";
    emulator.create_keyspace(keyspace).await;

    for (case, status, shape) in &rows {
        let case = case.as_str();
        let table = case;
        let stream_arn = table_with_a_tagged_stream(&emulator, case, keyspace, table).await;

        let before = listed_tags(&emulator, case, &stream_arn).await;
        assert_eq!(
            before,
            BTreeMap::from([(
                STREAM_TAG_KEY.to_string(),
                STREAM_TAG_CREATED.to_string()
            )]),
            "case {case:?}: the created stream must hold its own tag, got {before:?}"
        );

        let mut specification = json!({"status": status});
        if let Some(propagate) = shape.propagate {
            specification["propagateTags"] = json!(propagate);
        }
        if let Some(tags) = shape.tags {
            specification["tags"] = Value::Array(
                tags.iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect(),
            );
        }
        assert_ok(
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
        tokio::time::sleep(settled()).await;

        // The stream the tags belong to is the one the table still reports as its latest:
        // a status asking for a capturing stream re-asserts the live one, and one asking
        // for none stops it without taking it away.
        let reported = emulator.get_table(case, keyspace, table).await;
        let (reported_status, _, latest) = reported_cdc(&reported);
        let asks_for_a_stream = matches!(
            *status,
            keyspaces::CDC_STATUS_ENABLED | keyspaces::CDC_STATUS_ENABLING
        );
        let expected_status = match asks_for_a_stream {
            true => keyspaces::CDC_STATUS_ENABLED,
            false => keyspaces::CDC_STATUS_DISABLED,
        };
        assert_eq!(
            (reported_status.as_deref(), latest.as_deref()),
            (Some(expected_status), Some(stream_arn.as_str())),
            "case {case:?}: expected the table to report {expected_status} on stream \
             {stream_arn}, got {reported}"
        );
        assert_ok(
            case,
            emulator
                .streams("GetStream", json!({"streamArn": stream_arn.clone()}))
                .await,
        );

        // What the specification applies: the table's own tags when it asks to inherit
        // them, then the tags it carries, onto the set the stream already held.
        let mut expected = before.clone();
        if shape.propagate == Some(keyspaces::CDC_PROPAGATE_TAGS_TABLE) {
            expected.insert(TABLE_TAG_KEY.to_string(), case.to_string());
        }
        for (key, value) in shape.tags.unwrap_or_default() {
            expected.insert((*key).to_string(), (*value).to_string());
        }
        let got = listed_tags(&emulator, case, &stream_arn).await;
        assert_eq!(
            got, expected,
            "case {case:?}: expected the stream's tags to be {expected:?}, got {got:?}"
        );

        // The TABLE's own tags are untouched by what its stream carries.
        let table_tags = listed_tags(&emulator, case, &table_arn(keyspace, table)).await;
        assert_eq!(
            table_tags,
            BTreeMap::from([(TABLE_TAG_KEY.to_string(), case.to_string())]),
            "case {case:?}: the table keeps exactly its own tags, got {table_tags:?}"
        );
    }
}

/// The same rule where the status asks for the state the table is ALREADY in: a second
/// disabling specification still applies its tags to the stream the table keeps, and a
/// re-enable mints a stream that starts from the tags its own specification carries.
#[tokio::test]
async fn a_disabling_specification_tags_the_stream_the_table_keeps() {
    let emulator = Cdc::start();
    let keyspace = "cdc_repeat_disable_ks";
    let table = "tbl";
    emulator.create_keyspace(keyspace).await;
    let first = table_with_a_tagged_stream(&emulator, "repeat_disable", keyspace, table).await;

    // Two disabling updates in a row: the first stops the stream, the second asks for the
    // state the table is already in. Each one's tags land on the stream the table keeps.
    for (case, value) in [("first_disable", "one"), ("second_disable", "two")] {
        assert_ok(
            case,
            emulator
                .keyspaces(
                    "UpdateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "cdcSpecification": {
                            "status": "DISABLED",
                            "tags": [{"key": case, "value": value}],
                        },
                    }),
                )
                .await,
        );
        tokio::time::sleep(settled()).await;
        let got = listed_tags(&emulator, case, &first).await;
        assert_eq!(
            got.get(case).map(String::as_str),
            Some(value),
            "case {case:?}: expected the stopped stream to hold {case}={value}, got {got:?}"
        );
        assert_eq!(
            got.get(STREAM_TAG_KEY).map(String::as_str),
            Some(STREAM_TAG_CREATED),
            "case {case:?}: expected the stopped stream to keep the tag it was created \
             with, got {got:?}"
        );
    }

    // A re-enable mints a NEW stream, so its tag set starts from what its own
    // specification carries — never from the stopped stream's.
    assert_ok(
        "re_enable",
        emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "cdcSpecification": {
                        "status": "ENABLED",
                        "tags": [{"key": "re_enabled", "value": "v"}],
                    },
                }),
            )
            .await,
    );
    tokio::time::sleep(settled()).await;
    let reported = emulator.get_table("re_enable", keyspace, table).await;
    let second = reported_cdc(&reported)
        .2
        .unwrap_or_else(|| panic!("case \"re_enable\": a re-enabled table reports a stream"));
    assert_ne!(
        second, first,
        "case \"re_enable\": a re-enabled table must get a new stream, got {second}"
    );
    let got = listed_tags(&emulator, "re_enable", &second).await;
    assert_eq!(
        got,
        BTreeMap::from([("re_enabled".to_string(), "v".to_string())]),
        "case \"re_enable\": expected the new stream to hold only its own tags, got {got:?}"
    );
    // And the stopped stream still answers with everything written to it.
    let kept = listed_tags(&emulator, "re_enable", &first).await;
    assert_eq!(
        kept,
        BTreeMap::from([
            (STREAM_TAG_KEY.to_string(), STREAM_TAG_CREATED.to_string()),
            ("first_disable".to_string(), "one".to_string()),
            ("second_disable".to_string(), "two".to_string()),
        ]),
        "case \"re_enable\": the stopped stream keeps its own tag set, got {kept:?}"
    );
}

/// A table with no stream at all gives a specification's tags no resource to belong to:
/// the call is accepted, and nothing is invented to carry them.
#[tokio::test]
async fn a_specification_with_no_stream_to_tag_stores_nothing() {
    let emulator = Cdc::start();
    let keyspace = "cdc_no_stream_ks";
    let table = "tbl";
    emulator.create_keyspace(keyspace).await;
    assert_ok(
        "no_stream",
        emulator
            .keyspaces(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    tokio::time::sleep(settled()).await;

    assert_ok(
        "no_stream",
        emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "cdcSpecification": {
                        "status": "DISABLED",
                        "tags": [{"key": "nowhere_to_land", "value": "v"}],
                    },
                }),
            )
            .await,
    );
    tokio::time::sleep(settled()).await;
    let reported = emulator.get_table("no_stream", keyspace, table).await;
    assert_eq!(
        reported_cdc(&reported),
        (None, None, None),
        "case \"no_stream\": a table nothing enabled CDC for reports none of it, got {reported}"
    );
    let listed = emulator
        .listed_stream_arns("no_stream", keyspace, table)
        .await;
    assert!(
        listed.is_empty(),
        "case \"no_stream\": no stream may be created to carry the tags, got {listed:?}"
    );
}

/// The per-resource tag limit bounds the set a disabling specification writes too, counted
/// against the tags the stream ALREADY holds: the published limit
/// ([`keyspaces::MAX_TAGS_PER_RESOURCE`]) is accepted, one past it is refused as an invalid
/// request, and a refused update leaves both the stream and its tags exactly as they were.
#[tokio::test]
async fn the_quota_counts_a_disabling_specifications_tags_against_the_streams_own() {
    let quota = keyspaces::MAX_TAGS_PER_RESOURCE;
    /// `count` distinct tags, as a `cdcSpecification` carries them.
    fn distinct(count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({"key": format!("quota_key{index:03}"), "value": "v"}))
                .collect(),
        )
    }

    // The stream starts with one tag of its own, so a specification carrying `quota - 1`
    // new keys lands exactly on the limit and one more key is one past it.
    for (case, supplied, accepted) in [
        ("at_the_limit", quota - 1, true),
        ("one_past_the_limit", quota, false),
    ] {
        let emulator = Cdc::start();
        let keyspace = "cdc_quota_ks";
        let table = "tbl";
        emulator.create_keyspace(keyspace).await;
        let stream_arn = table_with_a_tagged_stream(&emulator, case, keyspace, table).await;

        let request = json!({
            "keyspaceName": keyspace,
            "tableName": table,
            "cdcSpecification": {"status": "DISABLED", "tags": distinct(supplied)},
        });
        let got = emulator.keyspaces("UpdateTable", request).await;
        match accepted {
            true => {
                assert_ok(case, got);
                tokio::time::sleep(settled()).await;
                let tags = listed_tags(&emulator, case, &stream_arn).await;
                assert_eq!(
                    tags.len(),
                    quota,
                    "case {case:?}: expected the stream to hold {quota} tags, got {tags:?}"
                );
            }
            false => {
                assert_failure(case, got, 400, "ValidationException");
                // Refused before anything was written: the stream is untouched, so it is
                // still the capturing stream it was and still holds just its own tag.
                let tags = listed_tags(&emulator, case, &stream_arn).await;
                assert_eq!(
                    tags,
                    BTreeMap::from([(
                        STREAM_TAG_KEY.to_string(),
                        STREAM_TAG_CREATED.to_string()
                    )]),
                    "case {case:?}: a refused update must leave the tag set as it was, got \
                     {tags:?}"
                );
                let reported = emulator.get_table(case, keyspace, table).await;
                assert_eq!(
                    reported_cdc(&reported).0.as_deref(),
                    Some(keyspaces::CDC_STATUS_ENABLED),
                    "case {case:?}: a refused update must leave the stream capturing, got \
                     {reported}"
                );
            }
        }
    }
}

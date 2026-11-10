// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateTable` `cdcSpecification` that names NO `viewType` asks for, end to end
//! over the wire.
//!
//! The rule under test: an optional member the request omits carries no instruction, so a
//! setting fixed at creation time is compared only against a value the request actually
//! supplied — the member's documented default is what a resource being MINTED gets, never
//! a change demanded of one that already exists. For a table's CDC stream that means a
//! specification carrying no `viewType` re-asserts the live stream exactly as it stands
//! (and applies its tags to it), one naming the view type that stream already publishes
//! is the same no-change re-assert, and only one naming a DIFFERENT modelled view type is
//! refused as the change the stream's selection cannot take.
//!
//! The rows span what the rule generalises over: every value of the modelled `ViewType`
//! set as the selection the live stream publishes (the service's own generated catalog
//! `ops::types::VIEW_TYPE_VALUES`, so the rows cannot drift from the model — the default
//! `NEW_AND_OLD_IMAGES` is one row among them), every `CdcStatus` value that asks for a
//! capturing stream, the shape of the specification itself (`viewType` omitted with and
//! without tags, and named as the stored value), and, for the refusal, every other view
//! type the model declares.
//!
//! Every row is driven phase by phase — all of them updated, then all of them read — so
//! the transient statuses of the whole table settle once per phase rather than once per
//! row. Everything is asserted through the HTTP surface a real client uses:
//! `UpdateTable`, `GetTable` for the stream and selection the table reports, and
//! `ListTagsForResource` for the stream's own ARN.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::VIEW_TYPE_VALUES;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::cdc_specification_tags_on_update::listed_tags;
use super::cdc_streams::{reported_cdc, Cdc};
use super::keyspace_table_lifecycle::{assert_failure, assert_ok, minimal_schema, settle_wait};

/// The `CdcStatus` values that ask the table for a capturing stream, which are the only
/// ones a selection the request names could ever be applied to.
const CAPTURING_STATUSES: [&str; 2] = [
    keyspaces::CDC_STATUS_ENABLED,
    keyspaces::CDC_STATUS_ENABLING,
];

/// The tag key every shape that carries tags writes to the stream.
const APPLIED_TAG_KEY: &str = "env";

/// One `cdcSpecification` shape that asks for NO change of the stream's selection.
#[derive(Debug, Clone, Copy)]
struct Shape {
    name: &'static str,
    /// Whether the specification NAMES the view type the stream already publishes, rather
    /// than omitting the member altogether.
    names_the_stored_view_type: bool,
    /// Whether it carries a tag, which is then the only thing it changes.
    carries_a_tag: bool,
}

/// The shapes every row is exercised with.
const SHAPES: [Shape; 3] = [
    // The bare idempotent re-assert: nothing but the required status.
    Shape {
        name: "bare",
        names_the_stored_view_type: false,
        carries_a_tag: false,
    },
    // Tags for the stream, with no view type named — the only way `UpdateTable` applies
    // tags to a stream that already exists.
    Shape {
        name: "tags_only",
        names_the_stored_view_type: false,
        carries_a_tag: true,
    },
    // The stored view type named explicitly: the same no-change re-assert.
    Shape {
        name: "same_view_type",
        names_the_stored_view_type: true,
        carries_a_tag: false,
    },
];

/// One table under test: the selection its live stream publishes, the status its updates
/// ask for, and the stream they must all keep.
#[derive(Debug)]
struct Row {
    /// The table's name, which is also its case name.
    name: String,
    view_type: &'static str,
    status: &'static str,
    stream_arn: String,
}

/// How long a phase waits before expecting every table's and stream's transient status to
/// have settled: the longer of the two spans this service stores, plus a margin for a
/// loaded test machine.
fn settled() -> Duration {
    settle_wait().max(keyspaces::CDC_STATUS_TRANSITION_DELAY + Duration::from_millis(250))
}

/// What the CDC members of a `GetTable` report must be for a row whose stream is
/// unchanged: capturing, publishing the selection it was created with, and still the
/// table's latest stream.
fn unchanged(row: &Row) -> (Option<String>, Option<String>, Option<String>) {
    (
        Some(keyspaces::CDC_STATUS_ENABLED.to_string()),
        Some(row.view_type.to_string()),
        Some(row.stream_arn.clone()),
    )
}

/// RULE: a `cdcSpecification` that names no `viewType` asks for no selection, so it
/// re-asserts whatever the table's live stream publishes instead of being read as a
/// request for the member's default.
#[tokio::test]
async fn an_update_naming_no_view_type_re_asserts_the_stream_whatever_it_publishes() {
    let emulator = Cdc::start();
    let keyspace = "cdc_omitted_view_ks";
    emulator.create_keyspace(keyspace).await;

    // One table per (view type, capturing status): every selection a live stream can
    // publish, against every status that asks for one.
    let mut rows: Vec<Row> = Vec::new();
    for view_type in VIEW_TYPE_VALUES {
        for status in CAPTURING_STATUSES {
            let name = format!("{}_{}", view_type.to_lowercase(), status.to_lowercase());
            assert_ok(
                &name,
                emulator
                    .keyspaces(
                        "CreateTable",
                        json!({
                            "keyspaceName": keyspace,
                            "tableName": name,
                            "schemaDefinition": minimal_schema("pk"),
                            "cdcSpecification": {"status": "ENABLED", "viewType": view_type},
                        }),
                    )
                    .await,
            );
            rows.push(Row {
                name,
                view_type,
                status,
                stream_arn: String::new(),
            });
        }
    }
    tokio::time::sleep(settled()).await;
    for row in rows.iter_mut() {
        let reported = emulator.get_table(&row.name, keyspace, &row.name).await;
        let (_, published, stream_arn) = reported_cdc(&reported);
        assert_eq!(
            published.as_deref(),
            Some(row.view_type),
            "case {:?}: the created stream must publish the view type the create named, \
             got {reported}",
            row.name
        );
        row.stream_arn = stream_arn.unwrap_or_else(|| {
            panic!(
                "case {:?}: a table with CDC enabled reports a stream: {reported}",
                row.name
            )
        });
    }

    for shape in SHAPES {
        for row in &rows {
            let case = format!("{}_{}", row.name, shape.name);
            let mut specification = json!({"status": row.status});
            if shape.names_the_stored_view_type {
                specification["viewType"] = json!(row.view_type);
            }
            if shape.carries_a_tag {
                specification["tags"] = json!([{"key": APPLIED_TAG_KEY, "value": case}]);
            }
            assert_ok(
                &case,
                emulator
                    .keyspaces(
                        "UpdateTable",
                        json!({
                            "keyspaceName": keyspace,
                            "tableName": row.name,
                            "cdcSpecification": specification,
                        }),
                    )
                    .await,
            );
        }
        tokio::time::sleep(settled()).await;
        for row in &rows {
            let case = format!("{}_{}", row.name, shape.name);
            let case = case.as_str();
            let reported = emulator.get_table(case, keyspace, &row.name).await;
            assert_eq!(
                reported_cdc(&reported),
                unchanged(row),
                "case {case:?}: expected the table to keep stream {} publishing {}, got \
                 {reported}",
                row.stream_arn,
                row.view_type
            );
            if shape.carries_a_tag {
                let tags = listed_tags(&emulator, case, &row.stream_arn).await;
                assert_eq!(
                    tags.get(APPLIED_TAG_KEY).map(String::as_str),
                    Some(case),
                    "case {case:?}: expected the stream to carry the tag the \
                     specification applied, got {tags:?}"
                );
            }
        }
    }

    // Only a view type the request NAMES, and that differs from the one the stream
    // publishes, is the change a stream's selection cannot take. A refused update writes
    // nothing, so these cases need no settling phase of their own.
    for row in &rows {
        for other in VIEW_TYPE_VALUES
            .iter()
            .filter(|other| **other != row.view_type)
        {
            let case = format!("{}_to_{}", row.name, other.to_lowercase());
            let case = case.as_str();
            let got = emulator
                .keyspaces(
                    "UpdateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": row.name,
                        "cdcSpecification": {"status": row.status, "viewType": other},
                    }),
                )
                .await;
            let body: Value = got.1.clone();
            assert_failure(case, got, 400, "ValidationException");
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&format!("'{}'", row.view_type))
                    && message.contains(&format!("'{other}'")),
                "case {case:?}: the refusal must name the stream's selection {} and the \
                 view type the request carried {other}, got {message:?}",
                row.view_type
            );
            let reported = emulator.get_table(case, keyspace, &row.name).await;
            assert_eq!(
                reported_cdc(&reported),
                unchanged(row),
                "case {case:?}: a refused update must leave the stream exactly as it was, \
                 got {reported}"
            );
        }
    }
}

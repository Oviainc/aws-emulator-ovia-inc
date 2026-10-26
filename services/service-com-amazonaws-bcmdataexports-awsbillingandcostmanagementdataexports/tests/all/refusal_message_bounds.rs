// SPDX-License-Identifier: CC0-1.0

//! The modelled bound of a refusal, on EVERY refusal this service words itself, over the
//! awsJson1_1 wire.
//!
//! RULE: every string a refusal of this service carries is a modelled `GenericString`,
//! `@length(0, 1024)` (`smithy_query(query="shape",shape="GenericString")`): the `Message`
//! of `ValidationException` and of `AccessDeniedException`, and the `Name` and `Message` of
//! each `ValidationExceptionField`. So no refusal exceeds that bound in any of them,
//! whatever the request it answers — including a request whose own members admit MORE
//! characters than the refusal reporting them does (`TableName` admits 1024, a table
//! property value 16384, a `QueryStatement` 36000 and an `Arn` 2048:
//! `smithy_query(query="shape",shape="TableName,TableProperties,DataQuery,Arn")`).
//!
//! `tests/all/refusal_message_bound.rs` asserts the rule where the refusal is built from a
//! caller-sized LIST of values. This suite asserts it across the refusal CLASSES this
//! service words — one row per class, each driven end to end — because the bound belongs
//! to the member every refusal is written into and not to one sentence: the rows differ
//! along which operation refuses (`CreateExport`, `UpdateExport`, `TagResource`,
//! `UntagResource`, `GetTable`, `ListExports`), which input member is at fault, and which
//! outsized caller-supplied value the refusal has to report — a value at the published
//! ceiling of a member that admits more than a message does, and one past that ceiling
//! where the boundary validator answers instead.

use serde_json::{json, Value};

use crate::export_lifecycle::{cases, error_code, Client};

/// The `@length` ceiling of a `GenericString`, which every string member of every error
/// shape of this service targets (smithy_query shape `GenericString`: `@length(min: 0,
/// max: 1024)`). Stated from the MODEL, so the expectation is not read from the code under
/// test.
const MESSAGE_MAX_CHARS: usize = 1024;

/// The `@length` ceiling of `GetTable`'s `TableName` (smithy_query shape `TableName`:
/// `@length(min: 1, max: 1024)`) — as many characters as the message reporting it admits
/// in total.
const MAX_TABLE_NAME_CHARS: usize = 1024;

/// The `@length` ceiling of a table-property value (smithy_query shape `TableProperties`:
/// `@length(min: 0, max: 16384)`) — sixteen times what a message admits.
const MAX_TABLE_PROPERTY_CHARS: usize = 16_384;

/// The `@length` ceiling of `Export.DataQuery.QueryStatement` (smithy_query shape
/// `DataQuery`, member `QueryStatement`: `@length(min: 1, max: 36000)`).
const MAX_QUERY_STATEMENT_CHARS: usize = 36_000;

/// A table the catalog publishes, named by the first lifecycle case's own query.
const PUBLISHED_TABLE: &str = "COST_AND_USAGE_REPORT";

/// One refused request: the operation, the body, and what the row is about.
struct Row {
    label: String,
    operation: &'static str,
    body: Value,
}

/// The `Export` of the first lifecycle case, which is a definition the service accepts.
fn valid_export() -> Value {
    cases().remove(0).create_request()["Export"].clone()
}

/// [`valid_export`] with `member` of its `Export` replaced.
fn export_with(member: &str, value: Value) -> Value {
    let mut export = valid_export();
    export[member] = value;
    export
}

/// [`valid_export`] with its `QueryStatement` replaced, keeping everything else.
fn export_querying(statement: &str) -> Value {
    let mut export = valid_export();
    export["DataQuery"] = json!({ "QueryStatement": statement });
    export
}

/// [`valid_export`] with one member of its `S3Destination` replaced.
fn export_destined(member: &str, value: Value) -> Value {
    let mut export = valid_export();
    export["DestinationConfigurations"]["S3Destination"][member] = value;
    export
}

/// The rows: one per refusal class this service words, each carrying an outsized or
/// otherwise offending caller-supplied value.
fn rows() -> Vec<Row> {
    let outsized_name = "T".repeat(MAX_TABLE_NAME_CHARS);
    let outsized_property = "v".repeat(MAX_TABLE_PROPERTY_CHARS);
    // A statement at the published ceiling that is still not a query this service reads.
    let outsized_statement = format!("SELECT {}", "x".repeat(MAX_QUERY_STATEMENT_CHARS - 7));
    let outsized_key = "k".repeat(128);

    vec![
        // A `TableName` at its own published ceiling, which is the whole of what a message
        // admits, so the refusal cannot both echo it whole and stay inside the bound.
        Row {
            label: "a TableName at its published ceiling".to_string(),
            operation: "GetTable",
            body: json!({ "TableName": outsized_name }),
        },
        // A table property key no table publishes, at the same ceiling.
        Row {
            label: "an unpublished table property at the value ceiling".to_string(),
            operation: "GetTable",
            body: json!({
                "TableName": PUBLISHED_TABLE,
                "TableProperties": { "NOT_A_PROPERTY": outsized_property.clone() },
            }),
        },
        // A statement at the published `QueryStatement` ceiling that this service cannot
        // read as a data query.
        Row {
            label: "a QueryStatement at its published ceiling".to_string(),
            operation: "CreateExport",
            body: json!({ "Export": export_querying(&outsized_statement) }),
        },
        // A query naming a table outside the catalog.
        Row {
            label: "a query naming a table outside the catalog".to_string(),
            operation: "CreateExport",
            body: json!({
                "Export": export_querying(
                    "SELECT identity_line_item_id FROM NOT_A_PUBLISHED_TABLE",
                ),
            }),
        },
        // A query naming a column the queried table does not publish.
        Row {
            label: "a query naming a column the table does not publish".to_string(),
            operation: "CreateExport",
            body: json!({
                "Export": export_querying(&format!(
                    "SELECT not_a_published_column FROM {PUBLISHED_TABLE}",
                )),
            }),
        },
        // An `S3Region` that names no Region.
        Row {
            label: "an S3Region that names no Region".to_string(),
            operation: "CreateExport",
            body: json!({
                "Export": export_destined("S3Region", json!("not-a-region-at-all-1")),
            }),
        },
        // A `ResourceTags` list naming one key twice, at the key's published ceiling.
        Row {
            label: "a duplicated tag key at the key ceiling".to_string(),
            operation: "CreateExport",
            body: json!({
                "Export": export_with("Name", json!("bounded_duplicate_tag_key")),
                "ResourceTags": [
                    { "Key": outsized_key.clone(), "Value": "1" },
                    { "Key": outsized_key, "Value": "2" },
                ],
            }),
        },
        // A `NextToken` this service never issued.
        Row {
            label: "a NextToken no listing issued".to_string(),
            operation: "ListExports",
            body: json!({ "NextToken": "n".repeat(512) }),
        },
        // A tagging ARN of another account, which is refused before any lookup.
        Row {
            label: "a ResourceArn of another account".to_string(),
            operation: "TagResource",
            body: json!({
                "ResourceArn": "arn:aws:bcm-data-exports:us-east-1:111122223333:export/other",
                "ResourceTags": [{ "Key": "team", "Value": "billing" }],
            }),
        },
        // The same, on the operation that removes tags.
        Row {
            label: "a ResourceArn of another account on UntagResource".to_string(),
            operation: "UntagResource",
            body: json!({
                "ResourceArn": "arn:aws:bcm-data-exports:us-east-1:111122223333:export/other",
                "ResourceTagKeys": ["team"],
            }),
        },
    ]
}

/// Assert every string `body` carries fits the modelled bound, and that the refusal still
/// says something: an empty message is inside the bound but tells the caller nothing.
fn messages_are_bounded(case: &str, body: &Value) {
    let message = body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a Message: {body}"));
    assert!(
        !message.is_empty() && message.chars().count() <= MESSAGE_MAX_CHARS,
        "case {case:?}: Message must satisfy the modelled @length(0, {MESSAGE_MAX_CHARS}), got {} characters: {message:?}",
        message.chars().count()
    );
    if let Some(fields) = body["Fields"].as_array() {
        for field in fields {
            for member in ["Name", "Message"] {
                let text = field[member]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a field without {member}: {field}"));
                assert!(
                    !text.is_empty() && text.chars().count() <= MESSAGE_MAX_CHARS,
                    "case {case:?}: Fields[].{member} must satisfy the modelled @length(0, {MESSAGE_MAX_CHARS}), got {} characters: {text:?}",
                    text.chars().count()
                );
            }
        }
    }
}

#[tokio::test]
async fn every_refusal_this_service_words_stays_inside_the_modelled_bound() {
    let client = Client::new();

    for row in rows() {
        let case = format!("{} via {}", row.label, row.operation);
        let (status, body) = client.call(row.operation, &row.body).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        messages_are_bounded(&case, &body);
        // A refusal of a named input member reports that member, which is what the caller
        // acts on: the bound is kept by shortening what the sentence ECHOES, never by
        // dropping the member it is about.
        let names_a_member = body["Fields"]
            .as_array()
            .is_some_and(|fields| !fields.is_empty());
        assert!(
            names_a_member,
            "case {case:?}: the refusal must name the member at fault, got {body}"
        );
    }
}

/// The two refusals a request can only reach once something is stored: a second export
/// under a name in use, and an update that would rename one. Both echo caller-supplied
/// values, and both are bounded like every other refusal.
#[tokio::test]
async fn a_stored_export_refuses_a_duplicate_name_and_a_rename_within_the_bound() {
    let client = Client::new();
    let mut case = cases().remove(0);
    case.set_name("bounded_refusal_classes");
    let created = client.ok("CreateExport", &case.create_request()).await;
    let arn = created["ExportArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateExport returned no ExportArn: {created}"))
        .to_string();

    let (status, body) = client.call("CreateExport", &case.create_request()).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a name already in use must be refused, got {status} with {body}"
    );
    messages_are_bounded("a name already in use", &body);

    let mut renamed = case.update_export();
    renamed["Name"] = json!("bounded_refusal_classes_renamed");
    let (status, body) = client
        .call(
            "UpdateExport",
            &json!({ "ExportArn": arn, "Export": renamed }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "an update that renames the export must be refused, got {status} with {body}"
    );
    messages_are_bounded("an update that renames the export", &body);
}

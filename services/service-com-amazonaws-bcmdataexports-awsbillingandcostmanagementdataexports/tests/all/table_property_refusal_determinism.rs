// SPDX-License-Identifier: CC0-1.0

//! WHICH invalid entry a table-property refusal names when a request's property map
//! carries more than one, and WHICH sentence it earns.
//!
//! The rule under test: a refusal is decided by the content of the request alone, so a
//! property map carrying several invalid entries is refused for the entry whose KEY sorts
//! first and earns that entry's own sentence — the same refusal for every byte-identical
//! request, on every operation that carries a property map. The modelled map (`@map
//! TableProperties`, submitted at `GetTableRequest$TableProperties` and inside
//! `DataQuery$TableConfigurations`) deserializes into a hash map whose iteration order no
//! request controls and which is not even stable between two maps carrying the same
//! entries, so a refusal read off that order would leave the offender the caller has to
//! act on — and whether it is the unknown-property sentence or the invalid-value one —
//! determined by nothing the caller sent.
//!
//! No expected sentence is spelled here. Each row's expectation is the refusal the SAME
//! operation answers the row's expected offending entry ALONE with, so the multi-offender
//! refusal and the single-offender refusal of the entry the rule picks cannot drift apart,
//! and the assertion stays about the CHOICE rather than about any one wording.
//!
//! The rows are generated from the service's own published catalog, over every table it
//! publishes, and differ along what the rule generalises over: two unknown keys; eight of
//! them, written in descending order; an unknown key beside a value outside an enumerated
//! property's published values; an unknown key placed by construction just BEFORE and just
//! AFTER a published property name, so both sentences are the determined answer for some
//! row; two enumerated properties both carrying an unpublished value; and a valid entry
//! beside two invalid ones. Each row goes through all three operations that accept a
//! property map (`GetTable`, `CreateExport`, `UpdateExport`). The last test pins the same
//! determinism for a SERVED request, whose reported configuration and schema must equally
//! not depend on the order the entries were iterated in.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    TableSpec, TABLE_CATALOG,
};

use crate::data_query_resolution::export_body;
use crate::export_lifecycle::{error_code, Client};
use crate::table_property_refusal_field::enumerated_properties;

/// How many times each request is repeated. The caller's map is built afresh per request,
/// with its own hash seed, so a refusal that followed hash order would differ between two
/// of these repeats rather than within one.
const REPEATS: usize = 8;

/// The stem of a property key no published table names, so every table refuses it.
const UNPUBLISHED_PROPERTY: &str = "NOT_A_TABLE_PROPERTY";

/// A value no enumerated property of the catalog publishes.
const VALUE_OUTSIDE_EVERY_LIST: &str = "NOT_A_PUBLISHED_VALUE";

/// A value for a free-form property, whose value the caller writes rather than picks.
const FREE_FORM_VALUE: &str = "ANY_VALUE_THE_CALLER_WRITES";

/// The statement the baseline export an update replaces is created with: a published table
/// with the properties it publishes left to their defaults.
const BASELINE_STATEMENT: &str = "SELECT line_item_usage_account_id FROM COST_AND_USAGE_REPORT";

/// The `TableConfigurations` the baseline definition restates, which an update owes.
fn baseline_configurations() -> Value {
    json!({ "COST_AND_USAGE_REPORT": {} })
}

/// One row: a property map, and the published table it is submitted for.
#[derive(Debug, Clone)]
struct Row {
    label: String,
    table: &'static str,
    /// The submitted entries, in the order this row writes them — which is the order
    /// neither the wire nor the deserialized map preserves, and therefore the order the
    /// answer must not depend on.
    entries: Vec<(String, String)>,
}

/// The catalog entry called `name`.
fn spec_of(name: &str) -> &'static TableSpec {
    TABLE_CATALOG
        .iter()
        .find(|candidate| candidate.name == name)
        .unwrap_or_else(|| panic!("{name} is not a published catalog table"))
}

/// `entries` as the JSON map a request carries them as.
fn map_of(entries: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (name, value) in entries {
        map.insert(name.clone(), json!(value));
    }
    Value::Object(map)
}

/// Whether `table` refuses the entry `(name, value)`: a key the table does not publish, or
/// a value outside an enumerated property's published values.
fn offends(table: &'static TableSpec, (name, value): &(String, String)) -> bool {
    table
        .property(name)
        .is_none_or(|property| !property.accepts(value))
}

/// The entry the rule says the refusal names: among the entries `table` refuses, the one
/// whose KEY sorts first. Computed here from the published catalog, so the expectation is
/// the rule rather than whatever the service happens to answer.
fn expected_offender(table: &'static TableSpec, entries: &[(String, String)]) -> (String, String) {
    entries
        .iter()
        .filter(|entry| offends(table, entry))
        .min_by(|left, right| left.0.cmp(&right.0))
        .cloned()
        .unwrap_or_else(|| panic!("{} refuses no entry of {entries:?}", table.name))
}

/// Which sentence the rule's chosen entry earns: the unknown-property one for a key the
/// table does not publish, the invalid-value one for a published key at an unpublished
/// value. Classified from the catalog, never from the answer's text.
fn sentence_kind(table: &'static TableSpec, offender: &(String, String)) -> &'static str {
    match table.property(&offender.0) {
        None => "the unknown-property sentence",
        Some(_) => "the invalid-value sentence",
    }
}

/// The rows, generated from the published catalog so every table it publishes is covered.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for table in TABLE_CATALOG {
        let unknown_a = format!("{UNPUBLISHED_PROPERTY}_A");
        let unknown_b = format!("{UNPUBLISHED_PROPERTY}_B");
        // Written with the later key first, so neither the written order nor its reverse
        // can be mistaken for the answer.
        rows.push(Row {
            label: format!("two keys {} does not publish", table.name),
            table: table.name,
            entries: vec![
                (unknown_b, "TRUE".to_string()),
                (unknown_a.clone(), "TRUE".to_string()),
            ],
        });
        rows.push(Row {
            label: format!(
                "eight keys {} does not publish, written in descending order",
                table.name
            ),
            table: table.name,
            entries: (0..8)
                .rev()
                .map(|index| (format!("{UNPUBLISHED_PROPERTY}_{index}"), "TRUE".to_string()))
                .collect(),
        });

        let enumerated = enumerated_properties(table);
        if let Some(property) = enumerated.first() {
            let name = property.name;
            rows.push(Row {
                label: format!(
                    "a key {} does not publish beside a value outside {name}'s published values",
                    table.name
                ),
                table: table.name,
                entries: vec![
                    (name.to_string(), VALUE_OUTSIDE_EVERY_LIST.to_string()),
                    (unknown_a.clone(), "TRUE".to_string()),
                ],
            });
            // A key EXTENDING a published name sorts just after it, so the published key's
            // invalid VALUE is the entry the rule picks …
            rows.push(Row {
                label: format!(
                    "{name} at an unpublished value beside an unknown key sorting after it, \
                     on {}",
                    table.name
                ),
                table: table.name,
                entries: vec![
                    (
                        format!("{name}_{UNPUBLISHED_PROPERTY}"),
                        "TRUE".to_string(),
                    ),
                    (name.to_string(), VALUE_OUTSIDE_EVERY_LIST.to_string()),
                ],
            });
            // … and a key that is a proper PREFIX of it sorts just before it, so the
            // unknown KEY is. One construction per sentence, whichever names the catalog
            // publishes.
            let prefix = &name[..name.len() - 1];
            if table.property(prefix).is_none() {
                rows.push(Row {
                    label: format!(
                        "{name} at an unpublished value beside an unknown key sorting before \
                         it, on {}",
                        table.name
                    ),
                    table: table.name,
                    entries: vec![
                        (name.to_string(), VALUE_OUTSIDE_EVERY_LIST.to_string()),
                        (prefix.to_string(), "TRUE".to_string()),
                    ],
                });
            }
        }
        if let [first, second, ..] = enumerated.as_slice() {
            let mut entries = vec![
                (first.name.to_string(), VALUE_OUTSIDE_EVERY_LIST.to_string()),
                (
                    second.name.to_string(),
                    VALUE_OUTSIDE_EVERY_LIST.to_string(),
                ),
            ];
            entries.sort_by(|left, right| right.0.cmp(&left.0));
            rows.push(Row {
                label: format!(
                    "{} and {} both at an unpublished value, on {}",
                    first.name, second.name, table.name
                ),
                table: table.name,
                entries,
            });
            rows.push(Row {
                label: format!(
                    "{} at a published value beside two invalid entries, on {}",
                    first.name, table.name
                ),
                table: table.name,
                entries: vec![
                    (first.name.to_string(), first.valid_values[0].to_string()),
                    (unknown_a, "TRUE".to_string()),
                    (
                        second.name.to_string(),
                        VALUE_OUTSIDE_EVERY_LIST.to_string(),
                    ),
                ],
            });
        }
    }
    rows
}

/// One all-valid map per published table: every property it publishes, at a value it
/// accepts — a published value for an enumerated property, a written one for a free-form
/// property. A served answer must be as determined as a refusal.
fn served_rows() -> Vec<Row> {
    TABLE_CATALOG
        .iter()
        .filter(|table| !table.properties.is_empty())
        .map(|table| Row {
            label: format!("every property {} publishes, at a value it accepts", table.name),
            table: table.name,
            entries: table
                .properties
                .iter()
                .rev()
                .map(|property| {
                    let value = match enumerated_properties(table)
                        .iter()
                        .find(|candidate| candidate.name == property.name)
                    {
                        Some(enumerated) => enumerated.valid_values[0].to_string(),
                        None => FREE_FORM_VALUE.to_string(),
                    };
                    (property.name.to_string(), value)
                })
                .collect(),
        })
        .collect()
}

/// Assert `body` is the modelled 400 and return it, so a caller can compare one refusal
/// with another.
fn assert_refused(case: &str, operation: &str, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: {operation} must answer HTTP 400 ValidationException, \
         got {status} with {body}"
    );
}

#[tokio::test]
async fn get_table_refuses_a_multi_offender_property_map_for_the_first_offending_key() {
    let client = Client::new();
    let mut kinds: BTreeSet<&'static str> = BTreeSet::new();

    for row in rows() {
        let case = &row.label;
        let table = spec_of(row.table);
        let offender = expected_offender(table, &row.entries);
        kinds.insert(sentence_kind(table, &offender));

        // The refusal the entry the rule picks earns ON ITS OWN: a one-entry map, which no
        // iteration order can reorder.
        let alone = json!({
            "TableName": row.table,
            "TableProperties": map_of(std::slice::from_ref(&offender)),
        });
        let (status, expected) = client.call("GetTable", &alone).await;
        assert_refused(case, "GetTable", status, &expected);

        let request = json!({ "TableName": row.table, "TableProperties": map_of(&row.entries) });
        for attempt in 0..REPEATS {
            let (status, body) = client.call("GetTable", &request).await;
            assert_refused(case, "GetTable", status, &body);
            assert_eq!(
                body, expected,
                "case {case:?} attempt {attempt}: GetTable must answer the refusal the \
                 first offending entry {offender:?} earns alone ({expected}), got {body}"
            );
        }
    }

    // The construction above puts an unknown key both before and after a published one, so
    // the suite proves the SENTENCE is determined too, not only the key.
    assert_eq!(
        kinds.len(),
        2,
        "the rows must leave each sentence the determined answer for some row, covered: \
         {kinds:?}"
    );
}

#[tokio::test]
async fn a_submitted_definition_is_refused_for_the_first_offending_key() {
    let client = Client::new();

    for (index, row) in rows().into_iter().enumerate() {
        let case = &row.label;
        let table = spec_of(row.table);
        let offender = expected_offender(table, &row.entries);
        let statement = format!("SELECT * FROM {}", row.table);
        let alone = json!({ row.table: map_of(std::slice::from_ref(&offender)) });
        let submitted = json!({ row.table: map_of(&row.entries) });

        // THE CREATE PATH.
        let create_alone = json!({
            "Export": export_body(&format!("alone_{index}"), &statement, Some(alone.clone())),
        });
        let (status, expected) = client.call("CreateExport", &create_alone).await;
        assert_refused(case, "CreateExport", status, &expected);
        for attempt in 0..REPEATS {
            let request = json!({
                "Export": export_body(
                    &format!("created_{index}_{attempt}"),
                    &statement,
                    Some(submitted.clone()),
                ),
            });
            let (status, body) = client.call("CreateExport", &request).await;
            assert_refused(case, "CreateExport", status, &body);
            assert_eq!(
                body, expected,
                "case {case:?} attempt {attempt}: CreateExport must answer the refusal the \
                 first offending entry {offender:?} earns alone ({expected}), got {body}"
            );
        }

        // THE UPDATE PATH, over an export created from the baseline definition. A refused
        // update stores nothing, so one baseline serves every attempt.
        let update_name = format!("updated_{index}");
        let baseline = client
            .ok(
                "CreateExport",
                &json!({
                    "Export": export_body(
                        &update_name,
                        BASELINE_STATEMENT,
                        Some(baseline_configurations()),
                    ),
                }),
            )
            .await;
        let arn = baseline["ExportArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the baseline create must return an ExportArn, got {baseline}")
            })
            .to_string();
        let update_alone = json!({
            "ExportArn": arn,
            "Export": export_body(&update_name, &statement, Some(alone)),
        });
        let (status, expected) = client.call("UpdateExport", &update_alone).await;
        assert_refused(case, "UpdateExport", status, &expected);
        for attempt in 0..REPEATS {
            let request = json!({
                "ExportArn": arn,
                "Export": export_body(&update_name, &statement, Some(submitted.clone())),
            });
            let (status, body) = client.call("UpdateExport", &request).await;
            assert_refused(case, "UpdateExport", status, &body);
            assert_eq!(
                body, expected,
                "case {case:?} attempt {attempt}: UpdateExport must answer the refusal the \
                 first offending entry {offender:?} earns alone ({expected}), got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_served_property_map_reports_one_configuration_whatever_order_it_was_read_in() {
    let client = Client::new();

    for row in served_rows() {
        let case = &row.label;
        let request = json!({ "TableName": row.table, "TableProperties": map_of(&row.entries) });
        let expected = client.ok("GetTable", &request).await;
        for attempt in 0..REPEATS {
            let (status, body) = client.call("GetTable", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: GetTable must serve the request, \
                 got {status} with {body}"
            );
            assert_eq!(
                body, expected,
                "case {case:?} attempt {attempt}: GetTable must report one configuration \
                 and one schema for one request ({expected}), got {body}"
            );
        }
    }
}

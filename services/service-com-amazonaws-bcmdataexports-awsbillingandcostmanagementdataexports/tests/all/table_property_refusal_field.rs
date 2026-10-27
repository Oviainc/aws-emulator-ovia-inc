// SPDX-License-Identifier: CC0-1.0

//! Which MEMBER a table-property refusal names, for every operation that carries a
//! requested property map.
//!
//! The rule under test: `ValidationExceptionField$Name` is "[t]he field name where the
//! invalid entry was detected" (smithy_query shape `ValidationExceptionField`), so a
//! refusal names the member path of the input THE REQUEST SENT — never a member of
//! another operation's input. The property map under test reaches the service at two
//! different members: `GetTableRequest$TableProperties` for a table request, and
//! `Export$DataQuery$TableConfigurations` for a submitted export definition (`CreateExport`
//! and `UpdateExport`, neither of which declares a `TableProperties` member at all). One
//! invalid entry therefore earns one sentence and TWO member paths, each the path of the
//! input that carried it.
//!
//! The rule is about the member path, not about any one table or property, so it is
//! asserted over a TABLE of invalid property maps generated from the service's own
//! published catalog: for EVERY published table, a key that table does not publish, a key
//! only another table publishes, a value outside an enumerated property's published
//! values, a published value in the wrong case, an empty value, and one valid property
//! beside one invalid value. Each map goes through all three operations.
//!
//! The second test pins the agreement the rule implies: the sibling refusal about the SAME
//! submitted member — an update that leaves a queried table's entry unsaid — names that
//! same member path, so one request cannot be told about one member under two names.

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    TablePropertySpec, TableSpec, PROPERTY_ANY_VALUE, TABLE_CATALOG,
};

use crate::data_query_resolution::export_body;
use crate::export_lifecycle::{error_code, Client};

/// A property key no published table names, so every table refuses it.
const UNPUBLISHED_PROPERTY: &str = "NOT_A_TABLE_PROPERTY";

/// A value no enumerated property of the catalog publishes.
const VALUE_OUTSIDE_EVERY_LIST: &str = "NOT_A_PUBLISHED_VALUE";

/// The member path a `GetTable` refusal names: that operation's own input member
/// (smithy_query operation `GetTable`, input member `TableProperties`).
const GET_TABLE_FIELD: &str = "TableProperties";

/// The member path a refusal about a submitted definition names: the member the definition
/// carries its table configurations at (smithy_query shape `DataQuery`, member
/// `TableConfigurations`, reached through `CreateExportRequest$Export` /
/// `UpdateExportRequest$Export`).
const DEFINITION_FIELD: &str = "Export.DataQuery.TableConfigurations";

/// The statement the baseline export an update replaces is created with: a published table
/// with the properties it publishes left to their defaults.
const BASELINE_STATEMENT: &str = "SELECT line_item_usage_account_id FROM COST_AND_USAGE_REPORT";

/// The `TableConfigurations` the baseline definition restates, which an update owes.
fn baseline_configurations() -> Value {
    json!({ "COST_AND_USAGE_REPORT": {} })
}

/// One row: an invalid property map, and the published table it is submitted for.
#[derive(Debug)]
struct Row {
    label: String,
    table: &'static str,
    /// The `TableProperties` / per-table `TableConfigurations` entry under test.
    properties: Value,
}

/// The properties of `table` that publish a closed list of values, so a value outside the
/// list is refused. A free-form property publishes the wildcard or no list at all
/// (`TablePropertySpec::accepts`), and accepts anything.
fn enumerated_properties(table: &'static TableSpec) -> Vec<&'static TablePropertySpec> {
    table
        .properties
        .iter()
        .filter(|property| {
            !property.valid_values.is_empty()
                && !property.valid_values.contains(&PROPERTY_ANY_VALUE)
        })
        .collect()
}

/// A property name some OTHER published table names and `table` does not: a plausible key
/// that still names nothing of this table.
fn property_published_elsewhere(table: &'static TableSpec) -> Option<&'static str> {
    TABLE_CATALOG
        .iter()
        .flat_map(|other| other.properties.iter())
        .map(|property| property.name)
        .find(|name| table.property(name).is_none())
}

/// The rows, generated from the published catalog so every table it publishes is covered
/// rather than the few any one request happens to name.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for table in TABLE_CATALOG {
        rows.push(Row {
            label: format!("a key {} does not publish", table.name),
            table: table.name,
            properties: json!({ UNPUBLISHED_PROPERTY: "TRUE" }),
        });
        if let Some(elsewhere) = property_published_elsewhere(table) {
            rows.push(Row {
                label: format!("{elsewhere}, which only another table publishes, on {}", table.name),
                table: table.name,
                properties: json!({ elsewhere: "TRUE" }),
            });
        }

        let enumerated = enumerated_properties(table);
        if let Some(property) = enumerated.first() {
            let name = property.name;
            rows.push(Row {
                label: format!("a value outside {name}'s published values on {}", table.name),
                table: table.name,
                properties: json!({ name: VALUE_OUTSIDE_EVERY_LIST }),
            });
            let published = property.valid_values[0];
            let wrong_case = published.to_lowercase();
            if wrong_case != published {
                rows.push(Row {
                    label: format!("{published} in the wrong case on {}", table.name),
                    table: table.name,
                    properties: json!({ name: wrong_case }),
                });
            }
            rows.push(Row {
                label: format!("an empty value for {name} on {}", table.name),
                table: table.name,
                properties: json!({ name: "" }),
            });
        }
        if let [valid, invalid, ..] = enumerated.as_slice() {
            let (valid_name, valid_value) = (valid.name, valid.valid_values[0]);
            let invalid_name = invalid.name;
            rows.push(Row {
                label: format!(
                    "{valid_name} at a published value beside {invalid_name} outside its own, \
                     on {}",
                    table.name
                ),
                table: table.name,
                properties: json!({
                    valid_name: valid_value,
                    invalid_name: VALUE_OUTSIDE_EVERY_LIST,
                }),
            });
        }
    }
    rows
}

/// Whether `path` — a dot-separated member path — resolves to a value `request` carries.
fn resolves_in(request: &Value, path: &str) -> bool {
    let mut current = request;
    for segment in path.split('.') {
        match current.get(segment) {
            Some(next) => current = next,
            None => return false,
        }
    }
    true
}

/// Assert that `body` is the modelled 400 naming `expected` as the one member at fault,
/// and that `expected` is a member path the refused `request` actually carries. Returns the
/// refusal's sentence, so the callers can compare the sentence one fault earns across the
/// operations that report it.
fn assert_names_submitted_member(
    case: &str,
    operation: &str,
    request: &Value,
    (status, body): (u16, &Value),
    expected: &str,
) -> String {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: {operation} must answer HTTP 400 ValidationException, \
         got {status} with {body}"
    );
    let fields = body
        .get("Fields")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: {operation} must name the field at fault, got {body}"));
    assert_eq!(
        fields.len(),
        1,
        "case {case:?}: {operation} must name exactly the one member at fault, got {body}"
    );
    let name = fields[0].get("Name").and_then(Value::as_str).unwrap_or("");
    assert_eq!(
        name, expected,
        "case {case:?}: {operation} must name {expected:?} as the field the invalid entry \
         was detected at, got {name:?} in {body}"
    );
    assert!(
        resolves_in(request, name),
        "case {case:?}: {operation} named the member path {name:?}, which the request it \
         refused does not carry: {request}"
    );
    body["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {operation} must state the refusal, got {body}"))
        .to_string()
}

#[tokio::test]
async fn a_table_property_refusal_names_the_member_the_request_carried_it_at() {
    let client = Client::new();

    for (index, row) in rows().into_iter().enumerate() {
        let case = &row.label;
        let configurations = json!({ row.table: row.properties.clone() });
        let statement = format!("SELECT * FROM {}", row.table);

        // THE TABLE REQUEST: the property map at `GetTableRequest$TableProperties`.
        let get_table_request =
            json!({ "TableName": row.table, "TableProperties": row.properties.clone() });
        let (status, body) = client.call("GetTable", &get_table_request).await;
        let get_table_message = assert_names_submitted_member(
            case,
            "GetTable",
            &get_table_request,
            (status, &body),
            GET_TABLE_FIELD,
        );

        // THE CREATE PATH: the same map at `Export.DataQuery.TableConfigurations`.
        let create_request = json!({
            "Export": export_body(
                &format!("created_{index}"),
                &statement,
                Some(configurations.clone()),
            ),
        });
        let (status, body) = client.call("CreateExport", &create_request).await;
        let create_message = assert_names_submitted_member(
            case,
            "CreateExport",
            &create_request,
            (status, &body),
            DEFINITION_FIELD,
        );
        assert!(
            body.get("ExportArn").is_none(),
            "case {case:?}: a refused create must mint no export, got {body}"
        );

        // THE UPDATE PATH: the same map, submitted as the whole replacement definition.
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
        let update_request = json!({
            "ExportArn": arn,
            "Export": export_body(&update_name, &statement, Some(configurations)),
        });
        let (status, body) = client.call("UpdateExport", &update_request).await;
        let update_message = assert_names_submitted_member(
            case,
            "UpdateExport",
            &update_request,
            (status, &body),
            DEFINITION_FIELD,
        );

        // ONE fault, one sentence: the member path differs with the input that carried the
        // entry, and nothing else does.
        assert_eq!(
            create_message, update_message,
            "case {case:?}: CreateExport stated {create_message:?} and UpdateExport \
             {update_message:?}"
        );
        assert_eq!(
            create_message, get_table_message,
            "case {case:?}: the definition path stated {create_message:?} and GetTable \
             {get_table_message:?}"
        );
    }
}

#[tokio::test]
async fn both_refusals_about_the_submitted_table_configurations_name_one_member() {
    let client = Client::new();

    // An update owes every queried table's entry, so a definition that leaves one unsaid is
    // refused ABOUT THE SAME MEMBER as an invalid entry in it — over every published table
    // that has an entry to leave unsaid.
    for (index, table) in TABLE_CATALOG
        .iter()
        .filter(|table| !table.properties.is_empty())
        .enumerate()
    {
        let case = format!("{} configured by no entry at all", table.name);
        let name = format!("unsaid_{index}");
        let baseline = client
            .ok(
                "CreateExport",
                &json!({
                    "Export": export_body(
                        &name,
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

        let update_request = json!({
            "ExportArn": arn,
            "Export": export_body(&name, &format!("SELECT * FROM {}", table.name), None),
        });
        let (status, body) = client.call("UpdateExport", &update_request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: UpdateExport must answer HTTP 400 ValidationException, \
             got {status} with {body}"
        );
        let field = body["Fields"][0]["Name"].as_str().unwrap_or("");
        assert_eq!(
            field, DEFINITION_FIELD,
            "case {case:?}: the refusal must name {DEFINITION_FIELD:?}, got {field:?} in {body}"
        );
        // The entry itself is what the request left unsaid, so the member the path names is
        // absent — but the structure it belongs to is one the request carries.
        let parent = DEFINITION_FIELD
            .rsplit_once('.')
            .map(|(parent, _)| parent)
            .unwrap_or(DEFINITION_FIELD);
        assert!(
            resolves_in(&update_request, parent),
            "case {case:?}: the refusal named a path under {parent:?}, which the request it \
             refused does not carry: {update_request}"
        );
    }
}

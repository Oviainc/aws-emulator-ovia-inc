// SPDX-License-Identifier: CC0-1.0

//! The export definition's `DataQuery` as PERSISTED STATE: the table configuration a
//! reader reports for it, and what a client that reads it back can submit again.
//!
//! The rule under test: a stored definition carries the table configuration IN FORCE —
//! for every table its query names (and every table the request configures), each table
//! property the catalog publishes for that table, at the value the request gave it and at
//! the published default otherwise, which is the same rule `GetTable` applies to a table
//! request ("Each table property has a default value that it assumes if not specified" —
//! smithy_query shape `GetTableRequest`, member `TableProperties`). The `QueryStatement`
//! is stored verbatim. So nothing a request said is dropped, and a definition read back
//! through `GetExport` and handed straight to `UpdateExport` keeps every per-table
//! property the client never touched.
//!
//! It is asserted over a TABLE of definitions that differ along what the rule generalises
//! over: no `TableConfigurations` member at all, an empty one, an empty property map for
//! the queried table, one property named, several named, each published value of each
//! published property, a free-form property value, and a configuration for a table beside
//! the one the query reads — crossed with every table the catalog publishes. The catalog
//! tables and their properties come from the service's own published catalog
//! (`TABLE_CATALOG`), so a table and a value the issue never cited are covered too; the
//! Cost and Usage Report expectation is additionally written out by hand from the AWS
//! Data Exports table dictionary, so it is not just this emulator's copy of itself.
//!
//! Which queries are REFUSED is `export_definition_validation`'s rule, not this suite's;
//! the one refusal row below is here only to pin that a refused create persists nothing,
//! which is a statement about the store.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::TABLE_CATALOG;

use crate::export_lifecycle::{cases, error_code, Client, NOT_FOUND_STATUS};

/// A table name no catalog publishes — the recorded one.
const BOGUS_TABLE: &str = "totally_bogus_table_that_does_not_exist";

/// The table properties `COST_AND_USAGE_REPORT` publishes and the value each assumes when
/// a request does not name it, from the AWS Data Exports table dictionary: the switches
/// over the table's data and schema, plus the billing view the table is scoped to, which
/// defaults to the ACCOUNT's PRIMARY billing view. A property the dictionary publishes
/// with NO default assumes nothing, so a request that does not name it reports no value
/// for it and it is absent here.
///
/// Written out independently of [`TABLE_CATALOG`] so the expectation is the published
/// dictionary and not this emulator's copy of it. The billing view's default carries the
/// account id, so it is built through `engine::emulator_arn` rather than pinned as text.
fn cost_and_usage_report_defaults() -> BTreeMap<String, String> {
    [
        ("BILLING_VIEW_ARN", primary_billing_view()),
        ("INCLUDE_CAPACITY_RESERVATION_DATA", "FALSE".to_string()),
        ("INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY", "FALSE".to_string()),
        ("INCLUDE_RESOURCES", "FALSE".to_string()),
        ("INCLUDE_SPLIT_COST_ALLOCATION_DATA", "FALSE".to_string()),
        ("TIME_GRANULARITY", "HOURLY".to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// The ARN of the account's PRIMARY billing view,
/// `arn:aws:billing::<account>:billingview/primary` — the value `BILLING_VIEW_ARN`
/// assumes when a definition does not name one.
fn primary_billing_view() -> String {
    engine::emulator_arn("billing", "", "billingview/primary")
}

/// A `CreateExport` body for `name` whose query is `statement` and which submits
/// `table_configurations`, built on a valid definition from the lifecycle table.
fn create_request(name: &str, statement: &str, table_configurations: Option<Value>) -> Value {
    let mut request = cases().remove(0).create_request();
    let export = request["Export"]
        .as_object_mut()
        .expect("the Export structure is a JSON object");
    export.insert("Name".to_string(), json!(name));
    let mut data_query = Map::new();
    data_query.insert("QueryStatement".to_string(), json!(statement));
    if let Some(configurations) = table_configurations {
        data_query.insert("TableConfigurations".to_string(), configurations);
    }
    export.insert("DataQuery".to_string(), Value::Object(data_query));
    request
}

/// The `TableConfigurations` of the definition `GetExport` reports for `arn`, or `None`
/// when the definition carries none. The `QueryStatement` is checked here too: it is
/// stored verbatim for every row.
async fn stored_table_configurations(
    client: &Client,
    case: &str,
    arn: &str,
    statement: &str,
) -> Option<BTreeMap<String, BTreeMap<String, String>>> {
    let body = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
    let data_query = &body["Export"]["DataQuery"];
    assert_eq!(
        data_query["QueryStatement"],
        json!(statement),
        "case {case:?}: the QueryStatement must be stored verbatim, got {body}"
    );
    let configurations = data_query.get("TableConfigurations")?;
    let map = configurations
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: TableConfigurations must be a map, got {body}"));
    Some(
        map.iter()
            .map(|(table, properties)| {
                let properties = properties.as_object().unwrap_or_else(|| {
                    panic!("case {case:?}: the properties of {table} must be a map, got {body}")
                });
                (
                    table.clone(),
                    properties
                        .iter()
                        .map(|(property, value)| {
                            (
                                property.clone(),
                                value
                                    .as_str()
                                    .unwrap_or_else(|| {
                                        panic!(
                                            "case {case:?}: {table}.{property} must be a string, got {body}"
                                        )
                                    })
                                    .to_string(),
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

/// RULE: every table the catalog publishes, queried with NO `TableConfigurations` at all,
/// is read back with that table's published properties at their defaults — and a table
/// publishing none is read back carrying no configuration.
#[tokio::test]
async fn a_stored_definition_carries_the_table_configuration_in_force() {
    let client = Client::new();

    for table in TABLE_CATALOG {
        let case = format!("{} with no TableConfigurations", table.name);
        let name = format!("defaults_{}", table.name.to_lowercase());
        let statement = format!("SELECT * FROM {}", table.name);
        let created = client
            .ok("CreateExport", &create_request(&name, &statement, None))
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateExport returned no ARN: {created}"))
            .to_string();

        let stored = stored_table_configurations(&client, &case, &arn, &statement).await;
        if table.properties.is_empty() {
            assert!(
                stored.is_none(),
                "case {case:?}: a table publishing no properties has no configuration to report, got {stored:?}"
            );
            continue;
        }
        let expected: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::from([(
            table.name.to_string(),
            table
                .properties
                .iter()
                // A property the catalog publishes no default for is in force only for a
                // request that names it, and these rows name none.
                .filter_map(|property| {
                    property
                        .default_value()
                        .map(|default| (property.name.to_string(), default.to_string()))
                })
                .collect(),
        )]);
        assert_eq!(
            stored,
            Some(expected.clone()),
            "case {case:?}: expected the properties in force {expected:?}, got {stored:?}"
        );
    }
}

/// RULE, against the PUBLISHED dictionary rather than this emulator's catalog: the Cost
/// and Usage Report table with none, one and several properties named, and with each
/// published value of each of its properties.
#[tokio::test]
async fn a_named_property_wins_and_the_rest_default() {
    let client = Client::new();
    let statement = "SELECT line_item_usage_amount FROM COST_AND_USAGE_REPORT";
    let cost_and_usage = TABLE_CATALOG
        .iter()
        .find(|table| table.name == "COST_AND_USAGE_REPORT")
        .expect("the catalog publishes COST_AND_USAGE_REPORT");

    let mut rows: Vec<(String, Map<String, Value>)> = vec![
        ("no properties named".to_string(), Map::new()),
        (
            "an empty property map".to_string(),
            Map::from_iter([] as [(String, Value); 0]),
        ),
        (
            "one property named".to_string(),
            Map::from_iter([("TIME_GRANULARITY".to_string(), json!("MONTHLY"))]),
        ),
        (
            "two properties named, one free-form".to_string(),
            Map::from_iter([
                ("INCLUDE_RESOURCES".to_string(), json!("TRUE")),
                (
                    "BILLING_VIEW_ARN".to_string(),
                    json!(engine::emulator_arn(
                        "billing",
                        "",
                        "billingview/custom-view"
                    )),
                ),
            ]),
        ),
    ];
    // One row per published value of each published property, so a value no recorded
    // request ever used is covered too.
    for property in cost_and_usage.properties {
        for value in property.valid_values {
            rows.push((
                format!("{} = {value}", property.name),
                Map::from_iter([(property.name.to_string(), json!(value))]),
            ));
        }
    }

    for (index, (label, requested)) in rows.into_iter().enumerate() {
        let case = format!("COST_AND_USAGE_REPORT with {label}");
        let name = format!("dictionary_{index}");
        let request = create_request(
            &name,
            statement,
            Some(json!({ "COST_AND_USAGE_REPORT": Value::Object(requested.clone()) })),
        );
        let created = client.ok("CreateExport", &request).await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateExport returned no ARN: {created}"))
            .to_string();

        let mut expected_properties = cost_and_usage_report_defaults();
        for (property, value) in &requested {
            expected_properties.insert(
                property.clone(),
                value
                    .as_str()
                    .expect("a property value is a string")
                    .to_string(),
            );
        }
        let expected = BTreeMap::from([("COST_AND_USAGE_REPORT".to_string(), expected_properties)]);
        let stored = stored_table_configurations(&client, &case, &arn, statement).await;
        assert_eq!(
            stored,
            Some(expected.clone()),
            "case {case:?}: expected {expected:?}, got {stored:?}"
        );
    }
}

/// RULE: a table the request configures but the query does not read is still the caller's
/// configuration — it is checked, completed and reported like any other.
#[tokio::test]
async fn a_configured_table_the_query_does_not_read_is_reported_too() {
    let client = Client::new();
    let statement = "SELECT line_item_usage_amount FROM COST_AND_USAGE_REPORT";
    let other = TABLE_CATALOG
        .iter()
        .find(|table| table.name == "COST_OPTIMIZATION_RECOMMENDATIONS")
        .expect("the catalog publishes COST_OPTIMIZATION_RECOMMENDATIONS");

    let created = client
        .ok(
            "CreateExport",
            &create_request(
                "configured_second_table",
                statement,
                Some(json!({
                    other.name: { "INCLUDE_ALL_RECOMMENDATIONS": "TRUE" },
                })),
            ),
        )
        .await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    let stored = stored_table_configurations(&client, "a second configured table", &arn, statement)
        .await
        .unwrap_or_else(|| panic!("the stored definition must carry a configuration"));
    assert_eq!(
        stored.get("COST_AND_USAGE_REPORT"),
        Some(&cost_and_usage_report_defaults()),
        "the queried table must be reported at its defaults, got {stored:?}"
    );
    let mut expected_other: BTreeMap<String, String> = other
        .properties
        .iter()
        .filter_map(|property| {
            property
                .default_value()
                .map(|default| (property.name.to_string(), default.to_string()))
        })
        .collect();
    expected_other.insert(
        "INCLUDE_ALL_RECOMMENDATIONS".to_string(),
        "TRUE".to_string(),
    );
    assert_eq!(
        stored.get(other.name),
        Some(&expected_other),
        "a configured table the query does not read must be reported too, got {stored:?}"
    );
}

/// RULE, the other half: a definition read back can be handed straight to `UpdateExport`
/// and comes back unchanged — the read-modify-write a client performs when it edits an
/// export, which only holds because the stored definition carries the properties in force
/// ("All export parameters must be specified in the UpdateExport request").
#[tokio::test]
async fn a_definition_read_back_can_be_submitted_again_unchanged() {
    let client = Client::new();

    for (index, table) in TABLE_CATALOG.iter().enumerate() {
        let case = format!("{} round trip", table.name);
        let name = format!("round_trip_{index}");
        let statement = format!("SELECT * FROM {}", table.name);
        let created = client
            .ok("CreateExport", &create_request(&name, &statement, None))
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateExport returned no ARN: {created}"))
            .to_string();

        let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        let definition = fetched["Export"].clone();
        client
            .ok(
                "UpdateExport",
                &json!({ "ExportArn": arn, "Export": definition.clone() }),
            )
            .await;
        let after = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            after["Export"], definition,
            "case {case:?}: a round-tripped definition must come back unchanged, got {}",
            after["Export"]
        );
    }
}

/// A refused create stores NOTHING: the ARN its name would have minted names nothing
/// afterwards. (Which queries are refused is `export_definition_validation`'s rule; this
/// row is about the store.)
#[tokio::test]
async fn a_refused_create_persists_nothing() {
    let client = Client::new();

    for (index, statement) in [
        format!("SELECT x FROM {BOGUS_TABLE}"),
        format!("SELECT a.x FROM COST_AND_USAGE_REPORT a JOIN {BOGUS_TABLE} b ON a.y = b.y"),
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("refused create {index}");
        let name = format!("refused_create_{index}");
        let (status, body) = client
            .call("CreateExport", &create_request(&name, &statement, None))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a query naming an unpublished table must be refused, got {status} with {body}"
        );

        let arn = engine::emulator_arn("bcm-data-exports", "us-east-1", &format!("export/{name}"));
        let (status, body) = client.call("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {case:?}: a refused create must have stored nothing, got {status} with {body}"
        );
    }
}

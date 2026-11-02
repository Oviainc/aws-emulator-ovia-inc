// SPDX-License-Identifier: CC0-1.0

//! The COLUMNS a `DataQuery.QueryStatement` may select and filter on, over the awsJson1_1
//! wire, for both operations that accept an export definition (`CreateExport`,
//! `UpdateExport`).
//!
//! The rule under test: a query statement may name a column exactly when some table it
//! reads PUBLISHES that column under the table configuration the request states — "To view
//! the available tables and columns, see the Data Exports table dictionary" (the modelled
//! doc for `CreateExport`'s `Export`), where a table property "[changes] the data and
//! schema of a table" (the modelled doc for `GetTableRequest$TableProperties`), so the
//! published column set is the one `GetTable` reports for THAT configuration. A statement
//! naming anything else selects a column the export could never deliver, so it is refused
//! with the modelled `ValidationException` (HTTP 400) naming the offending column and
//! `Export.DataQuery.QueryStatement` — and, for an update, the stored definition is left
//! exactly as it was rather than replaced with one the service could not serve.
//!
//! Every row's expectation comes from `GetTable` — the service's own published schema for
//! the table and properties the row states — so the suite asserts the two readings of the
//! dictionary AGREE, rather than restating a column list here.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the table: every table the catalog publishes, not only the one the issue cited;
//! * WHERE in the statement the name sits: a select item, a second select item, a `WHERE`,
//!   a join condition, a sub-query, a function argument, a `GROUP BY`, an `ORDER BY`;
//! * the column: a published one, an unpublished one, and — the gating half — every column
//!   the catalog publishes only while a table property holds a value, at the value that
//!   publishes it and at a published value that does not;
//! * the operation: the create that first stores a definition and the update that
//!   overwrites one.

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    TableSpec, TABLE_CATALOG,
};

use crate::export_lifecycle::{cases, error_code, Client};

/// The modelled input member every refusal of a query statement names
/// (`DataQuery$QueryStatement`, reached through `Export$DataQuery`).
const QUERY_STATEMENT_FIELD: &str = "Export.DataQuery.QueryStatement";

/// The `ValidationExceptionReason` a refusal of one named input member reports — one of
/// the modelled values (smithy_query shape `ValidationExceptionReason`).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// A column name no table of the catalog publishes — the one the issue cited.
///
/// `pub(crate)` so a suite asserting the same rule over another part of a statement names
/// the SAME unpublished column rather than growing a second spelling of it.
pub(crate) const UNPUBLISHED_COLUMN: &str = "not_a_real_column";

/// The `Export` a row submits: a definition that satisfies every modelled constraint, with
/// the name, the query statement and the table configurations substituted.
///
/// `pub(crate)` so a suite asserting the same rule over another part of the catalog submits
/// the SAME definition rather than growing a second copy of it.
pub(crate) fn export_body(name: &str, statement: &str, table_configurations: Option<Value>) -> Value {
    let mut export = cases().remove(0).create_request()["Export"].clone();
    let object = export.as_object_mut().expect("Export is a JSON object");
    object.insert("Name".to_string(), json!(name));
    let mut data_query = Map::new();
    data_query.insert("QueryStatement".to_string(), json!(statement));
    if let Some(configurations) = table_configurations {
        data_query.insert("TableConfigurations".to_string(), configurations);
    }
    object.insert("DataQuery".to_string(), Value::Object(data_query));
    export
}

/// An export name derived from `label`, inside `Export.Name`'s `@pattern
/// ^[0-9A-Za-z\-_]+$` and `@length(1, 128)` (smithy_query shape `Export`, member `Name`).
pub(crate) fn export_name(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(128)
        .collect()
}

/// The columns `table` PUBLISHES under `properties`, as `GetTable` reports them — the
/// service's own published schema for that configuration, which is what a query statement
/// is judged against.
///
/// `pub(crate)` so every suite in this target reads the published column set through the
/// service's own `GetTable` rather than restating a column list of its own.
pub(crate) async fn published_columns(
    client: &Client,
    table: &str,
    properties: Option<&Value>,
) -> Vec<String> {
    let mut request = Map::new();
    request.insert("TableName".to_string(), json!(table));
    if let Some(properties) = properties {
        request.insert("TableProperties".to_string(), properties.clone());
    }
    let described = client.ok("GetTable", &Value::Object(request)).await;
    let schema = described["Schema"].as_array().unwrap_or_else(|| {
        panic!("GetTable must report a Schema for {table}, got {described}")
    });
    schema
        .iter()
        .map(|column| {
            column["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("every published column has a Name, got {column}"))
                .to_string()
        })
        .collect()
}

/// The statements that name `column` — one per PLACE a statement can name a column, all
/// over `table` and all otherwise valid, so the named column is the only thing that can
/// decide the answer. `anchor` is a column the configuration in force publishes, which
/// carries the rest of each statement.
///
/// Each row is `(where the name sits, the statement, the reference AS WRITTEN)`: a refusal
/// names what the caller wrote, which for a qualified reference is the qualifier and the
/// column together.
fn statements_naming(table: &str, column: &str, anchor: &str) -> Vec<(String, String, String)> {
    vec![
        (
            "a select item".to_string(),
            format!("SELECT {column} FROM {table}"),
            column.to_string(),
        ),
        (
            "a second select item".to_string(),
            format!("SELECT {anchor}, {column} FROM {table}"),
            column.to_string(),
        ),
        (
            "a WHERE predicate".to_string(),
            format!("SELECT {anchor} FROM {table} WHERE {column} IS NOT NULL"),
            column.to_string(),
        ),
        (
            "a join condition".to_string(),
            format!(
                "SELECT a.{anchor} FROM {table} a JOIN {table} b ON a.{column} = b.{anchor}"
            ),
            format!("a.{column}"),
        ),
        (
            "a sub-query select item".to_string(),
            format!(
                "SELECT {anchor} FROM {table} WHERE {anchor} IN (SELECT {column} FROM {table})"
            ),
            column.to_string(),
        ),
        (
            "a function argument".to_string(),
            format!("SELECT COUNT({column}) FROM {table}"),
            column.to_string(),
        ),
        (
            "a GROUP BY".to_string(),
            format!("SELECT {anchor} FROM {table} GROUP BY {column}"),
            column.to_string(),
        ),
        (
            "an ORDER BY".to_string(),
            format!("SELECT {anchor} FROM {table} ORDER BY {column}"),
            column.to_string(),
        ),
    ]
}

/// Assert `body` is the modelled refusal of a statement naming `column`: HTTP 400,
/// `ValidationException`, a `Message` built from the request that names the column, the
/// field-failure reason, and `Export.DataQuery.QueryStatement` as the named field.
pub(crate) fn assert_column_refused(case: &str, status: u16, body: &Value, column: &str) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(format!("Column name {column} is invalid").as_str()),
        "case {case:?}: the refusal must name the offending column, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(FIELD_VALIDATION_FAILED),
        "case {case:?}: expected the field-failure reason, got {body}"
    );
    let named: Vec<&str> = body["Fields"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must name a field, got {body}"))
        .iter()
        .filter_map(|field| field["Name"].as_str())
        .collect();
    assert_eq!(
        named,
        vec![QUERY_STATEMENT_FIELD],
        "case {case:?}: expected the refusal to name {QUERY_STATEMENT_FIELD}, got {named:?}"
    );
}

/// Every column `spec` publishes only while a table property holds a value, with that
/// property and value, and a published value of the same property that does NOT publish
/// it. A property with a single published value gates nothing a request can switch off, so
/// it contributes no row.
///
/// Read off the service's published catalog (`TABLE_CATALOG`, which `ListTables` reports),
/// so a gated column no issue ever cited is covered too.
///
/// `pub(crate)` so a suite asserting another half of the column rule walks the SAME gated
/// columns rather than listing them again.
pub(crate) fn gated_columns(
    spec: &TableSpec,
) -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    spec.columns()
        .filter_map(|column| {
            let (property, publishing) = column.requires?;
            let excluding = spec
                .property(property)?
                .valid_values
                .iter()
                .find(|value| **value != publishing)?;
            Some((column.name, property, publishing, *excluding))
        })
        .collect()
}

/// How many exports the store holds.
async fn export_count(client: &Client) -> usize {
    let listed = client.ok("ListExports", &json!({})).await;
    listed["Exports"].as_array().map(Vec::len).unwrap_or_default()
}

/// RULE: for every table the catalog publishes, a create naming a column that table
/// publishes is accepted and read back, and a create naming one it does not is refused
/// with the modelled 400 naming the column — wherever in the statement the name sits.
#[tokio::test]
async fn a_create_accepts_the_published_columns_and_refuses_the_rest() {
    let client = Client::new();
    let mut stored = 0;

    for (table_index, table) in TABLE_CATALOG.iter().enumerate() {
        // The published schema for this table under its own defaults, as GetTable reports
        // it: the first column carries the statements, and the last is the row that proves
        // the acceptance is not about one column.
        let columns = published_columns(&client, table.name, None).await;
        assert!(
            columns.len() > 1,
            "case {:?}: the published schema must carry columns, got {columns:?}",
            table.name
        );
        let anchor = columns[0].clone();
        let published = columns[columns.len() - 1].clone();

        for (index, (label, statement, _written)) in
            statements_naming(table.name, &published, &anchor)
                .into_iter()
                .enumerate()
        {
            let case = format!("{} publishes {published} in {label}", table.name);
            let name = export_name(&format!("ok_{table_index}_{index}"));
            let created = client
                .ok(
                    "CreateExport",
                    &json!({ "Export": export_body(&name, &statement, None) }),
                )
                .await;
            stored += 1;
            let arn = created["ExportArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: CreateExport must return an ExportArn"))
                .to_string();
            let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
            assert_eq!(
                fetched["Export"]["DataQuery"]["QueryStatement"].as_str(),
                Some(statement.as_str()),
                "case {case:?}: GetExport must report the accepted statement, got {fetched}"
            );
        }

        for (index, (label, statement, written)) in
            statements_naming(table.name, UNPUBLISHED_COLUMN, &anchor)
                .into_iter()
                .enumerate()
        {
            let case = format!("{} does not publish {UNPUBLISHED_COLUMN} in {label}", table.name);
            assert!(
                !columns.iter().any(|column| column == UNPUBLISHED_COLUMN),
                "case {case:?}: the row's column must be unpublished, got {columns:?}"
            );
            let name = export_name(&format!("bad_{table_index}_{index}"));
            let (status, body) = client
                .call(
                    "CreateExport",
                    &json!({ "Export": export_body(&name, &statement, None) }),
                )
                .await;
            assert_column_refused(&case, status, &body, &written);
            assert_eq!(
                export_count(&client).await,
                stored,
                "case {case:?}: a refused create must store nothing"
            );
        }
    }
}

/// RULE, the gating half: a column the dictionary publishes only while a table property
/// holds a value may be named exactly when the request's own `TableConfigurations` sets
/// that value — and `GetTable` reports the same column set for the same configuration.
#[tokio::test]
async fn a_gated_column_needs_the_configuration_that_publishes_it() {
    let client = Client::new();
    let mut rows = 0;

    for table in TABLE_CATALOG {
        for (index, (column, property, publishing, excluding)) in
            gated_columns(table).into_iter().enumerate()
        {
            rows += 1;
            let configuration = |value: &str| json!({ property: value });
            let statement = format!("SELECT {column} FROM {}", table.name);

            // At the value that publishes it: GetTable reports the column, and a create
            // naming it is accepted and read back.
            let published =
                published_columns(&client, table.name, Some(&configuration(publishing))).await;
            let case = format!("{}.{column} with {property} = {publishing}", table.name);
            assert!(
                published.iter().any(|name| name == column),
                "case {case:?}: GetTable must publish the column at this value, got {published:?}"
            );
            let name = export_name(&format!("gated_on_{index}_{}", table.name));
            let created = client
                .ok(
                    "CreateExport",
                    &json!({
                        "Export": export_body(
                            &name,
                            &statement,
                            Some(json!({ table.name: configuration(publishing) })),
                        ),
                    }),
                )
                .await;
            let arn = created["ExportArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: CreateExport must return an ExportArn"))
                .to_string();
            let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
            assert_eq!(
                fetched["Export"]["DataQuery"]["QueryStatement"].as_str(),
                Some(statement.as_str()),
                "case {case:?}: GetExport must report the accepted statement, got {fetched}"
            );

            // At a published value that does not: GetTable omits the column, and the same
            // statement is refused naming it.
            let without =
                published_columns(&client, table.name, Some(&configuration(excluding))).await;
            let case = format!("{}.{column} with {property} = {excluding}", table.name);
            assert!(
                !without.iter().any(|name| name == column),
                "case {case:?}: GetTable must omit the column at this value, got {without:?}"
            );
            let (status, body) = client
                .call(
                    "CreateExport",
                    &json!({
                        "Export": export_body(
                            &export_name(&format!("gated_off_{index}_{}", table.name)),
                            &statement,
                            Some(json!({ table.name: configuration(excluding) })),
                        ),
                    }),
                )
                .await;
            assert_column_refused(&case, status, &body, column);

            // And with no configuration at all, the answer follows the property's PUBLISHED
            // DEFAULT, which is the value in force then.
            let default = table
                .property(property)
                .and_then(|spec| spec.default_value())
                .unwrap_or(excluding);
            let (status, body) = client
                .call(
                    "CreateExport",
                    &json!({
                        "Export": export_body(
                            &export_name(&format!("gated_default_{index}_{}", table.name)),
                            &statement,
                            None,
                        ),
                    }),
                )
                .await;
            let case = format!("{}.{column} with {property} defaulted to {default}", table.name);
            if default == publishing {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the default publishes the column, got {status} with {body}"
                );
            } else {
                assert_column_refused(&case, status, &body, column);
            }
        }
    }

    assert!(
        rows > 0,
        "the catalog must publish at least one property-gated column to assert the rule on"
    );
}

/// RULE for `UpdateExport`, which "[updates] an existing data export by overwriting all
/// export parameters": the replacement definition is judged exactly as a create's is, so a
/// replacement naming a column the configuration it states does not publish is refused —
/// and the stored definition, with the instant it was last updated, is left untouched
/// rather than replaced with one the service could never deliver.
#[tokio::test]
async fn an_update_refuses_an_unservable_replacement_and_keeps_the_stored_definition() {
    let client = Client::new();
    let table = "COST_AND_USAGE_REPORT";
    let columns = published_columns(&client, table, None).await;
    let anchor = columns[0].clone();

    let name = export_name("updated_columns");
    let created = client
        .ok(
            "CreateExport",
            &json!({
                "Export": export_body(&name, &format!("SELECT {anchor} FROM {table}"), None),
            }),
        )
        .await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    // The stored definition carries the table configuration in force, which an update has
    // to restate — so every update below submits the definition it read back, with only
    // the statement (and, for the gated rows, the configuration) substituted.
    let before = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
    let definition = before["Export"].clone();
    let replacement = |statement: &str, configuration: Option<Value>| {
        let mut export = definition.clone();
        export["DataQuery"]["QueryStatement"] = json!(statement);
        if let Some(configuration) = configuration {
            export["DataQuery"]["TableConfigurations"] = json!({ table: configuration });
        }
        json!({ "ExportArn": arn, "Export": export })
    };

    // A gated column of this table, for the row whose configuration switches it off: the
    // statement is readable and every table it names is published, so the configuration is
    // the only thing that refuses it.
    let gated = gated_columns(
        TABLE_CATALOG
            .iter()
            .find(|candidate| candidate.name == table)
            .expect("the catalog publishes the cost and usage table"),
    );
    assert!(
        !gated.is_empty(),
        "the cost and usage table must publish a property-gated column to assert the rule on"
    );
    let (gated_column, gating_property, publishing_value, excluding_value) = gated[0];

    let mut rows: Vec<(String, String, Option<Value>, String)> = Vec::new();
    for (label, statement, written) in statements_naming(table, UNPUBLISHED_COLUMN, &anchor) {
        rows.push((
            format!("the replacement names {UNPUBLISHED_COLUMN} in {label}"),
            statement,
            None,
            written,
        ));
    }
    rows.push((
        format!("the replacement names {gated_column} with {gating_property} = {excluding_value}"),
        format!("SELECT {gated_column} FROM {table}"),
        Some(json!({ gating_property: excluding_value })),
        gated_column.to_string(),
    ));
    rows.push((
        "the replacement qualifies an unpublished column".to_string(),
        format!("SELECT t.{UNPUBLISHED_COLUMN} FROM {table} t"),
        None,
        format!("t.{UNPUBLISHED_COLUMN}"),
    ));

    for (case, statement, configuration, column) in rows {
        let (status, body) = client
            .call("UpdateExport", &replacement(&statement, configuration))
            .await;
        assert_column_refused(&case, status, &body, &column);

        let after = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            after["Export"], definition,
            "case {case:?}: a refused update must leave the stored definition untouched, got {}",
            after["Export"]
        );
        for instant in ["CreatedAt", "LastUpdatedAt"] {
            assert_eq!(
                after["ExportStatus"][instant], before["ExportStatus"][instant],
                "case {case:?}: a refused update must not move {instant}, got {}",
                after["ExportStatus"]
            );
        }
    }

    // The other half: a replacement naming a column the configuration it states DOES
    // publish is accepted and observable — including the gated column, once the request
    // states the value that publishes it.
    for (case, statement, configuration) in [
        (
            "a published column".to_string(),
            format!("SELECT {} FROM {table}", columns[columns.len() - 1]),
            None,
        ),
        (
            format!("{gated_column} with {gating_property} = {publishing_value}"),
            format!("SELECT {gated_column} FROM {table}"),
            Some(json!({ gating_property: publishing_value })),
        ),
    ] {
        client
            .ok("UpdateExport", &replacement(&statement, configuration))
            .await;
        let after = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            after["Export"]["DataQuery"]["QueryStatement"].as_str(),
            Some(statement.as_str()),
            "case {case:?}: an accepted update must be observable, got {}",
            after["Export"]
        );
    }
}

/// A name the statement DEFINES for itself is not a column of the table: the alias of a
/// select item may be named by a later clause of the same statement, and a qualified
/// reference resolves on the part that names the column.
#[tokio::test]
async fn a_statement_may_name_what_it_defines_itself() {
    let client = Client::new();
    let table = "COST_AND_USAGE_REPORT";
    let columns = published_columns(&client, table, None).await;
    let anchor = columns[0].clone();

    for (index, (case, statement)) in [
        (
            "an alias of a select item, named by an ORDER BY",
            format!("SELECT {anchor} AS picked FROM {table} ORDER BY picked"),
        ),
        (
            "an alias of an aggregate, named by an ORDER BY",
            format!("SELECT COUNT({anchor}) AS rows_counted FROM {table} ORDER BY rows_counted"),
        ),
        (
            "a column qualified by a relation alias",
            format!("SELECT t.{anchor} FROM {table} t"),
        ),
        (
            "a column qualified by the table name",
            format!("SELECT {table}.{anchor} FROM {table}"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = export_name(&format!("defined_{index}"));
        let created = client
            .ok(
                "CreateExport",
                &json!({ "Export": export_body(&name, &statement, None) }),
            )
            .await;
        assert!(
            created["ExportArn"].is_string(),
            "case {case:?}: expected the statement to be accepted, got {created}"
        );
    }

    // And an alias is not a licence for the rest: a statement that defines one and then
    // names an unpublished column is still refused.
    let statement =
        format!("SELECT {anchor} AS picked FROM {table} WHERE {UNPUBLISHED_COLUMN} = 1");
    let (status, body) = client
        .call(
            "CreateExport",
            &json!({ "Export": export_body(&export_name("defined_and_bad"), &statement, None) }),
        )
        .await;
    assert_column_refused(
        "an alias beside an unpublished column",
        status,
        &body,
        UNPUBLISHED_COLUMN,
    );
}

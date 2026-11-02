// SPDX-License-Identifier: CC0-1.0

//! The SCOPE of a name a `DataQuery.QueryStatement` defines for itself, over the awsJson1_1
//! wire, for both operations that accept an export definition (`CreateExport`,
//! `UpdateExport`).
//!
//! The rule under test: a select item's alias is a name the statement defines for the
//! clauses that come AFTER the select list (and for a query reading that one), not for the
//! expressions of that same select list — so an identifier standing in a select-list
//! expression is a column reference like any other and may be named exactly when some
//! table the statement reads publishes it under the table configuration the request states
//! ("To view the available tables and columns, see the Data Exports table dictionary" — the
//! modelled doc for `CreateExport`'s `Export`, where a table property "[changes] the data
//! and schema of a table" — the modelled doc for `GetTableRequest$TableProperties`).
//! Aliasing a name to itself therefore defines nothing that can resolve it: the statement
//! is refused with the modelled `ValidationException` (HTTP 400) naming the reference as
//! written and `Export.DataQuery.QueryStatement`, exactly as the bare name is — and, for an
//! update, the stored definition is left as it was rather than displaced by a definition
//! this service could never deliver.
//!
//! Every expectation comes from `GetTable` — the service's own published schema for the
//! table and properties the row states — so the suite asserts the two readings of the
//! dictionary AGREE rather than restating a column list here.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the table: every table the catalog publishes, not only the one the issue cited;
//! * WHERE the self-aliased select item sits: the first select item, a later one, a
//!   function argument, a qualified reference, a sub-query's select list, a scalar
//!   sub-query inside a select item, and a pair of select items aliased to each other;
//! * the column: one no table publishes, one the queried table publishes (which stays
//!   accepted), and every column the catalog publishes only while a table property holds a
//!   value — at the value that publishes it and at a published value that does not;
//! * the operation: the create that first stores a definition and the update that
//!   overwrites one.
//!
//! The router, the request signing and the export-definition helpers come from the
//! `export_lifecycle` and `query_column_selection` suites of this same test target.

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::TABLE_CATALOG;

use crate::export_lifecycle::Client;
use crate::query_column_selection::{
    assert_column_refused, export_body, export_name, gated_columns, published_columns,
    UNPUBLISHED_COLUMN,
};

/// The table the update half of the rule is asserted over: the one the issue cited, which
/// is also the table that publishes property-gated columns.
const COST_AND_USAGE_REPORT: &str = "COST_AND_USAGE_REPORT";

/// The statements that alias `name` TO ITSELF — one per place such a select item can sit,
/// all over `table` and all otherwise valid, so the self-alias is the only thing that can
/// decide the answer. `anchor` is a column the configuration in force publishes, which
/// carries the rest of each statement.
///
/// Each row is `(where the self-alias sits, the statement, the reference AS WRITTEN)`: a
/// refusal names what the caller wrote, which for a qualified reference is the qualifier
/// and the column together.
fn statements_aliasing_to_itself(
    table: &str,
    name: &str,
    anchor: &str,
) -> Vec<(String, String, String)> {
    vec![
        (
            "a select item aliased to itself".to_string(),
            format!("SELECT {name} AS {name} FROM {table}"),
            name.to_string(),
        ),
        (
            "a second select item aliased to itself".to_string(),
            format!("SELECT {anchor}, {name} AS {name} FROM {table}"),
            name.to_string(),
        ),
        (
            "a function argument under the same alias".to_string(),
            format!("SELECT COUNT({name}) AS {name} FROM {table}"),
            name.to_string(),
        ),
        (
            "a qualified select item aliased to itself".to_string(),
            format!("SELECT t.{name} AS {name} FROM {table} t"),
            format!("t.{name}"),
        ),
        (
            "a sub-query select item aliased to itself".to_string(),
            format!(
                "SELECT {anchor} FROM {table} WHERE {anchor} IN \
                 (SELECT {name} AS {name} FROM {table})"
            ),
            name.to_string(),
        ),
        (
            "a scalar sub-query's select item aliased to itself".to_string(),
            format!("SELECT (SELECT {name} AS {name} FROM {table}) AS {anchor} FROM {table}"),
            name.to_string(),
        ),
        (
            "two select items aliased to each other".to_string(),
            format!("SELECT {anchor} AS {name}, {name} AS {anchor} FROM {table}"),
            name.to_string(),
        ),
    ]
}

/// How many exports the store holds.
async fn export_count(client: &Client) -> usize {
    let listed = client.ok("ListExports", &json!({})).await;
    listed["Exports"].as_array().map(Vec::len).unwrap_or_default()
}

/// Create the export `statement` defines under `configuration`, and assert it is accepted
/// and reported back by `GetExport` — the round trip that proves an accepted self-alias is
/// persisted rather than merely answered 200.
async fn assert_accepted(
    client: &Client,
    case: &str,
    name: &str,
    statement: &str,
    configuration: Option<Value>,
) {
    let created = client
        .ok(
            "CreateExport",
            &json!({ "Export": export_body(name, statement, configuration) }),
        )
        .await;
    let arn = created["ExportArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateExport must return an ExportArn, got {created}")
        })
        .to_string();
    let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
    assert_eq!(
        fetched["Export"]["DataQuery"]["QueryStatement"].as_str(),
        Some(statement),
        "case {case:?}: GetExport must report the accepted statement, got {fetched}"
    );
}

/// RULE: for every table the catalog publishes, a create whose select list aliases a name
/// that table does NOT publish to that same name is refused with the modelled 400 naming
/// the reference — wherever that select item sits — while the same shape over a column the
/// table publishes is accepted and read back.
#[tokio::test]
async fn a_self_alias_does_not_define_away_the_column_check() {
    let client = Client::new();
    let mut stored = 0;

    for (table_index, table) in TABLE_CATALOG.iter().enumerate() {
        // The published schema for this table under its own defaults, as GetTable reports
        // it: the first column carries the statements, the last is the one aliased to
        // itself in the accepted rows.
        let columns = published_columns(&client, table.name, None).await;
        assert!(
            columns.len() > 1,
            "case {:?}: the published schema must carry columns, got {columns:?}",
            table.name
        );
        let anchor = columns[0].clone();
        let published = columns[columns.len() - 1].clone();
        assert!(
            !columns.iter().any(|column| column == UNPUBLISHED_COLUMN),
            "case {:?}: the refused rows' column must be unpublished, got {columns:?}",
            table.name
        );

        for (index, (label, statement, written)) in
            statements_aliasing_to_itself(table.name, UNPUBLISHED_COLUMN, &anchor)
                .into_iter()
                .enumerate()
        {
            let case = format!("{} does not publish {UNPUBLISHED_COLUMN} in {label}", table.name);
            let (status, body) = client
                .call(
                    "CreateExport",
                    &json!({
                        "Export": export_body(
                            &export_name(&format!("self_bad_{table_index}_{index}")),
                            &statement,
                            None,
                        ),
                    }),
                )
                .await;
            assert_column_refused(&case, status, &body, &written);
            assert_eq!(
                export_count(&client).await,
                stored,
                "case {case:?}: a refused create must store nothing"
            );
        }

        for (index, (label, statement, _written)) in
            statements_aliasing_to_itself(table.name, &published, &anchor)
                .into_iter()
                .enumerate()
        {
            let case = format!("{} publishes {published} in {label}", table.name);
            assert_accepted(
                &client,
                &case,
                &export_name(&format!("self_ok_{table_index}_{index}")),
                &statement,
                None,
            )
            .await;
            stored += 1;
        }
    }
}

/// RULE, the gating half: a column the dictionary publishes only while a table property
/// holds a value is no more nameable aliased to itself than bare — refused at a published
/// value that does not publish it, accepted at the value that does, with `GetTable`
/// reporting the same column set for the same configuration.
#[tokio::test]
async fn a_self_alias_of_a_gated_column_follows_the_configuration() {
    let client = Client::new();
    let mut rows = 0;

    for table in TABLE_CATALOG {
        for (index, (column, property, publishing, excluding)) in
            gated_columns(table).into_iter().enumerate()
        {
            rows += 1;
            let configuration = |value: &str| json!({ property: value });
            let statement = format!("SELECT {column} AS {column} FROM {}", table.name);

            // At the value that publishes it: GetTable reports the column, and the
            // self-aliased statement is accepted and read back.
            let case = format!(
                "{}.{column} aliased to itself with {property} = {publishing}",
                table.name
            );
            let with =
                published_columns(&client, table.name, Some(&configuration(publishing))).await;
            assert!(
                with.iter().any(|name| name == column),
                "case {case:?}: GetTable must publish the column at this value, got {with:?}"
            );
            assert_accepted(
                &client,
                &case,
                &export_name(&format!("self_gated_on_{index}_{}", table.name)),
                &statement,
                Some(json!({ table.name: configuration(publishing) })),
            )
            .await;

            // At a published value that does not: GetTable omits the column, and the same
            // statement is refused naming it.
            let case = format!(
                "{}.{column} aliased to itself with {property} = {excluding}",
                table.name
            );
            let without =
                published_columns(&client, table.name, Some(&configuration(excluding))).await;
            assert!(
                !without.iter().any(|name| name == column),
                "case {case:?}: GetTable must omit the column at this value, got {without:?}"
            );
            let (status, body) = client
                .call(
                    "CreateExport",
                    &json!({
                        "Export": export_body(
                            &export_name(&format!("self_gated_off_{index}_{}", table.name)),
                            &statement,
                            Some(json!({ table.name: configuration(excluding) })),
                        ),
                    }),
                )
                .await;
            assert_column_refused(&case, status, &body, column);
        }
    }

    assert!(
        rows > 0,
        "case \"gated columns\": the catalog must publish at least one property-gated \
         column to assert the rule on"
    );
}

/// RULE for `UpdateExport`, which "[updates] an existing data export by overwriting all
/// export parameters": a replacement whose select list aliases an unservable name to itself
/// is judged exactly as a create's is, so it is refused and the stored definition — with
/// the instant it was last updated — is left untouched; a replacement whose self-alias IS
/// servable, and one naming an alias from a later clause, replace it and are observable.
#[tokio::test]
async fn an_update_refuses_a_self_aliased_unservable_replacement_and_keeps_the_stored_definition() {
    let client = Client::new();
    let table = COST_AND_USAGE_REPORT;
    let columns = published_columns(&client, table, None).await;
    let anchor = columns[0].clone();
    let published = columns[columns.len() - 1].clone();

    let created = client
        .ok(
            "CreateExport",
            &json!({
                "Export": export_body(
                    &export_name("self_aliased_update"),
                    &format!("SELECT {anchor} FROM {table}"),
                    None,
                ),
            }),
        )
        .await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    // The stored definition carries the table configuration in force, which an update has
    // to restate — so every update below submits the definition it read back, with only
    // the statement (and, for the gated row, the configuration) substituted.
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

    let gated = gated_columns(
        TABLE_CATALOG
            .iter()
            .find(|candidate| candidate.name == table)
            .expect("the catalog publishes the cost and usage table"),
    );
    assert!(
        !gated.is_empty(),
        "case \"gated replacement\": the cost and usage table must publish a property-gated column"
    );
    let (gated_column, gating_property, publishing_value, excluding_value) = gated[0];

    let mut rows: Vec<(String, String, Option<Value>, String)> = Vec::new();
    for (label, statement, written) in
        statements_aliasing_to_itself(table, UNPUBLISHED_COLUMN, &anchor)
    {
        rows.push((
            format!("the replacement aliases {UNPUBLISHED_COLUMN} to itself in {label}"),
            statement,
            None,
            written,
        ));
    }
    rows.push((
        format!(
            "the replacement aliases {gated_column} to itself with \
             {gating_property} = {excluding_value}"
        ),
        format!("SELECT {gated_column} AS {gated_column} FROM {table}"),
        Some(json!({ gating_property: excluding_value })),
        gated_column.to_string(),
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

    // The other half, so the rule is a SCOPE and not a ban on aliases: a self-alias of a
    // published column, the gated column's self-alias under the configuration that
    // publishes it, and an alias named by a later clause of the same statement all replace
    // the stored definition and are observable.
    for (case, statement, configuration) in [
        (
            format!("a self-alias of the published {published}"),
            format!("SELECT {published} AS {published} FROM {table}"),
            None,
        ),
        (
            format!("{gated_column} aliased to itself with {gating_property} = {publishing_value}"),
            format!("SELECT {gated_column} AS {gated_column} FROM {table}"),
            Some(json!({ gating_property: publishing_value })),
        ),
        (
            "an alias named by a later clause".to_string(),
            format!("SELECT {anchor} AS picked FROM {table} ORDER BY picked"),
            None,
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

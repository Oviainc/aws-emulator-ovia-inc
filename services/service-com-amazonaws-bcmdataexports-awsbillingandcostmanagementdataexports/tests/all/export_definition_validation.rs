// SPDX-License-Identifier: CC0-1.0

//! The export DEFINITION a mutating operation accepts, over the awsJson1_1 wire.
//!
//! Two rules are under test.
//!
//! 1. A `DataQuery.QueryStatement` may only name tables this service PUBLISHES — the
//!    catalog `ListTables` lists and `GetTable` describes. "The QueryStatement is an SQL
//!    statement… To view the available tables and columns, see the Data Exports table
//!    dictionary" (the modelled doc for `CreateExport`'s `Export`). A statement naming
//!    anything else, or one that is not readable SQL at all, is refused with the modelled
//!    `ValidationException` (HTTP 400) naming the offending table, and NOTHING is stored.
//!
//! 2. `UpdateExport` "[updates] an existing data export by overwriting all export
//!    parameters. All export parameters must be specified in the UpdateExport request"
//!    (smithy_query operation `UpdateExport`), so an update whose query names a table that
//!    publishes table properties must restate that table's `TableConfigurations` entry:
//!    omitting it is refused with `ValidationException` (400) and leaves the stored
//!    definition untouched, rather than silently keeping or clearing the per-table
//!    properties. A `CreateExport` has no prior definition to overwrite, so it does not
//!    owe them.
//!
//! Both are asserted over TABLES of inputs that differ along what the rules generalise
//! over: every table the catalog publishes (not only the one the issue cited), table
//! names that miss the catalog in several ways, and the places a statement can name a
//! table — a bare `FROM`, a join, a sub-query — plus statements that are not SQL.

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::TABLE_CATALOG;

use crate::export_lifecycle::{cases, error_code, Client};

/// The `Export` a row submits: a definition that satisfies every modelled constraint,
/// with the name, the query statement and the table configurations substituted.
fn export_body(name: &str, query_statement: &str, table_configurations: Option<Value>) -> Value {
    let mut export = cases().remove(0).create_request()["Export"].clone();
    let object = export.as_object_mut().expect("Export is a JSON object");
    object.insert("Name".to_string(), json!(name));
    let mut data_query = Map::new();
    data_query.insert("QueryStatement".to_string(), json!(query_statement));
    if let Some(configurations) = table_configurations {
        data_query.insert("TableConfigurations".to_string(), configurations);
    }
    object.insert("DataQuery".to_string(), Value::Object(data_query));
    export
}

/// An export name derived from `label`, inside `Export.Name`'s `@pattern
/// ^[0-9A-Za-z\-_]+$` (smithy_query shape `Export`, member `Name`).
fn export_name(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// The `TableConfigurations` naming `table` with the catalog's own first property for it,
/// or an empty property map for a table that publishes none.
fn configurations_for(table: &str) -> Value {
    let spec = TABLE_CATALOG
        .iter()
        .find(|candidate| candidate.name == table)
        .expect("the row names a published table");
    let mut properties = Map::new();
    if let Some(property) = spec.properties.first() {
        let value = property
            .default_value()
            .or_else(|| property.valid_values.first().copied())
            .unwrap_or("");
        properties.insert(property.name.to_string(), json!(value));
    }
    json!({ table: Value::Object(properties) })
}

/// Statements that name a table this service does not publish, paired with the table the
/// refusal must name. They differ along WHERE the offending name sits: a bare `FROM`, a
/// join beside a published table, and a sub-query.
fn statements_naming_an_unpublished_table() -> Vec<(&'static str, &'static str, String)> {
    vec![
        (
            "the recorded bare FROM",
            "NOT_A_REAL_TABLE_XYZ",
            "SELECT some_column FROM NOT_A_REAL_TABLE_XYZ".to_string(),
        ),
        (
            "a published name in the wrong case",
            "cost_and_usage_report",
            "SELECT identity_line_item_id FROM cost_and_usage_report".to_string(),
        ),
        (
            "a name built from a published one",
            "COST_AND_USAGE_REPORT_V3",
            "SELECT identity_line_item_id FROM COST_AND_USAGE_REPORT_V3".to_string(),
        ),
        (
            "an unpublished table joined to a published one",
            "GHOST_TABLE",
            "SELECT a.identity_line_item_id FROM COST_AND_USAGE_REPORT a \
             JOIN GHOST_TABLE b ON a.identity_line_item_id = b.id"
                .to_string(),
        ),
        (
            "an unpublished table inside a sub-query",
            "INNER_GHOST",
            "SELECT identity_line_item_id FROM COST_AND_USAGE_REPORT WHERE \
             identity_line_item_id IN (SELECT id FROM INNER_GHOST)"
                .to_string(),
        ),
    ]
}

/// How many exports the store holds.
async fn export_count(client: &Client) -> usize {
    let listed = client.ok("ListExports", &json!({})).await;
    listed["Exports"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

#[tokio::test]
async fn a_create_accepts_only_a_query_over_published_tables() {
    let client = Client::new();

    // Every table the catalog publishes is a table a query may name, and the stored
    // definition reads back through GetExport.
    let mut created = 0;
    for table in TABLE_CATALOG {
        let name = export_name(table.name);
        let statement = format!("SELECT * FROM {}", table.name);
        let response = client
            .ok(
                "CreateExport",
                &json!({ "Export": export_body(&name, &statement, None) }),
            )
            .await;
        created += 1;
        let arn = response["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: CreateExport must return an ExportArn"))
            .to_string();
        let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            fetched["Export"]["DataQuery"]["QueryStatement"].as_str(),
            Some(statement.as_str()),
            "case {name:?}: GetExport must report the query that was stored, got {fetched}"
        );
        assert_eq!(
            export_count(&client).await,
            created,
            "case {name:?}: an accepted create must be listed"
        );
    }

    // A query over two published tables is accepted too: the rule is about which tables a
    // statement names, not about how many.
    let two_tables = format!(
        "SELECT a.* FROM {} a JOIN {} b ON a.payer_account_id = b.bill_payer_account_id",
        TABLE_CATALOG[0].name, "COST_AND_USAGE_REPORT"
    );
    client
        .ok(
            "CreateExport",
            &json!({ "Export": export_body("two_published_tables", &two_tables, None) }),
        )
        .await;
    created += 1;

    // And every statement naming a table outside the catalog is refused, naming it.
    for (label, offending, statement) in statements_naming_an_unpublished_table() {
        let (status, body) = client
            .call(
                "CreateExport",
                &json!({ "Export": export_body(&export_name(label), &statement, None) }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(format!("Table name {offending} is invalid").as_str()),
            "case {label:?}: the refusal must name the offending table, got {body}"
        );
        assert_eq!(
            export_count(&client).await,
            created,
            "case {label:?}: a refused create must store nothing"
        );
    }

    // A statement that is not readable SQL names no table this service could serve.
    for statement in ["not sql at all", "SELECT FROM", "DROP"] {
        let (status, body) = client
            .call(
                "CreateExport",
                &json!({ "Export": export_body("unreadable_query", statement, None) }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {statement:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        assert_eq!(
            export_count(&client).await,
            created,
            "case {statement:?}: a refused create must store nothing"
        );
    }

    // A QUOTED published name is the same table: the identifier's value is what resolves.
    client
        .ok(
            "CreateExport",
            &json!({
                "Export": export_body(
                    "quoted_table_name",
                    "SELECT identity_line_item_id FROM \"COST_AND_USAGE_REPORT\"",
                    None,
                ),
            }),
        )
        .await;
    created += 1;

    // A QUALIFIED name is not the published table its last part spells: data exports
    // publishes one catalog, not a schema a caller can qualify into.
    for qualified in [
        "other_db.COST_AND_USAGE_REPORT",
        "awsbilling.public.COST_AND_USAGE_REPORT",
    ] {
        let (status, body) = client
            .call(
                "CreateExport",
                &json!({
                    "Export": export_body(
                        "qualified_table_name",
                        &format!("SELECT identity_line_item_id FROM {qualified}"),
                        None,
                    ),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {qualified:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(format!("Table name {qualified} is invalid").as_str()),
            "case {qualified:?}: the refusal must name the qualified relation, got {body}"
        );
    }

    // A statement naming more distinct tables than this service resolves is refused on
    // that bound, before any of them is looked up — the work is bounded by the service
    // and not by the request. 64 is the bound; 64 distinct names are accepted as far as
    // the catalog check, which then refuses the first unpublished one.
    for (label, count, expected_message) in [
        ("at the relation bound", 64, "Table name t0 is invalid".to_string()),
        (
            "one past the relation bound",
            65,
            "QueryStatement must not name more than 64 tables".to_string(),
        ),
    ] {
        let relations: Vec<String> = (0..count).map(|index| format!("t{index}")).collect();
        let statement = format!("SELECT 1 FROM {}", relations.join(", "));
        let (status, body) = client
            .call(
                "CreateExport",
                &json!({ "Export": export_body("many_relations", &statement, None) }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(expected_message.as_str()),
            "case {label:?}: expected {expected_message:?}, got {body}"
        );
    }

    // A statement nested deeper than this service reads is refused before the parser is
    // entered, and one at the bound is read normally: the bound is on the DEPTH a parse
    // and the walk over its tree descend through, which the statement's own bracketing
    // states. The deep rows are built from a published table, so what refuses them is the
    // depth and nothing else.
    for (label, depth, expected_message) in [
        (
            "at the nesting bound",
            32,
            "Table name COST_AND_USAGE_REPORT_DEEP is invalid".to_string(),
        ),
        (
            "one past the nesting bound",
            33,
            "QueryStatement must not be nested more than 32 levels deep".to_string(),
        ),
    ] {
        // `SELECT 1 FROM COST_AND_USAGE_REPORT_DEEP WHERE 1 IN ((((…1…))))`, bracketed to
        // exactly `depth`.
        let statement = format!(
            "SELECT 1 FROM COST_AND_USAGE_REPORT_DEEP WHERE 1 IN {}1{}",
            "(".repeat(depth),
            ")".repeat(depth)
        );
        let (status, body) = client
            .call(
                "CreateExport",
                &json!({ "Export": export_body("deep_query", &statement, None) }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(expected_message.as_str()),
            "case {label:?}: expected {expected_message:?}, got {body}"
        );
    }

    // A common table expression is outside the published subset: the catalog publishes no
    // `t`, so the statement is refused like any other naming an unpublished table.
    let (status, body) = client
        .call(
            "CreateExport",
            &json!({
                "Export": export_body(
                    "cte_query",
                    "WITH t AS (SELECT identity_line_item_id FROM COST_AND_USAGE_REPORT) \
                     SELECT * FROM t",
                    None,
                ),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), body["Message"].as_str()),
        (400, "ValidationException", Some("Table name t is invalid")),
        "a CTE name is not a published table, got {status} with {body}"
    );

    // A BRACKET-FREE deep statement: a long chain of binary predicates nests the parsed
    // tree at bracket depth zero, so this is the shape the bracket bound does not see. It
    // is read on the sized stack and served, because every table it names is published —
    // the point being that a deep tree is handled rather than left to the worker thread's
    // ordinary stack.
    let chained = format!(
        "SELECT 1 FROM COST_AND_USAGE_REPORT WHERE {}1=1",
        "1=1 AND ".repeat(500)
    );
    client
        .ok(
            "CreateExport",
            &json!({ "Export": export_body("chained_predicates", &chained, None) }),
        )
        .await;
    created += 1;

    // And a statement past the TOKEN bound is refused before the parser is entered, which
    // is what bounds the depth of a chain like the one above.
    let over_token_bound = format!(
        "SELECT 1 FROM COST_AND_USAGE_REPORT WHERE {}1=1",
        "1=1 AND ".repeat(2000)
    );
    let (status, body) = client
        .call(
            "CreateExport",
            &json!({ "Export": export_body("too_many_tokens", &over_token_bound, None) }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), body["Message"].as_str()),
        (
            400,
            "ValidationException",
            Some("QueryStatement must not carry more than 4096 tokens"),
        ),
        "a statement past the token bound must be refused, got {status} with {body}"
    );

    assert_eq!(
        export_count(&client).await,
        created,
        "every refused create must have stored nothing"
    );
}

#[tokio::test]
async fn an_update_must_restate_the_table_configurations_it_overwrites() {
    let client = Client::new();

    for table in TABLE_CATALOG {
        let name = export_name(table.name);
        let statement = format!("SELECT * FROM {}", table.name);
        let configurations = configurations_for(table.name);
        let created = client
            .ok(
                "CreateExport",
                &json!({
                    "Export": export_body(&name, &statement, Some(configurations.clone())),
                }),
            )
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: CreateExport must return an ExportArn"))
            .to_string();
        let before = client.ok("GetExport", &json!({ "ExportArn": arn })).await;

        // The update that drops TableConfigurations entirely: refused, and the stored
        // definition is exactly what it was.
        let (status, body) = client
            .call(
                "UpdateExport",
                &json!({
                    "ExportArn": arn,
                    "Export": export_body(&name, &statement, None),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {name:?}: an update without TableConfigurations must be refused, got {status} with {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(
                format!(
                    "'TableConfigurations' must be provided when updating a {} export",
                    table.name
                )
                .as_str()
            ),
            "case {name:?}: the refusal must name the table whose configuration is missing, got {body}"
        );
        let after_refusal = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            after_refusal, before,
            "case {name:?}: a refused update must leave the stored definition untouched"
        );

        // An update whose TableConfigurations names a DIFFERENT table does not restate the
        // queried table's own entry either.
        let other = TABLE_CATALOG
            .iter()
            .find(|candidate| candidate.name != table.name)
            .expect("the catalog publishes more than one table");
        let (status, body) = client
            .call(
                "UpdateExport",
                &json!({
                    "ExportArn": arn,
                    "Export": export_body(
                        &name,
                        &statement,
                        Some(configurations_for(other.name)),
                    ),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {name:?}: TableConfigurations for another table must not satisfy the rule, got {status} with {body}"
        );

        // Restating them is accepted, and the update is observable.
        let restated = format!("SELECT * FROM {} LIMIT 10", table.name);
        client
            .ok(
                "UpdateExport",
                &json!({
                    "ExportArn": arn,
                    "Export": export_body(&name, &restated, Some(configurations.clone())),
                }),
            )
            .await;
        let after_update = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            after_update["Export"]["DataQuery"]["QueryStatement"].as_str(),
            Some(restated.as_str()),
            "case {name:?}: GetExport must observe the accepted update, got {after_update}"
        );

        // An update whose query names an unpublished table is refused on that ground
        // first, whatever it says about table configurations.
        let (status, body) = client
            .call(
                "UpdateExport",
                &json!({
                    "ExportArn": arn,
                    "Export": export_body(
                        &name,
                        "SELECT some_column FROM NOT_A_REAL_TABLE_XYZ",
                        Some(configurations),
                    ),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {name:?}: an update naming an unpublished table must be refused, got {status} with {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some("Table name NOT_A_REAL_TABLE_XYZ is invalid"),
            "case {name:?}: the refusal must name the offending table, got {body}"
        );
    }
}

/// A create has no prior definition to overwrite, so it owes no `TableConfigurations` —
/// the asymmetry the update rule rests on.
#[tokio::test]
async fn a_create_without_table_configurations_is_accepted() {
    let client = Client::new();

    for table in TABLE_CATALOG {
        let name = format!("create_{}", export_name(table.name));
        let created = client
            .ok(
                "CreateExport",
                &json!({
                    "Export": export_body(
                        &name,
                        &format!("SELECT * FROM {}", table.name),
                        None,
                    ),
                }),
            )
            .await;
        assert!(
            created["ExportArn"].is_string(),
            "case {name:?}: a create without TableConfigurations must be accepted, got {created}"
        );
    }
}

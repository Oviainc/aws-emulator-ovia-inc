// SPDX-License-Identifier: CC0-1.0

//! The SQL SUBSET a `DataQuery.QueryStatement` has to be in, over the awsJson1_1 wire,
//! for both operations that accept an export definition (`CreateExport`, `UpdateExport`).
//!
//! The rule under test: a query statement has to be ONE SQL statement, that statement a
//! query, and every relation it reads a table the catalog publishes — "The QueryStatement
//! is an SQL statement. Data Exports only supports a limited subset of the SQL syntax. …
//! To view the available tables and columns, see the Data Exports table dictionary"
//! (smithy_query shape `CreateExportRequest`, member `Export`). A statement this service
//! cannot read that way selects nothing it could ever deliver, so it is refused with the
//! modelled `ValidationException` (HTTP 400) naming `Export.DataQuery.QueryStatement`, and
//! NOTHING is persisted — where a statement it can read is accepted and read back
//! verbatim.
//!
//! The refusal rows differ along WHY the statement is outside the subset, which is what
//! the rule generalises over: text the parser cannot read at all (the filed instance),
//! a statement of a kind that is not a query (`DELETE`, `DROP`, `INSERT`, `UPDATE`,
//! `TRUNCATE`, `CREATE … AS SELECT`, `CREATE VIEW`, `EXPLAIN`) — each of them over a
//! PUBLISHED table, so the statement's kind is the only thing that can refuse it — more
//! than one statement, no statement at all, and a query that reads no table.
//!
//! The acceptance rows are the other half, and they are what keeps the refusal from being
//! wider than the service: every table the catalog publishes, and the shapes a data query
//! is written in — a projection, a `WHERE`, a join, a sub-query, a quoted name, keywords
//! in lower case, an `ORDER BY`/`LIMIT`, a `GROUP BY` and a set operation. The last two
//! are shapes the generic dialect reads and the published subset does not name: this
//! service refuses what no data export could deliver, not everything the documentation
//! leaves unsaid.
//!
//! WHICH table names are published is `export_definition_validation`'s rule, not this
//! suite's; the rows here vary the statement's FORM over published tables.

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::TABLE_CATALOG;

use crate::export_lifecycle::{cases, error_code, Client, NOT_FOUND_STATUS};

/// The modelled input member every refusal of a query statement names
/// (`DataQuery$QueryStatement`, reached through `Export$DataQuery`).
const QUERY_STATEMENT_FIELD: &str = "Export.DataQuery.QueryStatement";

/// The `ValidationExceptionReason` a refusal of one named input member reports — one of
/// the modelled values (smithy_query shape `ValidationExceptionReason`: `cannotParse`,
/// `fieldValidationFailed`, `other`, `unknownOperation`).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// A published table every row that is not about table names is written over, so what
/// refuses such a row is its FORM and nothing else.
const PUBLISHED_TABLE: &str = "COST_AND_USAGE_REPORT";

/// A second published table, for the rows that read two of them.
const SECOND_PUBLISHED_TABLE: &str = "COST_OPTIMIZATION_RECOMMENDATIONS";

/// A `CreateExport` body for `name` whose `DataQuery.QueryStatement` is `statement`,
/// built on a definition from the lifecycle table that satisfies every other modelled
/// constraint — so the statement is the only thing a row varies.
fn create_request(name: &str, statement: &str) -> Value {
    let mut request = cases().remove(0).create_request();
    let export = request["Export"]
        .as_object_mut()
        .expect("the Export structure is a JSON object");
    export.insert("Name".to_string(), json!(name));
    let mut data_query = Map::new();
    data_query.insert("QueryStatement".to_string(), json!(statement));
    export.insert("DataQuery".to_string(), Value::Object(data_query));
    request
}

/// An export name derived from `label`, inside `Export.Name`'s `@pattern
/// ^[0-9A-Za-z\-_]+$` and `@length(1, 128)` (smithy_query shape `Export`, member `Name`).
fn export_name(label: &str) -> String {
    let sanitised: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(100)
        .collect();
    format!("q_{sanitised}")
}

/// The statements this service cannot read as a data query, each with the reason it is
/// outside the published subset. Every row that is not about a table name reads
/// [`PUBLISHED_TABLE`], so nothing but the row's own reason can refuse it.
fn statements_outside_the_subset() -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = vec![
        // Text the parser cannot read at all — the filed instance first.
        ("not a SQL statement at all", "not sql at all".to_string()),
        ("a SELECT with no select list", "SELECT FROM".to_string()),
        ("a bare keyword", "DROP".to_string()),
        (
            "a FROM with no relation",
            "SELECT identity_line_item_id FROM".to_string(),
        ),
        (
            "a WHERE with no predicate",
            format!("SELECT identity_line_item_id FROM {PUBLISHED_TABLE} WHERE"),
        ),
        (
            "an unterminated string literal",
            format!("SELECT 'unterminated FROM {PUBLISHED_TABLE}"),
        ),
        ("a JSON document", "{\"Query\": \"everything\"}".to_string()),
        // Statements of a kind that is not a query: each reads or writes a PUBLISHED
        // table, so the kind is the only ground of refusal. The first four would CHANGE
        // the catalog, which no data export does.
        (
            "a DELETE over a published table",
            format!("DELETE FROM {PUBLISHED_TABLE}"),
        ),
        (
            "a DROP TABLE of a published table",
            format!("DROP TABLE {PUBLISHED_TABLE}"),
        ),
        (
            "an INSERT into a published table",
            format!("INSERT INTO {PUBLISHED_TABLE} (identity_line_item_id) VALUES ('x')"),
        ),
        (
            "an UPDATE of a published table",
            format!("UPDATE {PUBLISHED_TABLE} SET identity_line_item_id = 'x'"),
        ),
        (
            "a TRUNCATE of a published table",
            format!("TRUNCATE TABLE {PUBLISHED_TABLE}"),
        ),
        (
            "a CREATE TABLE AS SELECT over a published table",
            format!("CREATE TABLE copy AS SELECT identity_line_item_id FROM {PUBLISHED_TABLE}"),
        ),
        (
            "a CREATE VIEW over a published table",
            format!("CREATE VIEW v AS SELECT identity_line_item_id FROM {PUBLISHED_TABLE}"),
        ),
        (
            "an EXPLAIN of a query over a published table",
            format!("EXPLAIN SELECT identity_line_item_id FROM {PUBLISHED_TABLE}"),
        ),
        // More than one statement, and none at all.
        (
            "two queries over published tables",
            format!(
                "SELECT identity_line_item_id FROM {PUBLISHED_TABLE}; \
                 SELECT recommendation_id FROM {SECOND_PUBLISHED_TABLE}"
            ),
        ),
        (
            "a query followed by a DELETE",
            format!(
                "SELECT identity_line_item_id FROM {PUBLISHED_TABLE}; \
                 DELETE FROM {PUBLISHED_TABLE}"
            ),
        ),
        ("one statement separator", ";".to_string()),
        ("nothing but separators", ";;".to_string()),
        ("nothing but whitespace", "   ".to_string()),
        // A query that reads no relation: over the request's own literals, so there is no
        // published table for the export to deliver.
        ("a query over a literal", "SELECT 1".to_string()),
        (
            "a query over a named literal",
            "SELECT 'literal' AS value".to_string(),
        ),
        ("a VALUES list", "VALUES (1)".to_string()),
    ];
    // A row per catalog table whose statement asks to DROP it, so the statement rule is
    // asserted over every published table and not only the one cited above.
    for table in TABLE_CATALOG {
        rows.push((
            "a DROP TABLE of each published table",
            format!("DROP TABLE {}", table.name),
        ));
    }
    rows
}

/// The statements this service CAN read as a data query, each over published tables only.
fn statements_inside_the_subset() -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    // Every table the catalog publishes, selected whole.
    for table in TABLE_CATALOG {
        rows.push((
            "every published table, selected whole",
            format!("SELECT * FROM {}", table.name),
        ));
    }
    rows.extend([
        (
            "a projection",
            format!(
                "SELECT identity_line_item_id, line_item_unblended_cost FROM {PUBLISHED_TABLE}"
            ),
        ),
        (
            "a projection with a filter",
            format!(
                "SELECT identity_line_item_id FROM {PUBLISHED_TABLE} \
                 WHERE line_item_usage_start_date > '2026-01-01'"
            ),
        ),
        (
            "a join of two published tables",
            format!(
                "SELECT a.identity_line_item_id FROM {PUBLISHED_TABLE} a \
                 JOIN {SECOND_PUBLISHED_TABLE} b ON a.line_item_usage_account_id = b.account_id"
            ),
        ),
        (
            "a sub-query over a published table",
            format!(
                "SELECT identity_line_item_id FROM {PUBLISHED_TABLE} WHERE \
                 line_item_usage_account_id IN (SELECT account_id FROM {SECOND_PUBLISHED_TABLE})"
            ),
        ),
        (
            "a quoted relation name",
            format!("SELECT identity_line_item_id FROM \"{PUBLISHED_TABLE}\""),
        ),
        (
            "keywords in lower case",
            format!("select identity_line_item_id from {PUBLISHED_TABLE}"),
        ),
        (
            "an ORDER BY and a LIMIT",
            format!(
                "SELECT identity_line_item_id FROM {PUBLISHED_TABLE} \
                 ORDER BY identity_line_item_id LIMIT 10"
            ),
        ),
        // The last two are shapes the generic dialect reads and the published subset does
        // not name: they are served because every relation they read is published.
        (
            "a GROUP BY",
            format!(
                "SELECT line_item_usage_account_id, SUM(line_item_unblended_cost) \
                 FROM {PUBLISHED_TABLE} GROUP BY line_item_usage_account_id"
            ),
        ),
        (
            "a set operation over two published tables",
            format!(
                "SELECT line_item_usage_account_id FROM {PUBLISHED_TABLE} \
                 UNION SELECT account_id FROM {SECOND_PUBLISHED_TABLE}"
            ),
        ),
        (
            "a trailing statement separator",
            format!("SELECT identity_line_item_id FROM {PUBLISHED_TABLE};"),
        ),
    ]);
    rows
}

/// Assert that `body` is the modelled refusal of `statement`: HTTP 400,
/// `ValidationException`, a `Message` built from the request that echoes the statement,
/// the field-failure reason, and `Export.DataQuery.QueryStatement` as the named field.
fn assert_refused(case: &str, status: u16, body: &Value, statement: &str) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(format!("QueryStatement {statement} is invalid").as_str()),
        "case {case:?}: the refusal must be built from the submitted statement, got {body}"
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

/// The ARN `CreateExport` mints for `name` in the region the harness signs for — what a
/// refused create must leave naming nothing.
fn export_arn(name: &str) -> String {
    engine::emulator_arn("bcm-data-exports", "us-east-1", &format!("export/{name}"))
}

/// RULE: a `CreateExport` whose query statement this service cannot read as a single query
/// over published tables is refused with the modelled 400 naming the statement, and
/// nothing is persisted.
#[tokio::test]
async fn a_create_refuses_a_query_statement_outside_the_published_subset() {
    let client = Client::new();

    for (index, (label, statement)) in statements_outside_the_subset().into_iter().enumerate() {
        let case = format!("{label}: {statement:?}");
        let name = export_name(&format!("{label}_{index}"));
        let (status, body) = client
            .call("CreateExport", &create_request(&name, &statement))
            .await;
        assert_refused(&case, status, &body, &statement);

        // Nothing was stored: the ARN this name would have minted names nothing.
        let (status, body) = client
            .call("GetExport", &json!({ "ExportArn": export_arn(&name) }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {case:?}: a refused create must have stored nothing, got {status} with {body}"
        );
    }

    // And the store is empty of them: no refused row is listed.
    let listed = client.ok("ListExports", &json!({})).await;
    let count = listed["Exports"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(count, 0, "no refused create may be listed, got {listed}");
}

/// RULE, the other half: a statement this service CAN read as a query over published
/// tables is accepted and read back verbatim through `GetExport`.
#[tokio::test]
async fn a_create_accepts_every_statement_the_service_can_read() {
    let client = Client::new();

    for (index, (label, statement)) in statements_inside_the_subset().into_iter().enumerate() {
        let case = format!("{label}: {statement:?}");
        let name = export_name(&format!("{label}_{index}"));
        let created = client
            .ok("CreateExport", &create_request(&name, &statement))
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateExport must return an ExportArn"))
            .to_string();
        let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            fetched["Export"]["DataQuery"]["QueryStatement"].as_str(),
            Some(statement.as_str()),
            "case {case:?}: GetExport must report the statement that was stored, got {fetched}"
        );
    }
}

/// RULE: the same subset governs `UpdateExport` — a statement outside it is refused with
/// the modelled 400 and leaves the stored definition exactly as it was, while one inside
/// it is accepted and observable.
#[tokio::test]
async fn an_update_refuses_a_query_statement_outside_the_published_subset() {
    let client = Client::new();

    let name = export_name("updated_export");
    let accepted = format!("SELECT identity_line_item_id FROM {PUBLISHED_TABLE}");
    let created = client
        .ok("CreateExport", &create_request(&name, &accepted))
        .await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();
    // The stored definition carries the table configuration in force, which an update has
    // to restate — so every update below submits the definition it read back, with only
    // the statement substituted.
    let stored = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
    let definition = stored["Export"].clone();
    let with_statement = |statement: &str| {
        let mut export = definition.clone();
        export["DataQuery"]["QueryStatement"] = json!(statement);
        json!({ "ExportArn": arn, "Export": export })
    };

    for (label, statement) in statements_outside_the_subset() {
        let case = format!("update with {label}: {statement:?}");
        let (status, body) = client
            .call("UpdateExport", &with_statement(&statement))
            .await;
        assert_refused(&case, status, &body, &statement);

        let after = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            after["Export"]["DataQuery"]["QueryStatement"].as_str(),
            Some(accepted.as_str()),
            "case {case:?}: a refused update must leave the stored statement untouched, got {after}"
        );
    }

    // A statement inside the subset over the same table is accepted and observable.
    let next = format!("SELECT identity_line_item_id FROM {PUBLISHED_TABLE} LIMIT 5");
    client.ok("UpdateExport", &with_statement(&next)).await;
    let after = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
    assert_eq!(
        after["Export"]["DataQuery"]["QueryStatement"].as_str(),
        Some(next.as_str()),
        "an accepted update must be observable, got {after}"
    );
}

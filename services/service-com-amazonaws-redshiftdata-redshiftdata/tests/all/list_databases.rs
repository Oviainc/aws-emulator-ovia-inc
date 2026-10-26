// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListDatabases`, driven over the wire: every call below
//! is a real `POST /` with `X-Amz-Target: RedshiftData.ListDatabases`
//! dispatched through the router, so what is asserted is the emulator's HTTP
//! behaviour and not a handler's return value.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one case:
//! `ListDatabases` reports the databases of the data warehouse the request
//! connects to — the databases statements have created there, never those of
//! another warehouse — in name order, paged by `MaxResults` with an opaque
//! `NextToken`, and refuses a connection through a database the warehouse does
//! not hold.

use serde_json::{json, Value};

// The harness every suite in this crate drives the emulator through.
use super::emulator::{error_code, list, Emulator};

/// One row of the warehouse table: a data warehouse, the databases statements
/// create on it, and the databases `ListDatabases` must then report for it.
struct WarehouseCase {
    label: &'static str,
    /// The warehouse and authorization members of every request about it; the
    /// `Database` the request connects through is added per call.
    warehouse: fn() -> Value,
    /// The databases a statement is run against, in the order they are created.
    created: &'static [&'static str],
    /// What `ListDatabases` must report for this warehouse.
    expected: &'static [&'static str],
}

/// The provisioned cluster most of this suite runs against.
fn cluster() -> Value {
    json!({"ClusterIdentifier": "my-cluster", "DbUser": "awsuser"})
}

/// A second provisioned cluster, so that one warehouse's databases can be shown
/// not to be reported for another.
fn other_cluster() -> Value {
    json!({"ClusterIdentifier": "other-cluster", "DbUser": "awsuser"})
}

/// A serverless workgroup: the other kind of data warehouse a request may name.
fn workgroup() -> Value {
    json!({"WorkgroupName": "my-workgroup", "DbUser": "awsuser"})
}

/// A request about `warehouse`, connecting through `database`.
fn connect(warehouse: &Value, database: &str) -> Value {
    let mut request = warehouse.clone();
    request["Database"] = json!(database);
    request
}

/// Create `database` on `warehouse` the only way a database comes into
/// existence here: by running a statement against it.
async fn create_database(emulator: &Emulator, warehouse: &Value, database: &str) {
    let mut request = connect(warehouse, database);
    request["Sql"] = json!("CREATE TABLE marker (id INTEGER)");
    let response = emulator.execute_and_wait(&request).await;
    assert_eq!(
        response["Status"], "FINISHED",
        "case {database:?}: a statement creating the database must finish, got {response}"
    );
}

/// The `Databases` a response reports, as plain strings.
fn reported(response: &Value) -> Vec<String> {
    list(response, "Databases")
        .iter()
        .map(|database| database.as_str().unwrap_or_default().to_string())
        .collect()
}

/// `ListDatabases` reports the databases of the warehouse the request connects
/// to, in name order, and reports no database of any other warehouse.
#[tokio::test]
async fn list_databases_reports_the_databases_of_the_warehouse_it_connects_to() {
    let cases = [
        WarehouseCase {
            label: "a cluster whose databases were created out of name order",
            warehouse: cluster,
            created: &["zeta", "dev", "analytics"],
            expected: &["analytics", "dev", "zeta"],
        },
        WarehouseCase {
            label: "a second cluster holding one database of its own",
            warehouse: other_cluster,
            created: &["dev"],
            expected: &["dev"],
        },
        WarehouseCase {
            label: "a serverless workgroup",
            warehouse: workgroup,
            created: &["dev", "reporting"],
            expected: &["dev", "reporting"],
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        for database in case.created {
            create_database(&emulator, &(case.warehouse)(), database).await;
        }
    }

    for case in &cases {
        let label = case.label;
        let warehouse = (case.warehouse)();
        // Every database of the warehouse is a connection the list can be read
        // through, and each one reports the same set.
        for through in case.created {
            let response = emulator
                .ok("ListDatabases", &connect(&warehouse, through))
                .await;
            assert_eq!(
                reported(&response),
                case.expected
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<Vec<String>>(),
                "case {label}: connected through {through:?}, expected {:?}, got {response}",
                case.expected
            );
            assert!(
                response.get("NextToken").is_none(),
                "case {label}: one page holds every database, got {response}"
            );
        }
    }
}

/// A database created after a list was read is reported by the next one: the
/// response is computed from state at the time of the call.
#[tokio::test]
async fn list_databases_reports_a_database_created_after_an_earlier_list() {
    let emulator = Emulator::new();
    create_database(&emulator, &cluster(), "dev").await;

    let before = emulator.ok("ListDatabases", &connect(&cluster(), "dev")).await;
    assert_eq!(
        reported(&before),
        vec!["dev".to_string()],
        "case first list: expected only the database created so far, got {before}"
    );

    create_database(&emulator, &cluster(), "analytics").await;
    let after = emulator.ok("ListDatabases", &connect(&cluster(), "dev")).await;
    assert_eq!(
        reported(&after),
        vec!["analytics".to_string(), "dev".to_string()],
        "case second list: expected the newly created database too, got {after}"
    );
}

/// `ListDatabases` pages its `Databases` with an opaque `NextToken`, returns
/// every database exactly once across the pages, and refuses a token it never
/// issued — including one issued for another warehouse.
#[tokio::test]
async fn list_databases_pages_its_database_list() {
    let emulator = Emulator::new();
    for database in ["zeta", "dev", "analytics"] {
        create_database(&emulator, &cluster(), database).await;
    }
    create_database(&emulator, &workgroup(), "dev").await;

    // Every page size from one database at a time up to one past the number of
    // databases there are: each one must walk the whole list exactly once.
    for page_size in 1..=4 {
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = connect(&cluster(), "dev");
            request["MaxResults"] = json!(page_size);
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = emulator.ok("ListDatabases", &request).await;
            let databases = reported(&page);
            assert!(
                databases.len() <= page_size,
                "case MaxResults {page_size}, page {pages}: a page holds at most that many \
                 databases, got {page}"
            );
            collected.extend(databases);
            pages += 1;
            match page.get("NextToken").and_then(Value::as_str) {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected,
            vec![
                "analytics".to_string(),
                "dev".to_string(),
                "zeta".to_string()
            ],
            "case MaxResults {page_size}: paging must return every database once, in name order"
        );
        let expected_pages = 3_usize.div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case MaxResults {page_size}: expected {expected_pages} page(s), walked {pages}"
        );
    }

    // A token minted while paging the workgroup's databases is not a token for
    // the cluster's, however valid it is for the warehouse it came from.
    let mut request = connect(&workgroup(), "dev");
    request["MaxResults"] = json!(1);
    create_database(&emulator, &workgroup(), "reporting").await;
    let workgroup_page = emulator.ok("ListDatabases", &request).await;
    let foreign_token = workgroup_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the workgroup's first page must carry a token: {workgroup_page}"))
        .to_string();

    for token in ["not-hexadecimal", "6162636465", foreign_token.as_str()] {
        let mut request = connect(&cluster(), "dev");
        request["NextToken"] = json!(token);
        let (status, response) = emulator.call("ListDatabases", &request).await;
        assert_eq!(
            status, 400,
            "case token {token:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case token {token:?}: expected ValidationException, got {response}"
        );
    }
}

/// A connection through a database this emulator does not hold names a missing
/// resource, and a request that does not name one warehouse, one database and
/// one authorization method is refused as invalid.
#[tokio::test]
async fn list_databases_refuses_a_request_it_cannot_serve() {
    let emulator = Emulator::new();
    create_database(&emulator, &cluster(), "dev").await;

    let missing: [(&str, Value); 2] = [
        (
            "a database no statement has ever run against",
            connect(&cluster(), "never-used"),
        ),
        (
            "a warehouse no statement has ever run against",
            connect(&json!({"ClusterIdentifier": "empty-cluster", "DbUser": "awsuser"}), "dev"),
        ),
    ];
    for (label, request) in &missing {
        let (status, response) = emulator.call("ListDatabases", request).await;
        assert_eq!(
            status, 404,
            "case {label}: expected 404, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {label}: expected ResourceNotFoundException, got {response}"
        );
    }

    let refused: [(&str, Value); 4] = [
        ("no data warehouse at all", json!({"Database": "dev"})),
        (
            "both a cluster and a workgroup",
            json!({
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName": "my-workgroup",
                "Database": "dev",
                "DbUser": "awsuser",
            }),
        ),
        ("no database", cluster()),
        (
            "two authorization methods at once",
            json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "SecretArn": "arn:aws:secretsmanager:us-east-1:123456789012:secret:db-AbCdEf",
            }),
        ),
    ];
    for (label, request) in &refused {
        let (status, response) = emulator.call("ListDatabases", request).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {label}: expected ValidationException, got {response}"
        );
    }
}

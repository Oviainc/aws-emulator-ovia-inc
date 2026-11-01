// SPDX-License-Identifier: CC0-1.0

//! What each AWS-managed keyspace publishes as its tables, driven end to end over the
//! service's own `awsJson1_0` wire protocol through the harness in
//! [`crate::keyspace_table_lifecycle`].
//!
//! The rule under test: an account holds every table of the catalog each AWS-managed
//! keyspace publishes before any client call, so `ListTables` on one of them reports that
//! WHOLE catalog — the handshake tables are entries of it, not the catalog — and
//! `GetTable` resolves each entry and nothing else, whatever page size the caller asks
//! for.
//!
//! The expected catalogs below are written out literally rather than read back from the
//! crate's seed constants, so this suite pins the contents of each catalog instead of
//! agreeing with whatever the handler happens to read.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{assert_failure, assert_ok, table_arn, Emulator};

/// One row of the catalog under test: an AWS-managed keyspace, every table it publishes,
/// and names that are NOT tables of it.
struct Row {
    /// The keyspace a `ListTables` names.
    keyspace: &'static str,
    /// Every `tableName` that keyspace publishes, in no particular order here — the
    /// listing's own order is asserted separately.
    tables: &'static [&'static str],
    /// Names a client might expect in this keyspace that it does NOT publish, so the
    /// catalog is pinned from both sides rather than as "anything is in a system
    /// keyspace".
    absent: &'static [&'static str],
}

/// The AWS-managed catalogs, as a recorded `ListTables` reports them for an account that
/// created nothing. Rows differ in how many tables they carry (4 to 15), in the shape of
/// the names (the mixed-case `IndexInfo` beside the snake_case rest) and in which views
/// they share, so the rule is asserted over the whole class and not over one keyspace.
const CATALOGS: &[Row] = &[
    // The Cassandra `system` keyspace: the node- and cluster-local metadata a driver
    // reads, of which `local` and `peers` — the pair a connect handshake reads — are two
    // entries.
    Row {
        keyspace: "system",
        tables: &[
            "IndexInfo",
            "available_ranges",
            "batches",
            "built_views",
            "compaction_history",
            "local",
            "paxos",
            "peer_events",
            "peers",
            "prepared_statements",
            "range_xfers",
            "size_estimates",
            "sstable_activity",
            "transferred_ranges",
            "views_builds_in_progress",
        ],
        // The pre-3.0 write-path tables and the schema views, which live in
        // `system_schema` rather than here.
        absent: &["batchlog", "hints", "keyspaces", "columns"],
    },
    // The Cassandra schema catalog.
    Row {
        keyspace: "system_schema",
        tables: &[
            "aggregates",
            "columns",
            "dropped_columns",
            "functions",
            "indexes",
            "keyspaces",
            "tables",
            "triggers",
            "types",
            "views",
        ],
        // The Amazon Keyspaces-specific views, which only its own copy of the catalog
        // carries.
        absent: &["autoscaling", "tags", "tables_history", "local"],
    },
    // Amazon Keyspaces' own copy of the schema catalog: the same schema views, without
    // `dropped_columns`, plus the history view of the table catalog and the two views a
    // table's AWS settings are read out of.
    Row {
        keyspace: "system_schema_mcs",
        tables: &[
            "aggregates",
            "autoscaling",
            "columns",
            "functions",
            "indexes",
            "keyspaces",
            "tables",
            "tables_history",
            "tags",
            "triggers",
            "types",
            "views",
        ],
        absent: &["dropped_columns", "local", "peers"],
    },
    // The multi-Region catalog: the per-Region views of the replicated resources.
    Row {
        keyspace: "system_multiregion_info",
        tables: &["autoscaling", "keyspaces", "tables", "types"],
        absent: &["columns", "tables_history", "local"],
    },
];

/// The `tableName`s of a `ListTables` body, in the order the page reports them.
fn page_names(case: &str, body: &Value) -> Vec<String> {
    body["tables"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a `tables` list, got {body}"))
        .iter()
        .map(|summary| {
            summary["tableName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every entry carries a tableName, got {summary}")
                })
                .to_string()
        })
        .collect()
}

/// Every table of `keyspace`, walked at `page_size` so the whole catalog has to be
/// reachable through the paginated path too; returns the names in page order.
async fn walk(emulator: &Emulator, keyspace: &str, page_size: usize) -> Vec<String> {
    let case = format!("{keyspace} at page size {page_size}");
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({"keyspaceName": keyspace, "maxResults": page_size});
        if let Some(resume) = &token {
            request["nextToken"] = json!(resume);
        }
        let page = assert_ok(&case, emulator.call("ListTables", request).await);
        pages += 1;
        assert!(
            pages <= seen.len() + 2,
            "case {case:?}: pagination did not terminate after {pages} pages, having seen \
             {seen:?}"
        );
        let names = page_names(&case, &page);
        assert!(
            names.len() <= page_size,
            "case {case:?}: expected at most {page_size} entries per page, got {:?}",
            names
        );
        seen.extend(names);
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

/// `ListTables` reports the whole catalog of every AWS-managed keyspace, each entry with
/// the three members the model requires, in the one total order every page size resumes
/// through.
#[tokio::test]
async fn list_tables_reports_each_managed_keyspaces_whole_catalog() {
    let emulator = Emulator::start();

    for row in CATALOGS {
        let case = row.keyspace;
        let mut expected: Vec<String> = row.tables.iter().map(|name| (*name).to_string()).collect();
        expected.sort();

        let listed = assert_ok(
            case,
            emulator
                .call("ListTables", json!({"keyspaceName": row.keyspace}))
                .await,
        );
        let got = page_names(case, &listed);
        assert_eq!(
            got, expected,
            "case {case:?}: expected the whole catalog {expected:?}, got {got:?}"
        );

        // Every entry carries keyspaceName, tableName and the published table ARN.
        for summary in listed["tables"].as_array().unwrap_or(&Vec::new()) {
            let table = summary["tableName"].as_str().unwrap_or_default();
            assert_eq!(
                summary,
                &json!({
                    "keyspaceName": row.keyspace,
                    "tableName": table,
                    "resourceArn": table_arn(row.keyspace, table),
                }),
                "case {case:?}: expected the three required members of {table:?}, got {summary}"
            );
        }

        // The whole catalog is reachable through the paginated path as well, in the same
        // order, from the smallest page the model allows (`maxResults` range 1..1000) to
        // one past the collection.
        for page_size in [1usize, 2, 7, expected.len(), expected.len() + 1, 1000] {
            let walked = walk(&emulator, row.keyspace, page_size).await;
            assert_eq!(
                walked, expected,
                "case {case:?} at page size {page_size}: expected {expected:?}, got {walked:?}"
            );
        }

        // A second read reports exactly what the first did: listing the catalog changes
        // nothing a later caller sees.
        let again = assert_ok(
            case,
            emulator
                .call("ListTables", json!({"keyspaceName": row.keyspace}))
                .await,
        );
        assert_eq!(
            again, listed,
            "case {case:?}: a read must leave the catalog alone, got {listed} then {again}"
        );
    }
}

/// Each listed entry is a table `GetTable` resolves, and a name the catalog does not
/// carry is the modelled not-found — so the catalog is a set of resources rather than a
/// blanket answer for any name in an AWS-managed keyspace.
#[tokio::test]
async fn every_catalog_entry_resolves_and_nothing_else_does() {
    let emulator = Emulator::start();

    for row in CATALOGS {
        for table in row.tables {
            let case = format!("{}.{table}", row.keyspace);
            let got = assert_ok(
                &case,
                emulator
                    .call(
                        "GetTable",
                        json!({"keyspaceName": row.keyspace, "tableName": table}),
                    )
                    .await,
            );
            assert_eq!(
                (
                    &got["keyspaceName"],
                    &got["tableName"],
                    &got["resourceArn"],
                    &got["status"]
                ),
                (
                    &json!(row.keyspace),
                    &json!(table),
                    &json!(table_arn(row.keyspace, table)),
                    &json!(keyspaces::TABLE_STATUS_ACTIVE)
                ),
                "case {case:?}: expected the stored system table, got {got}"
            );
        }

        for table in row.absent {
            let case = format!("{}.{table}", row.keyspace);
            assert_failure(
                &case,
                emulator
                    .call(
                        "GetTable",
                        json!({"keyspaceName": row.keyspace, "tableName": table}),
                    )
                    .await,
                400,
                "ResourceNotFoundException",
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for querying THROUGH a lookup table: `StartQuery` over a pipeline whose
//! `lookup` command enriches each record from the reference data `CreateLookupTable` uploaded.
//!
//! The rule under test: a query string built from the commands the query language defines
//! compiles, so `StartQuery` accepts it and answers a `queryId` — the closed set of commands
//! the compiler knows has to be the set the language defines, `lookup` among them — and the
//! lookup table such a stage names is resolved against the account's stored lookup tables, so
//! a name no table of this account is held under is the missing resource it is
//! (`ResourceNotFoundException`) rather than a fault in the query string, while a `lookup`
//! that names no table at all is a query the language does not define
//! (`MalformedQueryException`, with the detail and the location in the `QueryCompileError`).
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The table spans what the rule generalises over rather than one query: whether the `lookup`
//! stage carries an `OUTPUT` list, renames the record's key, renames an output column, how the
//! command is cased, whether the table's name is quoted, how many `lookup` stages the pipeline
//! has and which commands sit after them. The query one issue happens to quote is a single
//! row.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// The operations under test.
const START: &str = "StartQuery";
const DESCRIBE: &str = "DescribeQueries";

/// The modelled error codes the rule names.
const MALFORMED: &str = "MalformedQueryException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The lookup tables these cases query through: a name, and the CSV body it holds. The first
/// column of a body is the key a `lookup` joins on and the rest are the columns it can output.
const USERS: (&str, &str) = (
    "query_users",
    "user_id,name,department\nu1,Alice,Engineering\nu2,Bob,Sales\n",
);
const TEAMS: (&str, &str) = ("query_teams", "name,team\nAlice,platform\nBob,west\n");

/// The log group every case queries.
const GROUP: &str = "/lookup/queries";

/// Stand up the log group and both lookup tables, and answer the window a query of the group
/// is started with, in the unit `StartQuery` documents for its range ("epoch time, the number
/// of SECONDS since January 1, 1970").
async fn account_with_tables(logs: &Logs) -> (i64, i64) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    for (name, body) in [USERS, TEAMS] {
        logs.ok(
            "CreateLookupTable",
            &json!({"lookupTableName": name, "tableBody": body}),
        )
        .await;
    }
    let now_secs = now_ms() / 1_000;
    (now_secs - 600, now_secs + 600)
}

/// Start `query_string` over the standing group and window.
async fn start(logs: &Logs, window: (i64, i64), query_string: &str) -> (u16, Value) {
    logs.call(
        START,
        &json!({
            "queryLanguage": "CWLI",
            "logGroupName": GROUP,
            "startTime": window.0,
            "endTime": window.1,
            "queryString": query_string,
        }),
    )
    .await
}

/// The ARN of one of this account's lookup tables, which is how a table is deleted.
async fn table_arn(logs: &Logs, name: &str) -> String {
    let page = logs
        .ok("DescribeLookupTables", &json!({}))
        .await;
    page["lookupTables"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeLookupTables reports lookupTables, got {page}"))
        .iter()
        .find(|table| table["lookupTableName"].as_str() == Some(name))
        .and_then(|table| table["lookupTableArn"].as_str())
        .unwrap_or_else(|| panic!("the account must hold the lookup table {name}"))
        .to_string()
}

/// Every form of a `lookup` pipeline the query language defines is accepted, and the started
/// query is stored under the id the start answered.
#[tokio::test]
async fn a_query_through_a_lookup_table_is_accepted_and_stored() {
    let logs = Logs::new();
    let window = account_with_tables(&logs).await;
    let (users, teams) = (USERS.0, TEAMS.0);

    // (case, the query string) — one row per form the `lookup` stage takes.
    let cases: Vec<(&str, String)> = vec![
        (
            "the OUTPUT form the command documents",
            format!("fields user_id, action, name, department | lookup {users} user_id OUTPUT name, department"),
        ),
        (
            "no OUTPUT clause, so every column of the table is joined",
            format!("fields @message | lookup {users} user_id"),
        ),
        (
            "the record's key renamed to the table's own key column",
            format!("fields @message | lookup {users} uid AS user_id OUTPUT name"),
        ),
        (
            "an output column renamed, and read by a later command",
            format!("fields @message | lookup {users} user_id OUTPUT name AS who | display who"),
        ),
        (
            "the command spelled in upper case",
            format!("fields @message | LOOKUP {users} user_id OUTPUT name"),
        ),
        (
            "the table's name quoted",
            format!("fields @message | lookup \"{users}\" user_id OUTPUT name"),
        ),
        (
            "two lookup stages over two tables",
            format!(
                "fields @message | lookup {users} user_id OUTPUT name | lookup {teams} name \
                 OUTPUT team"
            ),
        ),
        (
            "the same table looked up twice",
            format!("fields @message | lookup {users} user_id OUTPUT name | lookup {users} user_id OUTPUT department"),
        ),
        (
            "a lookup the rest of the pipeline aggregates, sorts and limits",
            format!(
                "fields @message | lookup {users} user_id OUTPUT department | stats count(*) as \
                 hits by department | sort hits desc | limit 5"
            ),
        ),
        (
            "a lookup of a table whose key the pipeline parsed out itself",
            format!(r"parse @message /(?<user_id>u\d+)/ | lookup {users} user_id OUTPUT name"),
        ),
    ];
    let mut started: Vec<String> = Vec::new();
    for (case, query_string) in cases {
        let (status, body) = start(&logs, window, &query_string).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let id = body["queryId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an accepted start answers a queryId, got {body}"))
            .to_string();
        assert!(
            !started.contains(&id),
            "case {case:?}: every started query gets an id no earlier one used, got {id}"
        );

        // Read the query back through the reporting operation: the start really stored it.
        let page = logs.ok(DESCRIBE, &json!({"logGroupName": GROUP})).await;
        let reported = page["queries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: DescribeQueries reports queries, got {page}"))
            .iter()
            .find(|query| query["queryId"].as_str() == Some(id.as_str()))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the started query must be reported, got {page}"));
        assert_eq!(
            reported["queryString"].as_str(),
            Some(query_string.as_str()),
            "case {case:?}: the reported query string is the one that was started, got {reported}"
        );
        started.push(id);
    }
}

/// The lookup table a `lookup` stage names is resolved against the account's tables: a name no
/// table is held under is the missing resource, and a `lookup` naming no table at all does not
/// compile.
#[tokio::test]
async fn a_lookup_is_resolved_against_the_accounts_tables() {
    let logs = Logs::new();
    let window = account_with_tables(&logs).await;
    let (users, teams) = (USERS.0, TEAMS.0);

    // (case, the query string, the modelled code it is refused under, the text the detail
    // must mention — empty where the refusal carries no compile error)
    let cases: Vec<(&str, String, &str, &str)> = vec![
        (
            "a lookup table name the account does not hold",
            "fields @message | lookup query_absent user_id OUTPUT name".to_string(),
            NOT_FOUND,
            "",
        ),
        (
            "a second lookup stage naming a table the account does not hold",
            format!(
                "fields @message | lookup {users} user_id OUTPUT name | lookup query_absent name \
                 OUTPUT team"
            ),
            NOT_FOUND,
            "",
        ),
        (
            "a lookup table named by its ARN rather than its name",
            format!(
                "fields @message | lookup arn:aws:logs:eu-west-2:000000000000:lookup-table/{teams} \
                 name OUTPUT team"
            ),
            NOT_FOUND,
            "",
        ),
        (
            "a lookup naming no table at all",
            "fields @message | lookup".to_string(),
            MALFORMED,
            "lookup table",
        ),
        (
            "a word that is no command of the language",
            format!("fields @message | enrich {users} user_id OUTPUT name"),
            MALFORMED,
            "query command",
        ),
    ];
    for (case, query_string, expected, detail) in cases {
        let (status, body) = start(&logs, window, &query_string).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = crate::harness::error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code} with body {body}"
        );
        if detail.is_empty() {
            continue;
        }
        let reported = body["queryCompileError"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        assert!(
            reported.contains(detail),
            "case {case:?}: the compile error must say what it stopped on ({detail:?}), got {body}"
        );
        let location = &body["queryCompileError"]["location"];
        let (start_offset, end_offset) = (
            location["startCharOffset"].as_i64(),
            location["endCharOffset"].as_i64(),
        );
        let length = i64::try_from(query_string.chars().count()).unwrap_or(i64::MAX);
        assert!(
            start_offset.is_some_and(|start| (0..=length).contains(&start))
                && end_offset.is_some_and(|end| (0..=length).contains(&end))
                && start_offset <= end_offset,
            "case {case:?}: the compile error locates the text at fault inside the query, got \
             {body}"
        );
    }

    // Resolution follows the STATE: the very query that is accepted while the table is held is
    // refused once it is deleted, and accepted again once it is created again.
    let query = format!("fields @message | lookup {teams} name OUTPUT team");
    let (status, body) = start(&logs, window, &query).await;
    assert_eq!(
        status, 200,
        "case while the table is held: expected HTTP 200, got {status} with body {body}"
    );
    let arn = table_arn(&logs, teams).await;
    logs.ok("DeleteLookupTable", &json!({"lookupTableArn": arn}))
        .await;
    let (status, body) = start(&logs, window, &query).await;
    assert_eq!(
        status, 400,
        "case once the table is deleted: expected HTTP 400, got {status} with body {body}"
    );
    let code = crate::harness::error_code(&body);
    assert_eq!(
        code, NOT_FOUND,
        "case once the table is deleted: expected {NOT_FOUND}, got {code} with body {body}"
    );
    logs.ok(
        "CreateLookupTable",
        &json!({"lookupTableName": teams, "tableBody": TEAMS.1}),
    )
    .await;
    let (status, body) = start(&logs, window, &query).await;
    assert_eq!(
        status, 200,
        "case once the table is created again: expected HTTP 200, got {status} with body {body}"
    );
}

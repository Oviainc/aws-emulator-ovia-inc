// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartQuery` — starting a CloudTrail Lake query.
//!
//! The rules under test, each grounded in this service's Smithy model or in the
//! prose the model carries for `StartQuery`:
//!
//! * a query statement is "your SQL query" over an event data store, so
//!   `StartQuery` accepts a statement that parses as ONE read-only `SELECT`
//!   whose tables — the relations the PARSER read — include exactly one event
//!   data store this Region holds, whatever form that statement and that store
//!   reference take, mints a `UUID` `QueryId` for the run and persists it, so
//!   `DescribeQuery` reports the statement the request sent;
//! * anything else is not a Lake query ⇒ `InvalidQueryStatementException`
//!   (HTTP 400) and nothing persisted: a statement that does not parse, one
//!   that is not a `SELECT`, one whose query body writes, more than one
//!   statement, no `FROM` clause at all,
//!   and a `FROM` clause naming no event data store the Region it addresses
//!   holds — the caller's Region for a bare id, the ARN's own Region for an ARN,
//!   which is how every operation of this service reads a store identifier —
//!   the last being the recorded CloudTrail answer for a query over an account
//!   holding no store;
//! * a statement naming SEVERAL stores this account holds is a query CloudTrail
//!   runs ("you can query across multiple event data stores as long as all event
//!   data stores exist in your account") and this emulator does not, a run being
//!   recorded against the one store it reads ⇒ the modelled
//!   `UnsupportedOperationException`, and nothing persisted;
//! * "StartQuery requires you specify either the QueryStatement parameter, or a
//!   QueryAlias and any QueryParameters": neither, both, `QueryParameters`
//!   without an alias, or an alias naming no query template ⇒
//!   `InvalidParameterException`; an alias that DOES name one — a dashboard
//!   widget's — runs that template's statement, and `QueryParameters` for it are
//!   refused with `UnsupportedOperationException`, CloudTrail publishing no rule
//!   for how they bind into the template;
//! * the store the statement resolved to is the one the request addresses: a
//!   store `EventDataStoreOwnerAccountId` does not own ⇒
//!   `EventDataStoreNotFoundException`, a store awaiting deletion ⇒
//!   `InactiveEventDataStoreException`;
//! * a run is `QUEUED` and the clock settles it into `FINISHED`
//!   ([`state::QUERY_SETTLE_AFTER`]), with the statistics of a store that holds
//!   no events;
//! * "The maximum number of concurrent queries is 10"
//!   ([`state::MAX_CONCURRENT_QUERIES`]) ⇒ `MaxConcurrentQueriesException`
//!   (HTTP 429), and a run that has ended no longer counts against it.
//!
//! Most cases seed the registry the handler reads directly, through the shared
//! engine, and then go over the wire for every assertion: they need stores in
//! statuses and under owners no create call produces (a store awaiting deletion,
//! a store shared from another account, a store in another Region). The
//! implemented `CreateEventDataStore` is exercised too, by the case that mints a
//! store through the API and queries the id and the ARN it answers with.

use serde_json::{json, Value};
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::Widget;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::widget_query::{
    MAX_QUERY_NESTING_DEPTH, MAX_QUERY_TOKENS, QUERY_RECURSION_LIMIT,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    self, dashboard_arn, event_data_store_arn, DashboardRecord, EventDataStoreRecord,
    EventDataStoreState, QueryRecord, QueryState,
};

use super::emulator::{Emulator, HOME_REGION};

/// The id of the event data store most cases run their queries on. An event data
/// store id is the resource suffix of its ARN.
const STORE_ID: &str = "aaaaaaaa-1111-2222-3333-444444444444";

/// The HTTP status this service answers a not-found refusal with: the model
/// annotates `EventDataStoreNotFoundException` `@httpError(404)`, and the
/// recorded CloudTrail response is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// A margin on top of [`state::QUERY_SETTLE_AFTER`], so a case that waits for a
/// run to end waits past the instant it is due rather than exactly on it.
fn past_settle() -> Duration {
    state::QUERY_SETTLE_AFTER + Duration::from_millis(250)
}

/// The id of the nth run a case seeds directly, in the modelled `UUID` shape
/// (36 characters of `[a-f0-9-]`).
fn seeded_query_id(nth: u8) -> String {
    format!("{nth:02x}aaaaaa-1111-2222-3333-444444444444")
}

/// Seed one event data store, the way `CreateEventDataStore` mints one — used by
/// the cases that need a status or an owner a create call cannot produce.
fn seed_store(emulator: &Emulator, id: &str, region: &str, status: EventDataStoreState) {
    seed_store_owned_by(emulator, id, region, status, None);
}

/// Seed one event data store owned by `owner_account_id` (`None` = the
/// emulator's own account), for the cross-account cases.
fn seed_store_owned_by(
    emulator: &Emulator,
    id: &str,
    region: &str,
    status: EventDataStoreState,
    owner_account_id: Option<&str>,
) {
    emulator.state().lock().event_data_stores.insert(
        id.to_string(),
        EventDataStoreRecord {
            id: id.to_string(),
            name: format!("store-{id}"),
            region: region.to_string(),
            status,
            owner_account_id: owner_account_id.map(str::to_string),
            created_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            ..EventDataStoreRecord::default()
        },
    );
}

/// Seed one CloudTrail-managed dashboard whose widget carries `alias` and the
/// statement that alias names — the shape of a query template this account
/// holds, which is where a `QueryAlias` resolves.
fn seed_dashboard_widget(emulator: &Emulator, alias: &str, statement: &str) {
    let name = "lake-highlights";
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    emulator.state().lock().dashboards.insert(
        (name.to_string(), HOME_REGION.to_string()),
        DashboardRecord {
            name: name.to_string(),
            dashboard_arn: dashboard_arn(HOME_REGION, name),
            dashboard_type: state::DASHBOARD_TYPE_MANAGED.to_string(),
            status: state::DASHBOARD_STATUS_CREATED.to_string(),
            settles_at: at,
            widgets: vec![Widget {
                query_alias: Some(alias.to_string()),
                query_statement: Some(statement.to_string()),
                query_parameters: None,
                view_properties: None,
            }],
            refresh_schedule: None,
            termination_protection_enabled: false,
            tags: Vec::new(),
            created_at: at,
            updated_at: at,
            home_region: HOME_REGION.to_string(),
            last_refresh_id: None,
            last_refresh_failure_reason: None,
        },
    );
}

/// Start `statement` and hand back the reply.
async fn start(emulator: &Emulator, statement: &str) -> super::emulator::Reply {
    emulator
        .call("StartQuery", &json!({ "QueryStatement": statement }))
        .await
}

/// The `QueryId` of a successful `StartQuery`, asserted to be the modelled
/// `UUID`: exactly 36 characters of `[a-f0-9-]`.
fn minted_query_id(case: &str, body: &Value) -> String {
    let query_id = body
        .get("QueryId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        query_id.len(),
        36,
        "case {case:?}: a QueryId is 36 characters, got {query_id:?}"
    );
    assert!(
        query_id
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()
                || character == '-'),
        "case {case:?}: a QueryId is [a-f0-9-], got {query_id:?}"
    );
    query_id
}

/// Describe `query_id` and hand back the reported body.
async fn describe(emulator: &Emulator, query_id: &str) -> Value {
    let reply = emulator
        .call("DescribeQuery", &json!({ "QueryId": query_id }))
        .await;
    reply.expect_ok("DescribeQuery").clone()
}

/// Start every statement in `statements` and assert each one became a run on
/// [`STORE_ID`]: a `UUID` id of its own, and a record `DescribeQuery` reports
/// with the statement exactly as the request sent it.
///
/// One table per caller, because the runs a table starts are all in flight at
/// once and the concurrency limit bounds those (see
/// [`the_concurrency_limit_bounds_the_runs_in_flight`]).
async fn expect_every_statement_starts_a_run(emulator: &Emulator, statements: &[String]) {
    assert!(
        statements.len() <= state::MAX_CONCURRENT_QUERIES,
        "case table size: {} statements would pass the concurrency limit of {}",
        statements.len(),
        state::MAX_CONCURRENT_QUERIES
    );

    let mut minted: Vec<String> = Vec::new();
    for statement in statements {
        let reply = start(emulator, statement).await;
        let body = reply.expect_ok(statement);
        let query_id = minted_query_id(statement, body);
        assert!(
            body.get("EventDataStoreOwnerAccountId").is_none(),
            "case {statement:?}: a query on this account's own store reports no owner \
             account, got {body:?}"
        );

        // The run is readable back through `DescribeQuery`, and what it reports
        // is the statement the request sent — not a normalised form of it.
        let reported = describe(emulator, &query_id).await;
        assert_eq!(
            reported.get("QueryString").and_then(Value::as_str),
            Some(statement.as_str()),
            "case {statement:?}: expected the statement as sent, got {reported:?}"
        );
        assert_eq!(
            reported.get("QueryStatus").and_then(Value::as_str),
            Some(QueryState::Queued.as_wire()),
            "case {statement:?}: a new run is queued, got {reported:?}"
        );

        // The run reads the store the statement named, whichever form named it.
        let stored = emulator
            .state()
            .lock()
            .queries
            .get(&query_id)
            .map(|record| record.event_data_store_id.clone());
        assert_eq!(
            stored,
            Some(STORE_ID.to_string()),
            "case {statement:?}: expected the run on {STORE_ID}, got {stored:?}"
        );
        minted.push(query_id);
    }

    // Every run is its own: one id per call, never a reused one.
    let mut unique = minted.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        statements.len(),
        "case distinct ids: expected {} ids, got {minted:?}",
        statements.len()
    );
}

/// A statement that parses as one read-only `SELECT` whose tables name an event
/// data store this Region holds starts a run — whatever form the statement and
/// the store reference take. Each row differs in the SQL's shape and in how it
/// names the store: a bare id, the store's full ARN, a quoted id; one line or
/// several; upper- or lower-case keywords; a projection, a star, an aggregate, a
/// CTE, a derived table, a self-join.
#[tokio::test]
async fn start_query_accepts_a_select_over_a_store_this_region_holds() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    let arn = event_data_store_arn(HOME_REGION, STORE_ID);

    let statements = vec![
        format!("SELECT eventName FROM {STORE_ID}"),
        format!("SELECT * FROM {arn} WHERE eventSource = 'cloudtrail.amazonaws.com'"),
        format!("select eventid from \"{STORE_ID}\" limit 10"),
        format!(
            "SELECT count(*) AS total\nFROM {STORE_ID}\nWHERE eventTime > '2026-01-01 00:00:00'"
        ),
        format!(
            "WITH recent AS (SELECT eventName FROM {STORE_ID}) \
             SELECT eventName FROM recent GROUP BY eventName"
        ),
        format!("SELECT eventName FROM {STORE_ID};"),
        format!("SELECT eventName, useridentity.arn FROM {STORE_ID} ORDER BY eventName DESC"),
        // The tables are the ones the PARSER read, so a derived table and a
        // self-join name the same one store.
        format!("SELECT eventName FROM (SELECT eventName FROM {STORE_ID}) AS recent"),
        format!(
            "SELECT a.eventName FROM {STORE_ID} AS a JOIN {STORE_ID} AS b \
             ON a.eventID = b.eventID"
        ),
    ];

    expect_every_statement_starts_a_run(&emulator, &statements).await;
}

/// RULE: the store a query reads is one the registry holds, however it got there
/// — so a store MINTED through `CreateEventDataStore` is queryable by the id and
/// by the ARN that call answered with, and the run reads it back through
/// `DescribeQuery`.
///
/// The rows are the two forms a statement names a store by, over a store this
/// test did not seed: the API's own.
#[tokio::test]
async fn a_store_created_through_the_api_is_the_store_a_query_reads() {
    let emulator = Emulator::new();

    let created = emulator
        .call(
            "CreateEventDataStore",
            &json!({ "Name": "lake-store-for-queries" }),
        )
        .await;
    let arn = created
        .expect_ok("CreateEventDataStore")
        .get("EventDataStoreArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // An event data store id is the resource suffix of its ARN, which is how
    // every operation of this service reads one.
    let id = arn
        .rsplit('/')
        .next()
        .expect("an event data store ARN carries its id")
        .to_string();
    assert_eq!(
        arn,
        event_data_store_arn(HOME_REGION, &id),
        "case a created store: the ARN must be this Region's ARN of the minted id, got {arn:?}"
    );

    for (case, statement) in [
        ("by id", format!("SELECT eventName FROM {id}")),
        ("by ARN", format!("SELECT eventID FROM {arn} LIMIT 1")),
    ] {
        let reply = start(&emulator, &statement).await;
        let query_id = minted_query_id(case, reply.expect_ok(case));
        let described = describe(&emulator, &query_id).await;
        let reported = described
            .get("EventDataStoreOwnerAccountId")
            .and_then(Value::as_str);
        assert!(
            reported.is_none(),
            "case {case:?}: a query on this account's own store reports no owner account, \
             got {reported:?}"
        );
        let read_back = described
            .get("QueryString")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            read_back, statement,
            "case {case:?}: DescribeQuery must report the statement the request sent, got \
             {read_back:?}"
        );
    }
}

/// The `FROM` clause is the one the SQL TOKENIZER finds, so the word `from`
/// inside a string literal, a quoted identifier or a comment is a literal, an
/// identifier and a comment — never the clause that names the event data store.
/// Each row carries `from` somewhere that is not a clause, before the clause
/// itself.
#[tokio::test]
async fn the_from_clause_is_the_one_the_sql_tokenizer_finds() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    let statements = vec![
        format!("SELECT 'logged from console' AS source, eventName FROM {STORE_ID}"),
        format!("SELECT eventName AS \"from console\" FROM {STORE_ID}"),
        format!("-- events from the console\nSELECT eventName FROM {STORE_ID}"),
        format!("/* read from here */ SELECT eventName FROM {STORE_ID} WHERE eventName <> 'from'"),
    ];

    expect_every_statement_starts_a_run(&emulator, &statements).await;
}

/// A statement that is not a Lake query is refused with
/// `InvalidQueryStatementException`, and nothing is persisted. The rows span the
/// ways a statement fails to be one: it does not parse, it is not a `SELECT`, it
/// is more than one statement, it selects from nothing, and its `FROM` clause
/// names no event data store this Region holds.
#[tokio::test]
async fn start_query_refuses_a_statement_that_is_not_a_lake_query() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    // A store of another Region: reachable from there, so its ARN names no store
    // the calls below can read.
    let elsewhere = "bbbbbbbb-1111-2222-3333-444444444444";
    seed_store(
        &emulator,
        elsewhere,
        "eu-west-1",
        EventDataStoreState::Enabled,
    );
    // A SECOND store in this Region, for the multi-store rows below.
    let also_here = "ffffffff-1111-2222-3333-444444444444";
    seed_store(&emulator, also_here, HOME_REGION, EventDataStoreState::Enabled);

    let cases = vec![
        ("no FROM clause at all", "SELECT 1".to_string()),
        (
            "a FROM clause with nothing after it",
            "SELECT eventName FROM".to_string(),
        ),
        (
            "a statement the SQL parser cannot read",
            format!("SELEKT eventName FROM {STORE_ID} WHERE"),
        ),
        (
            "a statement that is not a SELECT",
            format!("DELETE FROM {STORE_ID}"),
        ),
        (
            "a statement whose query body writes",
            format!(
                "WITH source AS (SELECT eventName FROM {STORE_ID}) \
                 INSERT INTO other SELECT * FROM source"
            ),
        ),
        (
            "two statements",
            format!("SELECT eventName FROM {STORE_ID}; SELECT eventId FROM {STORE_ID}"),
        ),
        (
            "a FROM clause naming no store at all",
            "SELECT eventName FROM $$$".to_string(),
        ),
        (
            "a FROM clause naming an unknown store",
            "SELECT eventName FROM cccccccc-1111-2222-3333-444444444444".to_string(),
        ),
        (
            "a FROM clause naming a store of another Region by its bare id",
            format!("SELECT eventName FROM {elsewhere}"),
        ),
        (
            "a FROM clause naming an ARN no Region holds a store for",
            format!(
                "SELECT eventName FROM {}",
                event_data_store_arn("eu-west-1", "cccccccc-1111-2222-3333-444444444444")
            ),
        ),
    ];

    for (case, statement) in cases {
        let reply = start(&emulator, &statement).await;
        reply.expect_error(case, 400, "InvalidQueryStatementException");
        let message = reply
            .body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // The crate's ONE `InvalidQueryStatementException` sentence, which the
        // recorded CloudTrail response for an invalid statement also carries: every
        // way a statement fails to be a Lake query is answered with it, so no row
        // may word its own.
        assert_eq!(
            message,
            state::widget_query::INVALID_QUERY_STATEMENT_MESSAGE,
            "case {case:?}: every invalid statement is refused with the service's one \
             sentence, got {message:?}"
        );
        assert!(
            emulator.state().lock().queries.is_empty(),
            "case {case:?}: a refused query persists nothing, got {:?}",
            emulator.state().lock().queries
        );
    }

    // A statement whose tables name SEVERAL stores this account holds is a query
    // CloudTrail runs — "you can query across multiple event data stores as long
    // as all event data stores exist in your account" (the model doc for
    // `RequestWidget$QueryStatement`) — so it is not invalid SQL. This emulator
    // records a run against the one store it reads, so it answers the modelled
    // error for what it does not support, and still persists nothing. The rows
    // differ in WHERE the second store is named: a sub-query in the projection, a
    // join, and a union of two selects.
    for (case, statement) in [
        (
            "a second store in a projection sub-query",
            format!("SELECT (SELECT 1 FROM {also_here}) AS other, eventName FROM {STORE_ID}"),
        ),
        (
            "two stores joined",
            format!(
                "SELECT a.eventName FROM {STORE_ID} AS a JOIN {also_here} AS b \
                 ON a.eventID = b.eventID"
            ),
        ),
        (
            "two stores unioned",
            format!(
                "SELECT eventName FROM {STORE_ID} UNION SELECT eventName FROM {also_here}"
            ),
        ),
    ] {
        let reply = start(&emulator, &statement).await;
        reply.expect_error(case, 400, "UnsupportedOperationException");
        assert!(
            emulator.state().lock().queries.is_empty(),
            "case {case:?}: a refused query persists nothing, got {:?}",
            emulator.state().lock().queries
        );
    }
}

/// The selector rule: "StartQuery requires you specify either the QueryStatement
/// parameter, or a QueryAlias and any QueryParameters." An alias names a query
/// template, and this account holds none.
#[tokio::test]
async fn start_query_refuses_a_request_that_names_no_sql_to_run() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    for (case, body) in [
        ("neither QueryStatement nor QueryAlias", json!({})),
        (
            "both QueryStatement and QueryAlias",
            json!({
                "QueryStatement": format!("SELECT eventName FROM {STORE_ID}"),
                "QueryAlias": "DailySignIns",
            }),
        ),
        (
            "QueryParameters without a QueryAlias",
            json!({
                "QueryStatement": format!("SELECT eventName FROM {STORE_ID}"),
                "QueryParameters": ["2026-01-01"],
            }),
        ),
        (
            "a QueryAlias naming no query template",
            json!({ "QueryAlias": "DailySignIns" }),
        ),
        (
            "a QueryAlias with parameters, naming no query template",
            json!({ "QueryAlias": "DailySignIns", "QueryParameters": ["2026-01-01"] }),
        ),
    ] {
        let reply = emulator.call("StartQuery", &body).await;
        reply.expect_error(case, 400, "InvalidParameterException");
        assert!(
            emulator.state().lock().queries.is_empty(),
            "case {case:?}: a refused request persists nothing"
        );
    }
}

/// A `QueryAlias` names a query template, and the templates an account holds are
/// the ones its dashboards' widgets carry: the run is that template's statement,
/// and it is reachable by the alias it ran under. `QueryParameters` for the
/// template are refused — CloudTrail publishes no rule for how they bind into
/// its statement, so a run that ignored them would report work that did not
/// happen.
#[tokio::test]
async fn a_query_alias_runs_the_template_a_dashboard_widget_carries() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    let alias = "DailySignIns";
    let template = format!("SELECT eventName FROM {STORE_ID} WHERE eventName = 'ConsoleLogin'");
    seed_dashboard_widget(&emulator, alias, &template);

    let reply = emulator
        .call("StartQuery", &json!({ "QueryAlias": alias }))
        .await;
    let query_id = minted_query_id(
        "a template this account holds",
        reply.expect_ok("a template this account holds"),
    );
    let reported = describe(&emulator, &query_id).await;
    assert_eq!(
        reported.get("QueryString").and_then(Value::as_str),
        Some(template.as_str()),
        "case a template this account holds: expected the template's statement, got {reported:?}"
    );

    // The run is the alias's, so `DescribeQuery` reaches it by that alias.
    let by_alias = emulator
        .call("DescribeQuery", &json!({ "QueryAlias": alias }))
        .await;
    assert_eq!(
        by_alias
            .expect_ok("described by alias")
            .get("QueryId")
            .and_then(Value::as_str),
        Some(query_id.as_str()),
        "case described by alias: expected the run started under it, got {by_alias:?}"
    );

    let refused = emulator
        .call(
            "StartQuery",
            &json!({ "QueryAlias": alias, "QueryParameters": ["ConsoleLogin"] }),
        )
        .await;
    refused.expect_error(
        "parameters for a query template",
        400,
        "UnsupportedOperationException",
    );
    assert_eq!(
        emulator.state().lock().queries.len(),
        1,
        "case parameters for a query template: the refused request persists nothing"
    );
}

/// The store the statement resolved to has to be the one the request addresses:
/// `EventDataStoreOwnerAccountId` is the account that owns it, and a store
/// awaiting deletion serves no query.
#[tokio::test]
async fn the_store_the_statement_names_must_be_the_one_the_request_addresses() {
    let emulator = Emulator::new();
    let shared = "dddddddd-1111-2222-3333-444444444444";
    let owner = "210987654321";
    seed_store_owned_by(
        &emulator,
        shared,
        HOME_REGION,
        EventDataStoreState::Enabled,
        Some(owner),
    );
    let awaiting_deletion = "eeeeeeee-1111-2222-3333-444444444444";
    seed_store(
        &emulator,
        awaiting_deletion,
        HOME_REGION,
        EventDataStoreState::PendingDeletion,
    );

    // The owner account the store belongs to: the run starts, and the response
    // reports the owner the request named.
    let reply = emulator
        .call(
            "StartQuery",
            &json!({
                "QueryStatement": format!("SELECT eventName FROM {shared}"),
                "EventDataStoreOwnerAccountId": owner,
            }),
        )
        .await;
    let body = reply.expect_ok("the owning account");
    assert_eq!(
        body.get("EventDataStoreOwnerAccountId")
            .and_then(Value::as_str),
        Some(owner),
        "case the owning account: expected the owner echoed, got {body:?}"
    );
    let query_id = minted_query_id("the owning account", body);
    let reported = describe(&emulator, &query_id).await;
    assert_eq!(
        reported
            .get("EventDataStoreOwnerAccountId")
            .and_then(Value::as_str),
        Some(owner),
        "case the owning account: the run persists the owner, got {reported:?}"
    );

    for (case, body, status, code) in [
        (
            "an account that does not own the store",
            json!({
                "QueryStatement": format!("SELECT eventName FROM {shared}"),
                "EventDataStoreOwnerAccountId": "123456789012",
            }),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "a store awaiting deletion",
            json!({ "QueryStatement": format!("SELECT eventName FROM {awaiting_deletion}") }),
            400,
            "InactiveEventDataStoreException",
        ),
    ] {
        let reply = emulator.call("StartQuery", &body).await;
        reply.expect_error(case, status, code);
    }
}

/// A statement deep enough to recurse a SQL parser into the ground is REFUSED,
/// and the emulator serves the next request: a single `StartQuery` cannot take
/// the serving process down with it.
///
/// The rows span the depth a 10,000-character `QueryStatement` can ask for —
/// nested parentheses and chains of prefix operators, from the crate's bracket
/// bound ([`MAX_QUERY_NESTING_DEPTH`]) and one past it, past the parser's own
/// stated recursion limit ([`QUERY_RECURSION_LIMIT`]) and on to thousands of
/// levels — and the width an operator chain can ask for, at
/// [`MAX_QUERY_TOKENS`] and one past it — while a statement that is merely LONG,
/// or merely arithmetic-heavy, is a query like any other.
#[tokio::test]
async fn a_statement_cannot_recurse_the_parser_into_the_ground() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    for (case, statement) in [
        (
            "nesting one past the bound",
            format!(
                "SELECT {}1{} FROM {STORE_ID}",
                "(".repeat(MAX_QUERY_NESTING_DEPTH + 1),
                ")".repeat(MAX_QUERY_NESTING_DEPTH + 1)
            ),
        ),
        (
            "past the parser's own recursion limit",
            format!(
                "SELECT {}1{} FROM {STORE_ID}",
                "(".repeat(QUERY_RECURSION_LIMIT + 2),
                ")".repeat(QUERY_RECURSION_LIMIT + 2)
            ),
        ),
        (
            "a thousand nested parentheses",
            format!(
                "SELECT {}1{} FROM {STORE_ID}",
                "(".repeat(1_000),
                ")".repeat(1_000)
            ),
        ),
        (
            "a chain of unary minuses",
            format!("SELECT {}1 FROM {STORE_ID}", "-".repeat(2_000)),
        ),
        (
            "a chain of NOTs",
            format!(
                "SELECT eventName FROM {STORE_ID} WHERE {}true",
                "NOT ".repeat(1_000)
            ),
        ),
        (
            // Brackets do not express every descent: an operator chain nests the
            // TREE at bracket depth zero, so the TOKEN bound is what holds it.
            "an operator chain one token past the bound",
            format!(
                "SELECT {} FROM {STORE_ID}",
                vec!["1"; MAX_QUERY_TOKENS / 2 + 1].join(" - ")
            ),
        ),
    ] {
        let reply = start(&emulator, &statement).await;
        reply.expect_error(case, 400, "InvalidQueryStatementException");
        assert!(
            emulator.state().lock().queries.is_empty(),
            "case {case:?}: a refused query persists nothing"
        );
    }

    // Length is not depth: a statement at nearly the modelled maximum of 10,000
    // characters, carrying one parenthesised list, is a query like any other —
    // and the emulator is still serving, which is what the rows above assert.
    let mut values: Vec<String> = Vec::new();
    while values.join(",").len() < 9_000 {
        values.push(format!("'event-{}'", values.len()));
    }
    let long = format!(
        "SELECT eventName FROM {STORE_ID} WHERE eventName IN ({})",
        values.join(",")
    );
    assert!(
        long.len() < 10_000,
        "case a long statement: the modelled maximum is 10,000 characters, got {}",
        long.len()
    );
    let reply = start(&emulator, &long).await;
    minted_query_id("a long statement", reply.expect_ok("a long statement"));

    // Depth is not arithmetic either: a long chain of binary operators, and the
    // `FROM` SQL spells inside a function call, are read as what they are — and a
    // statement AT each bound is still a query, which is the other side of the
    // rows above.
    for (case, statement) in [
        (
            "nesting at the bound",
            format!(
                "SELECT {}1{} FROM {STORE_ID}",
                "(".repeat(MAX_QUERY_NESTING_DEPTH),
                ")".repeat(MAX_QUERY_NESTING_DEPTH)
            ),
        ),
        (
            "a long arithmetic expression",
            format!(
                "SELECT {} FROM {STORE_ID}",
                (0..200)
                    .map(|nth| nth.to_string())
                    .collect::<Vec<String>>()
                    .join(" - ")
            ),
        ),
        (
            // Just INSIDE the token bound, which is the other side of the row
            // above: a wide statement is a query like any other.
            "an operator chain just inside the token bound",
            format!(
                "SELECT {} FROM {STORE_ID}",
                vec!["1"; MAX_QUERY_TOKENS / 2 - 2].join(" - ")
            ),
        ),
        (
            "FROM inside EXTRACT",
            format!("SELECT extract(year FROM eventTime) FROM {STORE_ID}"),
        ),
        (
            "FROM inside SUBSTRING",
            format!("SELECT substring(eventName FROM 1 FOR 3) FROM {STORE_ID}"),
        ),
    ] {
        let reply = start(&emulator, &statement).await;
        minted_query_id(case, reply.expect_ok(case));
    }
}

/// A new run is `QUEUED`, and the clock — not a reader — settles it into
/// `FINISHED` with the statistics of the store it read: this emulator ingests no
/// CloudTrail events, so the run scanned and matched none.
#[tokio::test]
async fn a_new_run_is_queued_and_the_clock_finishes_it() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    let statement = format!("SELECT eventName FROM {STORE_ID}");
    let reply = start(&emulator, &statement).await;
    let query_id = minted_query_id("a new run", reply.expect_ok("a new run"));

    // Straight after the call the run has not ended: the span is never zero.
    let queued = describe(&emulator, &query_id).await;
    assert_eq!(
        queued.get("QueryStatus").and_then(Value::as_str),
        Some(QueryState::Queued.as_wire()),
        "case queued: expected QUEUED straight after the call, got {queued:?}"
    );
    assert!(
        queued
            .get("QueryStatistics")
            .and_then(|statistics| statistics.get("ExecutionTimeInMillis"))
            .is_none(),
        "case queued: a run that has not ended has no run time, got {queued:?}"
    );

    tokio::time::sleep(past_settle()).await;

    let finished = describe(&emulator, &query_id).await;
    assert_eq!(
        finished.get("QueryStatus").and_then(Value::as_str),
        Some(QueryState::Finished.as_wire()),
        "case finished: expected FINISHED once the run was due, got {finished:?}"
    );
    let statistics = finished
        .get("QueryStatistics")
        .cloned()
        .unwrap_or(Value::Null);
    for member in ["EventsMatched", "EventsScanned", "BytesScanned"] {
        assert_eq!(
            statistics.get(member).and_then(Value::as_i64),
            Some(0),
            "case finished {member}: the store held no events, got {statistics:?}"
        );
    }
    let took = statistics
        .get("ExecutionTimeInMillis")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let span = i64::try_from(state::QUERY_SETTLE_AFTER.as_millis()).expect("the span in millis");
    assert_eq!(
        took, span,
        "case finished: the run took the span from queued to ended ({span} ms), got {took}"
    );
    // A reader does not re-run the query: the run time it reports is the one the
    // run took, however long after it anybody asks.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let again = describe(&emulator, &query_id).await;
    assert_eq!(
        again, finished,
        "case described twice: expected the same report, got {again:?}"
    );
}

/// "You are already running the maximum number of concurrent queries. The
/// maximum number of concurrent queries is 10": the limit counts the runs that
/// have NOT ended — a started run included — and a run that has ended no longer
/// holds a slot.
///
/// The runs that fill the limit are seeded with a settle instant an hour out, so
/// what the case asserts is the rule and not how fast the machine ran it; the
/// last slot is taken by a real `StartQuery`, so a minted run is counted too.
#[tokio::test]
async fn the_concurrency_limit_bounds_the_runs_in_flight() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    let statement = format!("SELECT eventName FROM {STORE_ID}");

    let in_flight = state::MAX_CONCURRENT_QUERIES - 1;
    for nth in 0..in_flight {
        let nth = u8::try_from(nth).expect("fewer runs than a byte holds");
        emulator.state().lock().queries.insert(
            seeded_query_id(nth),
            QueryRecord {
                query_id: seeded_query_id(nth),
                event_data_store_id: STORE_ID.to_string(),
                region: HOME_REGION.to_string(),
                query_string: statement.clone(),
                status: QueryState::Queued,
                created_at: Some(SystemTime::now()),
                settles_at: Some(SystemTime::now() + Duration::from_secs(3_600)),
                ..QueryRecord::default()
            },
        );
    }

    // The last slot, taken by a run this emulator minted.
    let last = start(&emulator, &statement).await;
    let last_id = minted_query_id("the last slot", last.expect_ok("the last slot"));

    let refused = start(&emulator, &statement).await;
    refused.expect_error(
        "one run past the limit",
        429,
        "MaxConcurrentQueriesException",
    );
    assert_eq!(
        emulator.state().lock().queries.len(),
        state::MAX_CONCURRENT_QUERIES,
        "case one run past the limit: the refused run persists nothing"
    );

    // End the seeded runs the way the clock would, and the next request has a
    // slot again.
    {
        let state = emulator.state();
        let mut store = state.lock();
        for nth in 0..in_flight {
            let nth = u8::try_from(nth).expect("fewer runs than a byte holds");
            if let Some(record) = store.queries.get_mut(&seeded_query_id(nth)) {
                record.status = QueryState::Finished;
                record.settles_at = None;
            }
        }
    }
    let after = start(&emulator, &statement).await;
    let after_id = minted_query_id(
        "a run after the others ended",
        after.expect_ok("a run after the others ended"),
    );
    assert_ne!(
        after_id, last_id,
        "case a run after the others ended: expected a run of its own, got {after_id}"
    );
}

/// "Use the optional DeliveryS3Uri parameter to deliver the query results to an
/// S3 bucket": the URI is persisted with the modelled `PENDING` delivery status,
/// and a run that asked for no delivery reports neither member.
#[tokio::test]
async fn a_delivery_uri_is_persisted_with_a_pending_delivery() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    let statement = format!("SELECT eventName FROM {STORE_ID}");

    for (case, s3_uri) in [
        ("a bucket", Some("s3://lake-results")),
        ("a bucket and a prefix", Some("s3://lake-results/start-query/")),
        ("no delivery", None),
    ] {
        let mut request = json!({ "QueryStatement": statement });
        if let Some(s3_uri) = s3_uri {
            request["DeliveryS3Uri"] = json!(s3_uri);
        }
        let reply = emulator.call("StartQuery", &request).await;
        let query_id = minted_query_id(case, reply.expect_ok(case));
        let reported = describe(&emulator, &query_id).await;
        assert_eq!(
            reported.get("DeliveryS3Uri").and_then(Value::as_str),
            s3_uri,
            "case {case:?}: expected DeliveryS3Uri {s3_uri:?}, got {reported:?}"
        );
        assert_eq!(
            reported.get("DeliveryStatus").and_then(Value::as_str),
            s3_uri.map(|_| state::DELIVERY_STATUS_PENDING),
            "case {case:?}: the URI and the status travel together, got {reported:?}"
        );
    }
}

/// The registry is bounded: the oldest runs that have already ended are dropped
/// first, and a run still in flight is never dropped. `StartQuery` applies this
/// at [`state::QUERY_HISTORY_LIMIT`]; the rule is the same at any limit.
#[test]
fn the_query_history_drops_the_oldest_ended_runs_first() {
    let mut store =
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::ComAmazonawsCloudtrailCloudtrail20131101State::default();
    let at = |secs: u64| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
    for (query_id, status, created_at) in [
        ("oldest-ended", QueryState::Finished, at(1_000)),
        ("newer-ended", QueryState::Cancelled, at(2_000)),
        ("newest-ended", QueryState::Finished, at(3_000)),
        ("in-flight", QueryState::Queued, at(1)),
    ] {
        store.queries.insert(
            query_id.to_string(),
            QueryRecord {
                query_id: query_id.to_string(),
                region: HOME_REGION.to_string(),
                status,
                created_at,
                ..QueryRecord::default()
            },
        );
    }

    store.trim_query_history(2);

    let mut kept: Vec<&str> = store.queries.keys().map(String::as_str).collect();
    kept.sort_unstable();
    assert_eq!(
        kept,
        ["in-flight", "newest-ended"],
        "case limit 2: expected the newest ended run and the one in flight, got {kept:?}"
    );

    // A limit no terminal run can satisfy leaves the run in flight alone.
    store.trim_query_history(0);
    let kept: Vec<&str> = store.queries.keys().map(String::as_str).collect();
    assert_eq!(
        kept, ["in-flight"],
        "case limit 0: a run in flight is never dropped, got {kept:?}"
    );
}

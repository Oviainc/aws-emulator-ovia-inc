// SPDX-License-Identifier: CC0-1.0

//! What `StartQueryExecution` MINTS: one query execution, and one id, per accepted
//! request.
//!
//! `StartQueryExecutionOutput$QueryExecutionId` is "the unique ID of the query that ran
//! as a result of THIS request", and the documented collapse of two submits into one
//! execution is conditioned on a `ClientRequestToken`: "If another StartQueryExecution
//! request is received [with a previous client request token], the same response is
//! returned and another query is not created." So every submit that does not reuse a
//! token is a query of its own, however alike the requests are — and each id addresses
//! a persisted execution carrying that request's own parameters.
//!
//! Driven over the wire (awsJson1_1, `x-amz-target: AmazonAthena.<Operation>`) through
//! the same router the emulator boots, so each assertion is about what an SDK client
//! observes. Every test states that rule and asserts it over a TABLE whose rows differ
//! along what the rule generalises over: the statement submitted (including the same
//! text twice), how many times it is submitted, the workgroup and the database it runs
//! in, and how tokens are spread over the submits.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::DEFAULT_WORK_GROUP;
use service_com_amazonaws_athena_amazonathena::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `x-amz-target` prefix of this service: the local name of the Smithy service
/// shape (`com.amazonaws.athena#AmazonAthena`), which is how the awsJson protocols
/// build that header.
const TARGET_PREFIX: &str = "AmazonAthena";

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `athena` (this service's `sigv4Name`). Neither the key id
/// nor the signature is a credential: the harness turns request authentication off
/// below, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/athena/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=not-verified";

/// Where the queries in these tests write their results: a query has to name a location
/// or inherit one. Nothing is written to it.
const OUTPUT_LOCATION: &str = "s3://athena-results/identity/";

/// The modelled bound on `QueryExecutionId` ("len 1..128, pattern ^\S+$", from the
/// `QueryExecutionId` shape of this service's model).
const MAX_ID_CHARS: usize = 128;

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Athena's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Invoke one operation and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "athena.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Start one query and return the `QueryExecutionId` it minted.
async fn start(router: &MultiServiceRouter, body: &Value) -> String {
    let (status, answer) = call(router, "StartQueryExecution", body).await;
    assert_eq!(
        status, 200,
        "StartQueryExecution({body}) must be accepted, got {status} with {answer:?}"
    );
    answer["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQueryExecution must answer with an id, got {answer:?}"))
        .to_string()
}

/// The stored `QueryExecution` of `id`.
async fn execution(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, body) = call(router, "GetQueryExecution", &json!({ "QueryExecutionId": id })).await;
    assert_eq!(
        status, 200,
        "GetQueryExecution({id}) must answer 200, got {status} with {body:?}"
    );
    body["QueryExecution"].clone()
}

/// The ids `work_group`'s history holds.
async fn history(router: &MultiServiceRouter, work_group: &str) -> Vec<String> {
    let (status, listed) = call(
        router,
        "ListQueryExecutions",
        &json!({ "WorkGroup": work_group, "MaxResults": 50 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListQueryExecutions({work_group}) must answer 200, got {status} with {listed:?}"
    );
    listed["QueryExecutionIds"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// Create the workgroup `name`, with no configuration of its own.
async fn create_work_group(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(router, "CreateWorkGroup", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "the workgroup {name} must be created, got {status} with {created:?}"
    );
}

/// A `ClientRequestToken` of the modelled length (32 to 128 characters), built from
/// `seed` so each caller carries its own.
fn request_token(seed: &str) -> String {
    format!("{seed:-<32}")
}

/// Assert `id` satisfies the modelled `QueryExecutionId` constraints.
fn assert_well_formed(case: &str, id: &str) {
    assert!(
        !id.is_empty() && id.chars().count() <= MAX_ID_CHARS,
        "case {case:?}: a QueryExecutionId holds 1 to {MAX_ID_CHARS} characters, got \
         {id:?} of {}",
        id.chars().count()
    );
    assert!(
        !id.chars().any(char::is_whitespace),
        "case {case:?}: a QueryExecutionId carries no whitespace (^\\S+$), got {id:?}"
    );
}

/// One row of the minting table: a request, submitted `submits` times with no
/// `ClientRequestToken` at all.
struct Submission {
    /// What this row varies.
    case: &'static str,
    /// The statement submitted.
    sql: &'static str,
    /// The workgroup the query runs in, or `None` for Athena's own default workgroup.
    work_group: Option<&'static str>,
    /// The database the query's context names, or `None` for no context at all.
    database: Option<&'static str>,
    /// How many times this same request is submitted.
    submits: usize,
}

/// The rows: the statement differs in type and in text (two rows submit the SAME text),
/// the number of submits differs, and a row runs in a workgroup or a database of its
/// own.
fn submissions() -> Vec<Submission> {
    vec![
        Submission {
            case: "a projection submitted twice",
            sql: "SELECT 1 AS one",
            work_group: None,
            database: None,
            submits: 2,
        },
        Submission {
            case: "the same text as another row, submitted three times",
            sql: "SELECT 1 AS one",
            work_group: None,
            database: None,
            submits: 3,
        },
        Submission {
            case: "an expression the engine evaluates",
            sql: "SELECT 2 + 3 AS computed",
            work_group: None,
            database: None,
            submits: 1,
        },
        Submission {
            case: "a definition statement submitted twice",
            sql: "CREATE TABLE identity_ddl (id integer)",
            work_group: None,
            database: None,
            submits: 2,
        },
        Submission {
            case: "a utility statement",
            sql: "EXPLAIN SELECT 1",
            work_group: None,
            database: None,
            submits: 1,
        },
        Submission {
            case: "a query in a workgroup of its own",
            sql: "SELECT 'in a workgroup' AS where_it_ran",
            work_group: Some("identity-work-group"),
            database: None,
            submits: 2,
        },
        Submission {
            case: "a query in a database of its own",
            sql: "SELECT 'in a database' AS where_it_ran",
            work_group: None,
            database: Some("identity_database"),
            submits: 2,
        },
    ]
}

/// The request of one row.
fn request_of(row: &Submission) -> Value {
    let mut request = json!({
        "QueryString": row.sql,
        "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
    });
    if let Some(work_group) = row.work_group {
        request["WorkGroup"] = json!(work_group);
    }
    if let Some(database) = row.database {
        request["QueryExecutionContext"] = json!({ "Database": database });
    }
    request
}

/// Every submit that reuses no `ClientRequestToken` mints an execution of its OWN: a
/// fresh, well-formed id, a record carrying that request's parameters, and one more
/// entry in the history of the workgroup it ran in — even when the request is byte for
/// byte the one submitted before it.
#[tokio::test]
async fn every_submit_without_a_token_mints_its_own_execution() {
    let router = router();
    create_work_group(&router, "identity-work-group").await;

    // (the row, the ids its submits minted), and the ids per workgroup for the history
    // assertions below.
    let rows = submissions();
    let mut minted: Vec<(&Submission, Vec<String>)> = Vec::new();
    let mut per_work_group: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in &rows {
        let request = request_of(row);
        let mut ids = Vec::new();
        for submit in 0..row.submits {
            let id = start(&router, &request).await;
            assert_well_formed(row.case, &id);
            assert!(
                !ids.contains(&id),
                "case {:?}: submit {submit} must mint an id of its own, got {id:?} again \
                 among {ids:?}",
                row.case
            );
            per_work_group
                .entry(row.work_group.unwrap_or(DEFAULT_WORK_GROUP).to_string())
                .or_default()
                .push(id.clone());
            ids.push(id);
        }
        minted.push((row, ids));
    }

    // No two submits anywhere in the table share an id: the id names THIS query.
    let all: Vec<&String> = minted.iter().flat_map(|(_, ids)| ids).collect();
    for (position, id) in all.iter().enumerate() {
        let repeats = all.iter().filter(|other| other == &id).count();
        assert_eq!(
            repeats, 1,
            "the id of submit {position} must be unique across the table, got {id:?} \
             {repeats} times in {all:?}"
        );
    }

    // Each id addresses an execution of its own, carrying the parameters of the request
    // that minted it.
    for (row, ids) in &minted {
        for id in ids {
            let found = execution(&router, id).await;
            assert_eq!(
                found["QueryExecutionId"].as_str(),
                Some(id.as_str()),
                "case {:?}: the record of {id} must report its own id, got {found:?}",
                row.case
            );
            assert_eq!(
                found["Query"].as_str(),
                Some(row.sql),
                "case {:?}: the record of {id} must carry the submitted text, got {found:?}",
                row.case
            );
            assert_eq!(
                found["WorkGroup"].as_str(),
                Some(row.work_group.unwrap_or(DEFAULT_WORK_GROUP)),
                "case {:?}: the record of {id} must name the workgroup it ran in, got \
                 {found:?}",
                row.case
            );
            assert_eq!(
                found["QueryExecutionContext"]["Database"].as_str(),
                row.database,
                "case {:?}: the record of {id} must carry its context database, got \
                 {found:?}",
                row.case
            );
        }
    }

    // One history entry per submit, in the workgroup the submit named: nothing was
    // collapsed and nothing was overwritten.
    for (work_group, expected) in &per_work_group {
        let listed = history(&router, work_group).await;
        assert_eq!(
            listed.len(),
            expected.len(),
            "the history of {work_group} must hold one entry per submit: expected \
             {} ids, got {listed:?}",
            expected.len()
        );
        for id in expected {
            assert!(
                listed.contains(id),
                "the history of {work_group} must hold {id}, got {listed:?}"
            );
        }
    }
}

/// One row of the token table: how tokens are spread over several submits of the SAME
/// request, and how many executions that leaves.
struct TokenRun {
    /// What this row varies.
    case: &'static str,
    /// The statement every submit of this row carries.
    sql: &'static str,
    /// The token of each submit, in order; `None` for a submit that carries none.
    tokens: Vec<Option<&'static str>>,
    /// How many distinct executions the submits must have minted between them.
    executions: usize,
}

/// The rows: the same request submitted under one token, under two tokens, with and
/// without a token, and a longer run mixing them.
fn token_runs() -> Vec<TokenRun> {
    vec![
        TokenRun {
            case: "one token, submitted twice",
            sql: "SELECT 10 AS n",
            tokens: vec![Some("one-token"), Some("one-token")],
            executions: 1,
        },
        TokenRun {
            case: "two tokens over the same request",
            sql: "SELECT 11 AS n",
            tokens: vec![Some("first-token"), Some("second-token")],
            executions: 2,
        },
        TokenRun {
            case: "a token and then no token",
            sql: "SELECT 12 AS n",
            tokens: vec![Some("third-token"), None],
            executions: 2,
        },
        TokenRun {
            case: "no token and then a token",
            sql: "SELECT 13 AS n",
            tokens: vec![None, Some("fourth-token")],
            executions: 2,
        },
        TokenRun {
            case: "two tokens, each repeated, around an untokened submit",
            sql: "SELECT 14 AS n",
            tokens: vec![
                Some("fifth-token"),
                Some("sixth-token"),
                None,
                Some("fifth-token"),
                Some("sixth-token"),
            ],
            executions: 3,
        },
    ]
}

/// A submit joins an earlier execution ONLY by reusing its `ClientRequestToken`: the
/// same token answers with the id it first minted, a different token mints another
/// execution of the same query, and a submit carrying no token is always a query of its
/// own.
#[tokio::test]
async fn only_a_reused_token_collapses_submits_of_the_same_query() {
    let router = router();

    for (index, row) in token_runs().into_iter().enumerate() {
        // Each row runs in a workgroup of its own, so the history it leaves is its own.
        let work_group = format!("token-run-{index}");
        create_work_group(&router, &work_group).await;

        let mut ids = Vec::new();
        let mut by_token: BTreeMap<&str, String> = BTreeMap::new();
        for (submit, token) in row.tokens.iter().enumerate() {
            let mut request = json!({
                "QueryString": row.sql,
                "WorkGroup": work_group,
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
            });
            if let Some(token) = token {
                request["ClientRequestToken"] = json!(request_token(token));
            }
            let id = start(&router, &request).await;
            assert_well_formed(row.case, &id);
            if let Some(token) = token {
                match by_token.get(*token) {
                    // "the same response is returned and another query is not created"
                    Some(first) => assert_eq!(
                        &id, first,
                        "case {:?}: submit {submit} reuses the token {token:?}, so it must \
                         answer the id {first} it first minted, got {id}",
                        row.case
                    ),
                    None => {
                        by_token.insert(token, id.clone());
                    }
                }
            } else {
                assert!(
                    !ids.contains(&id),
                    "case {:?}: submit {submit} carries no token, so it must mint an id of \
                     its own, got {id} again among {ids:?}",
                    row.case
                );
            }
            ids.push(id);
        }

        let mut distinct = ids.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            row.executions,
            "case {:?}: {} submits must leave {} execution(s), got the ids {ids:?}",
            row.case,
            row.tokens.len(),
            row.executions
        );

        // And state agrees with the answers: the workgroup's history holds exactly the
        // executions the submits minted.
        let listed = history(&router, &work_group).await;
        assert_eq!(
            listed.len(),
            row.executions,
            "case {:?}: the history of {work_group} must hold {} entries, got {listed:?}",
            row.case,
            row.executions
        );
        for id in &distinct {
            assert!(
                listed.contains(id),
                "case {:?}: the history of {work_group} must hold {id}, got {listed:?}",
                row.case
            );
        }
    }
}

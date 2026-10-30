// SPDX-License-Identifier: CC0-1.0

//! The WORDS the Data API refuses a request with, driven over the wire.
//!
//! Four rules, each asserted over a TABLE of inputs rather than one request,
//! because each is a rule about a CLASS of requests:
//!
//! 1. a value outside a modelled constraint is refused with the constraint
//!    envelope QUOTING the value the caller submitted — verbatim for a string,
//!    however long it is, as its digits for a number, as its bracketed elements
//!    for a list, and as `null` for a member that was absent;
//! 2. a `SessionId` that names no session is RESOLVED and refused as a missing
//!    resource, in fixed words that carry no part of the id — not degraded to a
//!    filter that matches nothing;
//! 3. members the operation documents as mutually exclusive are refused with
//!    the service's mutual-exclusion sentence over exactly those members;
//! 4. a request that does not select exactly one compute target is refused with
//!    a sentence enumerating the target selectors THAT operation accepts, and no
//!    others.

use serde_json::{json, Value};

use super::emulator::{error_code, list, Emulator};

/// A `SessionId` in the form the `UUID` pattern accepts
/// (`smithy_query(query="operation",operation="ListSessions")` ->
/// `input.SessionId.pattern`) that no session was ever minted with.
const ABSENT_SESSION_ID: &str = "3dc6a4b5-5e2b-4b5a-9f1d-6b6d2b6c7a8e";

/// The connection a statement in this suite runs through.
fn cluster() -> Value {
    json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"})
}

/// The `Message` a handler's own refusal carries.
fn message(response: &Value) -> Option<&str> {
    response["Message"].as_str()
}

/// The `message` the boundary constraint envelope carries (the LOWERCASE key: a
/// handler's own `ValidationException` carries `Message`).
fn constraint_message(response: &Value) -> String {
    response["message"].as_str().unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: a constraint refusal quotes the value that was submitted
// ---------------------------------------------------------------------------

/// One row of the constraint-envelope table: the request, the member path the
/// clause names, the value half it states, and the requirement it ends with.
struct Quoted {
    case: &'static str,
    operation: &'static str,
    request: Value,
    path: &'static str,
    /// The value half of the clause, as the envelope writes it: `Value '<x>'`
    /// for a value that was submitted, `Value null` for a member that was not.
    value_half: String,
    requirement: String,
}

/// RULE: the clause a boundary refusal states quotes the value the CALLER
/// submitted, between `Value ` and ` at '<member path>'`, rendered the way a
/// value of that member's type renders.
///
/// The rows differ along what the rule generalises over — the value's TYPE
/// (list, string, integer), its SIZE (empty, one past a bound, far past it), its
/// FORM (a rendered collection, a verbatim string, digits, an absent member) and
/// the constraint class it broke (`@length` either way, `@range`, `@pattern`,
/// `@required`) — so what is asserted is the RENDERING and not one member's text.
#[tokio::test]
async fn a_constraint_refusal_quotes_the_value_the_caller_submitted() {
    let emulator = Emulator::new();

    // `SqlList` is `@length(min 1, max 40)` over `String`, so a list one past
    // its maximum is 41 statements.
    let over_long_batch: Vec<&str> = vec!["SELECT 1"; 41];
    // `StatementNameString` is `@length(max 2048)`
    // (`smithy_query(query="operation",operation="ExecuteStatement")`), so one
    // character past its maximum is 2049.
    let one_past_the_name_max = "a".repeat(2049);
    let far_past_the_name_max = "b".repeat(5_000);

    let rows: [Quoted; 8] = [
        // An EMPTY list renders as its own brackets: there is nothing between
        // them to state.
        Quoted {
            case: "an empty list",
            operation: "BatchExecuteStatement",
            request: json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "Sqls": []}),
            path: "sqls",
            value_half: "Value '[]'".to_string(),
            requirement: "Member must have length greater than or equal to 1".to_string(),
        },
        // A NON-EMPTY list renders as its elements, in order, comma-separated.
        Quoted {
            case: "a list one past its maximum",
            operation: "BatchExecuteStatement",
            request: json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "Sqls": over_long_batch,
            }),
            path: "sqls",
            value_half: format!("Value '[{}]'", vec!["SELECT 1"; 41].join(", ")),
            requirement: "Member must have length less than or equal to 40".to_string(),
        },
        // A string is quoted VERBATIM, however long it is: the echo is the
        // caller's own bytes and never a shortened rendering of them.
        Quoted {
            case: "a string one character past its maximum",
            operation: "ExecuteStatement",
            request: json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "Sql": "SELECT 1",
                "StatementName": one_past_the_name_max.clone(),
            }),
            path: "statementName",
            value_half: format!("Value '{one_past_the_name_max}'"),
            requirement: "Member must have length less than or equal to 2048".to_string(),
        },
        Quoted {
            case: "a string far past its maximum",
            operation: "ExecuteStatement",
            request: json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "Sql": "SELECT 1",
                "StatementName": far_past_the_name_max.clone(),
            }),
            path: "statementName",
            value_half: format!("Value '{far_past_the_name_max}'"),
            requirement: "Member must have length less than or equal to 2048".to_string(),
        },
        // An EMPTY string is a value that was submitted, so it is quoted as the
        // nothing it is — which is a different clause from an absent member's.
        Quoted {
            case: "an empty string below its minimum",
            operation: "ExecuteStatement",
            request: json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "Sql": "SELECT 1",
                "ClientToken": "",
            }),
            path: "clientToken",
            value_half: "Value ''".to_string(),
            requirement: "Member must have length greater than or equal to 1".to_string(),
        },
        // A number is quoted as its digits.
        Quoted {
            case: "an integer above its range",
            operation: "ExecuteStatement",
            request: json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "Sql": "SELECT 1",
                "SessionKeepAliveSeconds": 86_401,
            }),
            path: "sessionKeepAliveSeconds",
            value_half: "Value '86401'".to_string(),
            requirement: "Member must have value less than or equal to 86400".to_string(),
        },
        // A string outside its `@pattern` is quoted too, beside the expression
        // it has to satisfy.
        Quoted {
            case: "a string outside its pattern",
            operation: "ExecuteStatement",
            request: json!({
                "ClusterIdentifier": "Bad_Cluster",
                "Database": "dev",
                "Sql": "SELECT 1",
            }),
            path: "clusterIdentifier",
            value_half: "Value 'Bad_Cluster'".to_string(),
            requirement: "Member must satisfy regular expression pattern: \
                          [a-z][a-z0-9]*(-[a-z0-9]+)*"
                .to_string(),
        },
        // An ABSENT member has no value to quote, and is stated as the null it
        // was.
        Quoted {
            case: "a required member that is absent",
            operation: "BatchExecuteStatement",
            request: json!({"ClusterIdentifier": "my-cluster", "Database": "dev"}),
            path: "sqls",
            value_half: "Value null".to_string(),
            requirement: "Member must not be null".to_string(),
        },
    ];

    for row in &rows {
        let (status, response) = emulator.call(row.operation, &row.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} {response}",
            row.case
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {:?}: expected ValidationException, got {response}",
            row.case
        );
        let expected = format!(
            "1 validation error detected: {} at '{}' failed to satisfy constraint: {}",
            row.value_half, row.path, row.requirement
        );
        let stated = constraint_message(&response);
        assert_eq!(
            stated, expected,
            "case {:?}: expected {expected:?}, got {stated:?}",
            row.case
        );
    }

    // Nothing above was admitted: a refused request stores no statement.
    let page = emulator.ok("ListStatements", &json!({})).await;
    assert!(
        list(&page, "Statements").is_empty(),
        "a refused request must store no statement, got {page}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a SessionId that names no session
// ---------------------------------------------------------------------------

/// RULE: a `SessionId` names ONE session, so every operation that takes one
/// resolves it against session state: an id the account holds no session for is
/// the missing-resource error in fixed words, never an empty answer.
#[tokio::test]
async fn a_session_id_that_names_no_session_is_refused_as_a_missing_resource() {
    let emulator = Emulator::new();

    // Each row is a `SessionId` the `UUID` pattern accepts that names no
    // session: three different ids, and one carrying the `:<n>` suffix the
    // pattern allows — so the rule is about the id not resolving and never about
    // its spelling.
    let ids = [
        ("an id no session was minted with", ABSENT_SESSION_ID.to_string()),
        (
            "another id no session was minted with",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        (
            "an all-zero id",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the suffix form of an id no session was minted with",
            format!("{ABSENT_SESSION_ID}:1"),
        ),
    ];

    for (case, id) in &ids {
        // The metadata read answers this refusal as a client-input failure; the
        // submit operations answer the same error at the status its `@httpError`
        // declares. The CODE and the WORDS are the same on every path.
        let paths: [(&str, Value, u16); 3] = [
            ("ListSessions", json!({"SessionId": id}), 400),
            (
                "ExecuteStatement",
                json!({"SessionId": id, "Sql": "SELECT 1"}),
                404,
            ),
            (
                "BatchExecuteStatement",
                json!({"SessionId": id, "Sqls": ["SELECT 1"]}),
                404,
            ),
        ];
        for (operation, request, expected_status) in &paths {
            let (status, response) = emulator.call(operation, request).await;
            assert_eq!(
                status, *expected_status,
                "case {case:?} on {operation}: expected {expected_status}, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {response}"
            );
            assert_eq!(
                message(&response),
                Some("Session not found."),
                "case {case:?} on {operation}: expected the fixed wording, got {response}"
            );
            assert!(
                !message(&response).unwrap_or_default().contains(id.as_str()),
                "case {case:?} on {operation}: the message carries no part of the id, got \
                 {response}"
            );
        }
    }

    // The control: a session that EXISTS is reported, so the refusal above is
    // about the id naming no session and never about `SessionId` being supplied.
    let mut opening = cluster();
    opening["Sql"] = json!("SELECT 1");
    opening["SessionKeepAliveSeconds"] = json!(300);
    let opened = emulator.execute_and_wait(&opening).await;
    let session = opened["SessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("asking to keep a session alive must mint one: {opened}"))
        .to_string();
    let page = emulator
        .ok("ListSessions", &json!({"SessionId": session}))
        .await;
    let reported = list(&page, "Sessions");
    assert_eq!(
        reported.len(),
        1,
        "the session that exists must be reported, got {page}"
    );
    assert_eq!(
        reported[0]["SessionId"], json!(session),
        "the reported session must be the one asked for, got {page}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: members documented as mutually exclusive
// ---------------------------------------------------------------------------

/// RULE: a request that supplies members the operation documents as mutually
/// exclusive is refused with the sentence that rule is stated in, over exactly
/// the members it names — the mutual-exclusion sentence for a conflict between
/// the two TARGET selectors, and `SessionId`'s own sentence for the filters it
/// excludes.
#[tokio::test]
async fn mutually_exclusive_members_are_refused_with_the_exclusion_sentence() {
    let emulator = Emulator::new();

    // "When providing ClusterIdentifier, then WorkgroupName can't be specified."
    const TARGETS: &str = "Only one of the following can be specified: ClusterIdentifier, \
                           WorkgroupName.";
    // "When you provide SessionId, you can't specify Status, ClusterIdentifier,
    // WorkgroupName, or Database" — stated as one sentence about `SessionId`,
    // which is a different rule from the conflict between the two TARGET
    // selectors and so carries its own recorded wording.
    const WITH_SESSION_ID: &str = "SessionId cannot be specified with other filter parameters.";

    // Each row is a combination the operation documents as exclusive, and the
    // sentence that rule is stated with. The rows span BOTH rules the operation
    // documents and every member each one names.
    let rows: [(&str, Value, &str); 7] = [
        (
            "a cluster and a workgroup",
            json!({"ClusterIdentifier": "my-cluster", "WorkgroupName": "my-workgroup"}),
            TARGETS,
        ),
        (
            "a cluster and a workgroup named by ARN",
            json!({
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName":
                    "arn:aws:redshift-serverless:us-east-1:123456789012:workgroup/\
                     11111111-2222-4333-8444-555555555555",
            }),
            TARGETS,
        ),
        (
            "a session and a status",
            json!({"SessionId": ABSENT_SESSION_ID, "Status": "AVAILABLE"}),
            WITH_SESSION_ID,
        ),
        (
            "a session and a cluster",
            json!({"SessionId": ABSENT_SESSION_ID, "ClusterIdentifier": "my-cluster"}),
            WITH_SESSION_ID,
        ),
        (
            "a session and a workgroup",
            json!({"SessionId": ABSENT_SESSION_ID, "WorkgroupName": "my-workgroup"}),
            WITH_SESSION_ID,
        ),
        (
            "a session and a database",
            json!({"SessionId": ABSENT_SESSION_ID, "Database": "dev"}),
            WITH_SESSION_ID,
        ),
        (
            "a session and every filter it excludes",
            json!({
                "SessionId": ABSENT_SESSION_ID,
                "Status": "AVAILABLE",
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName": "my-workgroup",
                "Database": "dev",
            }),
            WITH_SESSION_ID,
        ),
    ];

    for (case, request, expected) in &rows {
        let (status, response) = emulator.call("ListSessions", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        assert_eq!(
            message(&response),
            Some(*expected),
            "case {case:?}: expected {expected:?}, got {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the selectors a target-selection refusal enumerates
// ---------------------------------------------------------------------------

/// RULE: the sentence refusing a request that does not select exactly one
/// compute target enumerates the target selectors THAT operation accepts, and no
/// others — so a caller is never sent after a parameter the operation does not
/// take.
///
/// The selector lists come from each operation's own input shape
/// (`smithy_query(query="operation",operation="ListDatabases,DescribeTable,ExecuteStatement,BatchExecuteStatement")`):
/// `ListDatabasesRequest` and `DescribeTableRequest` model `ClusterIdentifier`
/// and `WorkgroupName` and no other selector, while the two submit inputs model
/// `SessionId` as well and a submission may address a Glue-managed target.
#[tokio::test]
async fn a_target_selection_refusal_names_only_the_selectors_the_operation_accepts() {
    let emulator = Emulator::new();

    const WAREHOUSE_ONLY: &str = "There should be only one of the following: ClusterIdentifier, \
                                  WorkgroupName";
    const SUBMIT: &str = "There should be only one of the following: ClusterIdentifier, \
                          WorkgroupName, Glue resource, SessionId";

    // Each row is an operation, a selection that does not name exactly one
    // target, and the selectors that operation's refusal enumerates. The rows
    // span both ways a selection can be wrong (none, more than one) and both
    // selector sets this service has.
    let rows: [(&str, Value, &str); 8] = [
        (
            "ListDatabases naming no target",
            json!({"Database": "dev", "DbUser": "awsuser"}),
            WAREHOUSE_ONLY,
        ),
        (
            "ListDatabases naming two warehouses",
            json!({
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName": "my-workgroup",
                "Database": "dev",
                "DbUser": "awsuser",
            }),
            WAREHOUSE_ONLY,
        ),
        (
            "DescribeTable naming no target",
            json!({"Database": "dev", "DbUser": "awsuser", "Table": "t"}),
            WAREHOUSE_ONLY,
        ),
        (
            "DescribeTable naming two warehouses",
            json!({
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName": "my-workgroup",
                "Database": "dev",
                "DbUser": "awsuser",
                "Table": "t",
            }),
            WAREHOUSE_ONLY,
        ),
        (
            "ExecuteStatement naming no target",
            json!({"Database": "dev", "DbUser": "awsuser", "Sql": "SELECT 1"}),
            SUBMIT,
        ),
        (
            "ExecuteStatement naming a warehouse and a session",
            json!({
                "ClusterIdentifier": "my-cluster",
                "SessionId": ABSENT_SESSION_ID,
                "Database": "dev",
                "Sql": "SELECT 1",
            }),
            SUBMIT,
        ),
        (
            "BatchExecuteStatement naming no target",
            json!({"Database": "dev", "DbUser": "awsuser", "Sqls": ["SELECT 1"]}),
            SUBMIT,
        ),
        (
            "BatchExecuteStatement naming two warehouses",
            json!({
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName": "my-workgroup",
                "Database": "dev",
                "Sqls": ["SELECT 1"],
            }),
            SUBMIT,
        ),
    ];

    for (case, request, expected) in &rows {
        let operation = case.split(' ').next().unwrap_or_default();
        let (status, response) = emulator.call(operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        assert_eq!(
            message(&response),
            Some(*expected),
            "case {case:?}: expected {expected:?}, got {response}"
        );
        // The selectors the operation does NOT model are named nowhere in it.
        if *expected == WAREHOUSE_ONLY {
            let stated = message(&response).unwrap_or_default();
            for absent in ["Glue", "SessionId"] {
                assert!(
                    !stated.contains(absent),
                    "case {case:?}: {absent:?} is not a selector this operation models, got \
                     {stated:?}"
                );
            }
        }
    }
}

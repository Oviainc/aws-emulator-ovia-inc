// SPDX-License-Identifier: CC0-1.0

//! The Data API's ERROR SURFACE, driven over the wire: the status, the error code
//! and the MESSAGE the service answers a request it will not serve with.
//!
//! Every rule below is asserted over a TABLE of inputs rather than one recorded
//! request, because each is a rule about a CLASS of requests:
//!
//! 1. a submit request names ONE of the targets the service accepts — a
//!    provisioned cluster, a serverless workgroup, a Glue resource or a session —
//!    and naming none of them or more than one is refused with one canonical,
//!    request-independent message;
//! 2. an `Id` the service holds no query for is refused with the missing-resource
//!    error at HTTP 400 — the status the service answers that path with, not the
//!    shape's own — in fixed words that carry no part of the id asked for;
//! 3. a target is resolved against the service that OWNS it before a statement
//!    is admitted, so a cluster or workgroup that does not exist in the Region is
//!    refused and stores nothing — whether it is named by name or by ARN;
//! 4. a continuation token the service did not issue is refused with the one
//!    fixed message every paginated operation of the service shares;
//! 5. a member outside its modelled `@pattern` is refused with the constraint
//!    envelope, which quotes the pattern WITHOUT the anchors that wrap the whole
//!    value.
//!
//! Rules 1, 2, 4 and 5 run on the crate's own one-service harness. Rule 3 needs
//! the two Redshift control planes mounted beside the Data API, because the
//! cluster and the workgroup are THEIR resources: this suite creates them there
//! and then submits statements here.

use serde_json::{json, Value};

use super::emulator::{error_code, list, Emulator, Warehouses, ABSENT_ID};

/// AWS's canonical refusal of a request that does not select exactly one compute
/// target: the same sentence whatever the request named, enumerating every target
/// the service accepts.
const ONE_TARGET: &str = "There should be only one of the following: ClusterIdentifier, \
                          WorkgroupName, Glue resource, SessionId";

/// AWS's refusal of an `Id` it holds no query for.
const NO_SUCH_QUERY: &str = "Query does not exist.";

/// AWS's refusal of a target that resolves to no endpoint in the Region.
const NO_SUCH_ENDPOINT: &str = "Redshift endpoint doesn't exist in this region.";

/// AWS's refusal of a continuation token it did not issue.
const INVALID_TOKEN: &str = "Invalid token value";

/// The `@pattern` of the `UUID` shape every statement `Id` targets, as the model
/// writes it. Source:
/// `smithy_query(query="operation",operation="DescribeStatement")` ->
/// `input.Id.pattern`.
const UUID_PATTERN: &str = r"^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$";

/// The `@pattern` of `ClusterIdentifierString`, as the model writes it. Source:
/// `smithy_query(query="operation",operation="ExecuteStatement")` ->
/// `input.ClusterIdentifier.pattern`.
const CLUSTER_IDENTIFIER_PATTERN: &str = r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$";

/// A `SessionId` in the form the `UUID` pattern accepts. Whether a session with
/// it exists is never reached by the cases that use it: selecting two targets is
/// refused before any session is looked up.
const SESSION_ID: &str = "3dc6a4b5-5e2b-4b5a-9f1d-6b6d2b6c7a8e";

/// The `Message` an error body carries, or `None` when it carries none.
fn message(response: &Value) -> Option<&str> {
    response["Message"].as_str()
}

/// The requirement clause of a constraint refusal: everything the framework's
/// envelope states after `regular expression pattern: `.
///
/// Read off the LOWERCASE `message` key, which is where the constraint envelope
/// states the refusal (a handler's own `ValidationException` carries `Message`).
fn quoted_pattern(response: &Value) -> String {
    response["message"]
        .as_str()
        .unwrap_or_default()
        .rsplit("regular expression pattern: ")
        .next()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: exactly one compute target
// ---------------------------------------------------------------------------

/// A submit request names ONE target. Naming none of the targets the service
/// accepts, and naming more than one of them — two warehouses, or a warehouse
/// beside a session — are refused in the same canonical words.
#[tokio::test]
async fn a_request_that_does_not_select_exactly_one_target_is_refused() {
    let emulator = Emulator::new();

    // Each row is a TARGET SELECTION: the members that name a compute target,
    // and nothing else. The rows differ in how many targets they select, in
    // which ones, and in the form each is named in, which is what the rule
    // generalises over.
    let selections: [(&str, Value); 6] = [
        ("no target at all", json!({})),
        (
            "a cluster and a workgroup",
            json!({"ClusterIdentifier": "my-cluster", "WorkgroupName": "my-workgroup"}),
        ),
        (
            "a cluster and a session",
            json!({"ClusterIdentifier": "my-cluster", "SessionId": SESSION_ID}),
        ),
        (
            "a workgroup and a session",
            json!({"WorkgroupName": "my-workgroup", "SessionId": SESSION_ID}),
        ),
        (
            "both warehouses and a session",
            json!({
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName": "my-workgroup",
                "SessionId": SESSION_ID,
            }),
        ),
        // A workgroup may be named by ARN, which is still the one workgroup:
        // pairing it with a cluster names two targets just as the name form
        // does.
        (
            "a cluster and a workgroup named by ARN",
            json!({
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName":
                    "arn:aws:redshift-serverless:us-east-1:123456789012:workgroup/\
                     11111111-2222-4333-8444-555555555555",
            }),
        ),
    ];

    for (case, selection) in &selections {
        // The same selection is refused the same way by both submit operations,
        // which take the same target members, and by the metadata reader that
        // shares the check.
        for (operation, base) in [
            ("ExecuteStatement", json!({"Database": "dev", "Sql": "SELECT 1"})),
            (
                "BatchExecuteStatement",
                json!({"Database": "dev", "Sqls": ["SELECT 1"]}),
            ),
        ] {
            let mut request = base;
            for (member, value) in selection.as_object().expect("an object of members") {
                request[member] = value.clone();
            }
            let (status, response) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?} on {operation}: expected ValidationException, got {response}"
            );
            assert_eq!(
                message(&response),
                Some(ONE_TARGET),
                "case {case:?} on {operation}: expected the canonical message, got {response}"
            );
        }
    }

    // Nothing above was admitted.
    let page = emulator.ok("ListStatements", &json!({})).await;
    assert!(
        list(&page, "Statements").is_empty(),
        "a refused request must store no statement, got {page}"
    );

    // And the control: exactly one target is not refused at all.
    let accepted = emulator
        .ok(
            "ExecuteStatement",
            &json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "Sql": "SELECT 1"}),
        )
        .await;
    assert!(
        accepted["Id"].is_string(),
        "one target selects one warehouse and is admitted, got {accepted}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: an Id the service holds no query for
// ---------------------------------------------------------------------------

/// Every operation that takes an `Id` refuses one the service holds no query for
/// with the missing-resource error at HTTP 400, in fixed words: the id the caller
/// sent appears nowhere in the message.
#[tokio::test]
async fn an_id_the_service_holds_no_query_for_is_refused() {
    let emulator = Emulator::new();
    // A statement exists, so each refusal is about the id that was asked for and
    // not about an empty store.
    let live = emulator.run("SELECT 1").await;

    // Each row is an id the `UUID` pattern admits — so the request reaches the
    // handler — that names no query. They differ in form: a zero id, an ordinary
    // one, and one carrying the `:<n>` suffix the pattern allows.
    let ids = [
        ("an id that was never issued", ABSENT_ID.to_string()),
        (
            "another id that was never issued",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        (
            "a statement suffix on an id that was never issued",
            "11111111-2222-3333-4444-555555555555:1".to_string(),
        ),
        (
            "a statement suffix on a statement that is not a batch",
            format!("{live}:1"),
        ),
    ];

    for (case, id) in &ids {
        for operation in ["DescribeStatement", "GetStatementResult", "CancelStatement"] {
            let (status, response) = emulator.call(operation, &json!({"Id": id})).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {response}"
            );
            assert_eq!(
                message(&response),
                Some(NO_SUCH_QUERY),
                "case {case:?} on {operation}: expected the fixed wording, got {response}"
            );
            assert!(
                !message(&response).unwrap_or_default().contains(id.as_str()),
                "case {case:?} on {operation}: the message must carry no part of the id, got \
                 {response}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the target is resolved against the service that owns it
// ---------------------------------------------------------------------------

/// A statement runs on an ENDPOINT, so a target that resolves to none is refused
/// and nothing is stored for it — while the very same request against a target the
/// Region really holds is admitted.
#[tokio::test]
async fn a_target_that_resolves_to_no_endpoint_is_refused() {
    let warehouses = Warehouses::new();
    warehouses.create_cluster("present-cluster").await;
    let present_arn = warehouses.create_workgroup("present-workgroup").await;
    // The same ARN form for a workgroup that was never created: the workgroup id
    // is the only part that differs, so what decides the answer is the resource
    // and not the shape of the reference.
    let absent_arn = present_arn.replace(
        present_arn.rsplit('/').next().unwrap_or_default(),
        "00000000-0000-4000-8000-000000000000",
    );

    // Each row is a target: the member that names it, its value, and whether the
    // Region holds it. Both halves of the rule are rows of one table, so an
    // emulator that refused everything (or nothing) fails it.
    let targets: [(&str, &str, String, bool); 7] = [
        (
            "a cluster the Region holds",
            "ClusterIdentifier",
            "present-cluster".to_string(),
            true,
        ),
        (
            "a cluster the Region does not hold",
            "ClusterIdentifier",
            "absent-cluster".to_string(),
            false,
        ),
        (
            "a cluster identifier with a typo",
            "ClusterIdentifier",
            "presnt-cluster".to_string(),
            false,
        ),
        (
            "a workgroup the Region holds",
            "WorkgroupName",
            "present-workgroup".to_string(),
            true,
        ),
        (
            "a workgroup the Region does not hold",
            "WorkgroupName",
            "absent-workgroup".to_string(),
            false,
        ),
        (
            "the ARN of a workgroup the Region holds",
            "WorkgroupName",
            present_arn.clone(),
            true,
        ),
        (
            "the ARN of a workgroup the Region does not hold",
            "WorkgroupName",
            absent_arn,
            false,
        ),
    ];

    for (case, member, value, exists) in &targets {
        for (operation, base) in [
            (
                "ExecuteStatement",
                json!({"Database": "dev", "DbUser": "awsuser", "Sql": "SELECT 1"}),
            ),
            (
                "BatchExecuteStatement",
                json!({"Database": "dev", "DbUser": "awsuser", "Sqls": ["SELECT 1"]}),
            ),
        ] {
            let mut request = base;
            request[*member] = json!(value);
            let (status, response) = warehouses.data(operation, &request).await;
            if *exists {
                assert_eq!(
                    status, 200,
                    "case {case:?} on {operation}: a target the Region holds must be admitted, \
                     got {status} {response}"
                );
                assert!(
                    response["Id"].is_string(),
                    "case {case:?} on {operation}: an admitted statement carries an Id, got \
                     {response}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?} on {operation}: expected ValidationException, got {response}"
            );
            assert_eq!(
                message(&response),
                Some(NO_SUCH_ENDPOINT),
                "case {case:?} on {operation}: expected the fixed wording, got {response}"
            );
        }
    }

    // How a request SELECTS its target is a fault of the REQUEST, judged before
    // any endpoint is resolved: a request that names both warehouses is refused
    // as a selection that names two of them whether or not either one exists, so
    // a caller that wired two targets is never sent after a provisioning or
    // Region problem instead. A request that is wrong about its DATABASE or its
    // authorization method is told so on the same terms.
    for (case, overrides, expected) in [
        (
            "both warehouses, the cluster absent",
            json!({"ClusterIdentifier": "absent-cluster", "WorkgroupName": "present-workgroup"}),
            ONE_TARGET,
        ),
        (
            "both warehouses, both absent",
            json!({"ClusterIdentifier": "absent-cluster", "WorkgroupName": "absent-workgroup"}),
            ONE_TARGET,
        ),
        (
            "both warehouses, the cluster present",
            json!({"ClusterIdentifier": "present-cluster", "WorkgroupName": "present-workgroup"}),
            ONE_TARGET,
        ),
        (
            "an absent cluster and no database",
            json!({"ClusterIdentifier": "absent-cluster", "Database": null}),
            "Database is required",
        ),
        (
            "an absent cluster and two authorization methods",
            json!({
                "ClusterIdentifier": "absent-cluster",
                "SecretArn": "arn:aws:secretsmanager:us-east-1:123456789012:secret:db-AbCdEf",
            }),
            "DbUser and SecretArn cannot both be supplied: DbUser authenticates with temporary \
             credentials and SecretArn with Secrets Manager",
        ),
    ] {
        let mut request = json!({
            "Database": "dev",
            "DbUser": "awsuser",
            "Sql": "SELECT 1",
        });
        for (member, value) in overrides.as_object().expect("an object of overrides") {
            if value.is_null() {
                request.as_object_mut().expect("an object").remove(member);
            } else {
                request[member] = value.clone();
            }
        }
        let (status, response) = warehouses.data("ExecuteStatement", &request).await;
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
            Some(expected),
            "case {case:?}: expected {expected:?}, got {response}"
        );
    }

    // A refused submit stored nothing: the only statements the history holds are
    // the ones submitted against the two targets that exist (one per submit
    // operation, twice over for the workgroup named by name and by ARN).
    let (status, page) = warehouses.data("ListStatements", &json!({})).await;
    assert_eq!(status, 200, "ListStatements: expected 200, got {page}");
    let stored = list(&page, "Statements").len();
    assert_eq!(
        stored, 6,
        "only the statements admitted against a target that exists are stored, got {stored} in \
         {page}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a continuation token the service did not issue
// ---------------------------------------------------------------------------

/// Every paginated operation refuses a `NextToken` it did not issue with the one
/// fixed message the service states for an invalid token.
#[tokio::test]
async fn a_continuation_token_the_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let id = emulator.run("SELECT 1").await;
    emulator.run("CREATE TABLE tokens (a INTEGER)").await;

    // Each row is a value that is not a token this service issued: not hex at
    // all, hex that decodes to no cursor, a cursor of another operation's kind,
    // and a cursor whose position is not a number.
    let tokens = [
        "not-hexadecimal",
        "",
        "6162636465",
        "726f77733a35",
        "73746174656d656e74733a616263",
    ];

    // Each row is a paginated operation and the request that reaches its token
    // check, so the rule is asserted on every reader that takes a token.
    let operations: [(&str, Value); 4] = [
        ("ListStatements", json!({})),
        ("ListSessions", json!({})),
        ("GetStatementResult", json!({"Id": id})),
        (
            "DescribeTable",
            json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Table": "tokens",
            }),
        ),
    ];

    for (operation, base) in &operations {
        for token in tokens {
            let mut request = base.clone();
            request["NextToken"] = json!(token);
            let (status, response) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 400,
                "case {operation} {token:?}: expected 400, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {operation} {token:?}: expected ValidationException, got {response}"
            );
            assert_eq!(
                message(&response),
                Some(INVALID_TOKEN),
                "case {operation} {token:?}: expected the fixed wording, got {response}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 5: a @pattern requirement is quoted without its anchors
// ---------------------------------------------------------------------------

/// The pattern a constraint refusal quotes is the member's `@pattern` with the
/// anchors that wrap the whole value dropped — the form the service advertises —
/// for every member of this service that carries one.
#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_its_anchors() {
    let emulator = Emulator::new();

    // Each row is a request whose value breaks one member's `@pattern`, and the
    // pattern the model declares for that member. The rows span two different
    // members and four operations, so what is asserted is the RENDERING and not
    // one member's text.
    let cases: [(&str, &str, Value, &str); 5] = [
        (
            "a statement id that is not a UUID",
            "DescribeStatement",
            json!({"Id": "not-a-uuid"}),
            UUID_PATTERN,
        ),
        (
            "a statement id in upper case",
            "CancelStatement",
            json!({"Id": "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE"}),
            UUID_PATTERN,
        ),
        (
            "a statement id that is far too short",
            "GetStatementResult",
            json!({"Id": "x"}),
            UUID_PATTERN,
        ),
        (
            "a cluster identifier with an underscore",
            "ExecuteStatement",
            json!({"ClusterIdentifier": "bad_cluster", "Database": "dev", "Sql": "SELECT 1"}),
            CLUSTER_IDENTIFIER_PATTERN,
        ),
        (
            "a cluster identifier filter in upper case",
            "ListStatements",
            json!({"ClusterIdentifier": "BAD-CLUSTER"}),
            CLUSTER_IDENTIFIER_PATTERN,
        ),
    ];

    for (case, operation, request, pattern) in &cases {
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
        let unanchored = pattern
            .strip_prefix('^')
            .and_then(|pattern| pattern.strip_suffix('$'))
            .unwrap_or(pattern);
        let quoted = quoted_pattern(&response);
        assert_eq!(
            quoted, unanchored,
            "case {case:?}: expected the pattern quoted as {unanchored:?}, got {quoted:?} in \
             {response}"
        );
    }
}

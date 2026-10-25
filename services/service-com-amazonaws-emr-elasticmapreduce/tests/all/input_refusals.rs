// SPDX-License-Identifier: CC0-1.0

//! EMR refuses a value outside a modelled ENUM with the error identity it models.
//!
//! The rule under test: Amazon EMR validates an enum-valued member itself, ahead of
//! the generic request validator, so a value outside that enum's value set is
//! answered HTTP 400 with the error code `InvalidRequestException` ("This exception
//! occurs when there is something wrong with user input"), the client-input error
//! EMR's own model declares. A client catching
//! `emr.exceptions.InvalidRequestException` must catch it, which it cannot when the
//! answer carries the framework's synthetic `ValidationException` — an identity no
//! EMR operation declares. Every OTHER modelled constraint (a `@length`, a
//! `@range`, a required member absent) stays with the generic validator's
//! `ValidationException`, which is what EMR is recorded answering for one.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, so these are the same bytes an SDK client sends.
//!
//! The rows span what the rule generalises over: which enum is violated and on
//! which operation, where the offending value sits (a scalar member, and the 1st,
//! 2nd or 3rd element of a list of enums), how many values the member carries, and
//! the case-sensitivity of an enum value — a refusal of constrained input happens
//! BEFORE the handler either way. The other constraint classes are rows too, at
//! each published bound of a range, one past it, and one character past a maximum
//! length, so the discrimination between the two identities is asserted and not
//! assumed. The admitted rows are the last part of the rule: a value the model does
//! admit is never refused at the boundary.

use serde_json::{json, Value};

// The wire harness this crate's suites share (`job_flow_lifecycle::Emr`): one router
// over one engine, driving the real `POST /` path. Only the error-envelope accessors
// it does not carry are added below.
use crate::job_flow_lifecycle::Emr;

/// The error code EMR's model declares for bad user input
/// (`smithy_query(query="operation", operation="ListClusters")` lists
/// `InvalidRequestException`, HTTP 400, as its only client error).
const MODELED_CLIENT_ERROR: &str = "InvalidRequestException";

/// The generic request validator's code, which EMR keeps for every constraint
/// class it does not check itself.
const GENERIC_CLIENT_ERROR: &str = "ValidationException";

/// Every value `com.amazonaws.emr#ClusterState` declares
/// (`smithy_query(query="shape", shape="ClusterState")`).
const CLUSTER_STATES: [&str; 7] = [
    "STARTING",
    "BOOTSTRAPPING",
    "RUNNING",
    "WAITING",
    "TERMINATING",
    "TERMINATED",
    "TERMINATED_WITH_ERRORS",
];

/// Every value `com.amazonaws.emr#SessionState` declares
/// (`smithy_query(query="shape", shape="SessionState")`).
const SESSION_STATES: [&str; 8] = [
    "SUBMITTED",
    "STARTING",
    "STARTED",
    "IDLE",
    "BUSY",
    "TERMINATING",
    "TERMINATED",
    "FAILED",
];

/// `ListSessionsInput$MaxResults` targets `MaxResultsNumber`, whose `@range` is
/// 1..100 (`smithy_query(query="operation", operation="ListSessions")`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// `ClusterId` and `AddTagsInput$ClusterId` are `@length(0..256)`
/// (`smithy_query(query="operation", operation="AddTags")`).
const CLUSTER_ID_MAX_LENGTH: usize = 256;

/// The awsJson1_1 error code, without the namespace the envelope may prefix.
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
///
/// `pub(crate)` so every suite in this test target reads a refusal's message through one
/// helper rather than its own copy of the casing fallback.
pub(crate) fn error_message(payload: &Value) -> String {
    payload
        .get("Message")
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// What the boundary must do with one row's request.
enum Expectation {
    /// Refused: the error code the refusal must carry, and the member path its message
    /// must name.
    Refused {
        code: &'static str,
        path: String,
    },
    /// Admitted: the request reached the handler, which answered this status — 200 from
    /// an implemented operation, 501 from one still scaffolded. Naming it per row is
    /// what keeps a re-scaffolded operation, or an implemented one still answering 501,
    /// from passing here silently.
    Admitted(u16),
}

/// One row: a request, and what the boundary must do with it.
struct Case {
    case: &'static str,
    operation: &'static str,
    body: Value,
    expected: Expectation,
}

/// A request EMR refuses as an enum-value violation, with its own modelled
/// client-input error, whose message must name `path`.
fn refused_enum(case: &'static str, operation: &'static str, path: &str, body: Value) -> Case {
    Case {
        case,
        operation,
        body,
        expected: Expectation::Refused {
            code: MODELED_CLIENT_ERROR,
            path: path.to_string(),
        },
    }
}

/// A request the generic validator refuses, whose message must name `path`.
fn refused_generic(case: &'static str, operation: &'static str, path: &str, body: Value) -> Case {
    Case {
        case,
        operation,
        body,
        expected: Expectation::Refused {
            code: GENERIC_CLIENT_ERROR,
            path: path.to_string(),
        },
    }
}

/// A request the model admits, which an implemented operation answers 200.
fn admitted(case: &'static str, operation: &'static str, body: Value) -> Case {
    Case {
        case,
        operation,
        body,
        expected: Expectation::Admitted(200),
    }
}

/// A request the model admits, of an operation that is still a scaffold stub: it
/// reaches the handler and is answered 501 there, which is the OTHER half of "a
/// refusal of constrained input happens before the handler".
fn admitted_by_a_stub(case: &'static str, operation: &'static str, body: Value) -> Case {
    Case {
        case,
        operation,
        body,
        expected: Expectation::Admitted(501),
    }
}

fn cases() -> Vec<Case> {
    let over_long_cluster_id = "j".repeat(CLUSTER_ID_MAX_LENGTH + 1);
    let longest_cluster_id = "j".repeat(CLUSTER_ID_MAX_LENGTH);
    vec![
        // An enum value set, violated by the only element of a list of enums.
        refused_enum(
            "one ClusterStates entry outside ClusterState",
            "ListClusters",
            "clusterStates",
            json!({ "ClusterStates": ["NOPE"] }),
        ),
        // The same violation behind a valid element: the position of the offending
        // value does not change the refusal.
        refused_enum(
            "a valid ClusterStates entry followed by an invalid one",
            "ListClusters",
            "clusterStates",
            json!({ "ClusterStates": ["RUNNING", "NOPE"] }),
        ),
        // Enum values are case-sensitive, so the lowercase spelling of a declared
        // value is outside the set.
        refused_enum(
            "a ClusterStates entry that is a declared value in lower case",
            "ListClusters",
            "clusterStates",
            json!({ "ClusterStates": ["running"] }),
        ),
        refused_enum(
            "an invalid ClusterStates entry among three",
            "ListClusters",
            "clusterStates",
            json!({ "ClusterStates": ["TERMINATED_WITH_ERRORS", "bogus", "STARTING"] }),
        ),
        // The same constraint class on a SCALAR member rather than on a list
        // element: the member's own enum is checked the same way.
        refused_enum(
            "a Status outside NotebookExecutionStatus",
            "ListNotebookExecutions",
            "status",
            json!({ "Status": "NOT_A_REAL_STATUS" }),
        ),
        // A third enum, on a third operation, reached through a different list.
        refused_enum(
            "a SessionStates entry outside SessionState",
            "ListSessions",
            "sessionStates",
            json!({ "ClusterId": "j-ABCDEFGHIJKLM", "SessionStates": ["NOPE"] }),
        ),
        refused_enum(
            "a StepStates entry outside StepState",
            "ListSteps",
            "stepStates",
            json!({ "ClusterId": "j-ABCDEFGHIJKLM", "StepStates": ["NOT_A_STATE"] }),
        ),
        // An enum violation together with a violation of another class is still the
        // enum refusal: that is the check EMR runs first.
        refused_enum(
            "an invalid SessionStates entry beside a MaxResults out of range",
            "ListSessions",
            "sessionStates",
            json!({
                "ClusterId": "j-ABCDEFGHIJKLM",
                "SessionStates": ["NOPE"],
                "MaxResults": MAX_RESULTS_RANGE.1 + 1,
            }),
        ),
        // A @range, one past each published bound: another constraint class, so the
        // generic validator's identity stands.
        refused_generic(
            "MaxResults one below the range minimum",
            "ListSessions",
            "maxResults",
            json!({
                "ClusterId": "j-ABCDEFGHIJKLM",
                "MaxResults": MAX_RESULTS_RANGE.0 - 1,
            }),
        ),
        refused_generic(
            "MaxResults one above the range maximum",
            "ListSessions",
            "maxResults",
            json!({
                "ClusterId": "j-ABCDEFGHIJKLM",
                "MaxResults": MAX_RESULTS_RANGE.1 + 1,
            }),
        ),
        // A @length, one past the published maximum.
        refused_generic(
            "a ClusterId one character past its maximum length",
            "AddTags",
            "clusterId",
            json!({
                "ResourceId": "j-ABCDEFGHIJKLM",
                "Tags": [{ "Key": "env", "Value": "test" }],
                "ClusterId": over_long_cluster_id,
            }),
        ),
        // A required member absent is the generic validator's too.
        refused_generic(
            "DescribeCluster with no ClusterId at all",
            "DescribeCluster",
            "clusterId",
            json!({}),
        ),
        refused_generic(
            "AddTags with no Tags at all",
            "AddTags",
            "tags",
            json!({ "ResourceId": "j-ABCDEFGHIJKLM" }),
        ),
        // The other half of the rule: what the model admits is not refused.
        admitted(
            "a single declared ClusterState",
            "ListClusters",
            json!({ "ClusterStates": ["RUNNING"] }),
        ),
        admitted(
            "every declared ClusterState at once",
            "ListClusters",
            json!({ "ClusterStates": CLUSTER_STATES }),
        ),
        admitted("no filter at all", "ListClusters", json!({})),
        admitted_by_a_stub(
            "every declared SessionState at once",
            "ListSessions",
            json!({ "ClusterId": "j-ABCDEFGHIJKLM", "SessionStates": SESSION_STATES }),
        ),
        admitted_by_a_stub(
            "MaxResults at the range minimum",
            "ListSessions",
            json!({
                "ClusterId": "j-ABCDEFGHIJKLM",
                "MaxResults": MAX_RESULTS_RANGE.0,
            }),
        ),
        admitted_by_a_stub(
            "MaxResults at the range maximum",
            "ListSessions",
            json!({
                "ClusterId": "j-ABCDEFGHIJKLM",
                "MaxResults": MAX_RESULTS_RANGE.1,
            }),
        ),
        admitted_by_a_stub(
            "a ClusterId exactly at its maximum length",
            "AddTags",
            json!({
                "ResourceId": "j-ABCDEFGHIJKLM",
                "Tags": [{ "Key": "env", "Value": "test" }],
                "ClusterId": longest_cluster_id,
            }),
        ),
        admitted(
            "a declared NotebookExecutionStatus",
            "ListNotebookExecutions",
            json!({ "Status": "FINISHED" }),
        ),
    ]
}

#[tokio::test]
async fn constrained_input_is_refused_with_the_error_identity_emr_models() {
    let emr = Emr::new();
    for case in cases() {
        let label = case.case;
        let (status, payload) = emr.call(case.operation, case.body.clone()).await;
        let code = error_code(&payload);
        match &case.expected {
            Expectation::Refused {
                code: expected_code,
                path,
            } => {
                assert_eq!(
                    status, 400,
                    "case {label:?}: expected HTTP 400, got {status} with {payload}"
                );
                assert_eq!(
                    &code, expected_code,
                    "case {label:?}: expected error code {expected_code}, got {code:?} in \
                     {payload}"
                );
                let message = error_message(&payload);
                assert!(
                    message.contains(path.as_str()),
                    "case {label:?}: the refusal must name the member {path:?}, got \
                     {message:?}"
                );
            }
            Expectation::Admitted(expected_status) => {
                assert_eq!(
                    status, *expected_status,
                    "case {label:?}: a value the model admits must reach the handler and \
                     be answered {expected_status}, got {status} with {payload}"
                );
            }
        }
    }
}

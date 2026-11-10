// SPDX-License-Identifier: CC0-1.0

//! EMR refuses a value outside a modelled CONSTRAINT with the error identity it models.
//!
//! The rule under test: every modelled constraint class other than an enum — a
//! `@length`, a `@range` — is refused by the generic request validator with its
//! `ValidationException` and its member-path envelope, which is what EMR is recorded
//! answering for one, and a value the model DOES admit is not refused at the boundary at
//! all but reaches the handler.
//!
//! The enum class is the other half of that rule, and it is not here: which layer
//! refuses an out-of-enum value, the code it answers and the sentence it states are
//! asserted by `tests/all/enum_refusal_identity.rs`, which owns those rows so the two
//! suites do not hold two copies of one table. An input member the request omits
//! altogether is EMR's own refusal where EMR publishes a code for that member's class,
//! which `tests/all/cluster_id_refusal_code.rs` owns; the row below keeps the other
//! half, a member it publishes none for.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, so these are the same bytes an SDK client sends.
//!
//! The rows span what the rule generalises over: which constraint is violated and on
//! which operation, at each published bound of a range, one past it, and one character
//! past a maximum length. The admitted rows are the last part of the rule: a value the
//! model does admit is never refused at the boundary.

use serde_json::{json, Value};

// The wire harness this crate's suites share (`job_flow_lifecycle::Emr`): one router
// over one engine, driving the real `POST /` path. Only the error-envelope accessors
// it does not carry are added below.
use crate::job_flow_lifecycle::Emr;

/// The generic request validator's code, which EMR keeps for every constraint
/// class it does not check itself.
const GENERIC_CLIENT_ERROR: &str = "ValidationException";

/// The client error Amazon EMR funnels its OWN refusals through, which is what a handler
/// a request reached answers when it rejects that request on its own grounds.
const MODELLED_CLIENT_ERROR: &str = "InvalidRequestException";

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
        says: String,
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

/// A request the generic validator refuses, whose message must name `path`.
fn refused_generic(case: &'static str, operation: &'static str, path: &str, body: Value) -> Case {
    Case {
        case,
        operation,
        body,
        expected: Expectation::Refused {
            code: GENERIC_CLIENT_ERROR,
            says: path.to_string(),
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

/// A request the model admits, which the HANDLER then refuses on its own grounds: the
/// error Amazon EMR models rather than the boundary's member-path envelope, which is
/// what shows the request got past the boundary at all.
fn admitted_then_refused(
    case: &'static str,
    operation: &'static str,
    says: &str,
    body: Value,
) -> Case {
    Case {
        case,
        operation,
        body,
        expected: Expectation::Refused {
            code: MODELLED_CLIENT_ERROR,
            says: says.to_string(),
        },
    }
}

fn cases() -> Vec<Case> {
    let over_long_cluster_id = "j".repeat(CLUSTER_ID_MAX_LENGTH + 1);
    let longest_cluster_id = "j".repeat(CLUSTER_ID_MAX_LENGTH);
    vec![
        // A @range, one past each published bound.
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
        // A required member absent, on a member whose refusal class Amazon EMR
        // publishes no code for, is the generic validator's too.
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
        // `ListSessions` is implemented, so a request the boundary admits reaches the
        // handler and is refused there for naming no cluster — the cluster-id sentence,
        // not the validator's member-path envelope.
        admitted_then_refused(
            "every declared SessionState at once",
            "ListSessions",
            "is not valid.",
            json!({ "ClusterId": "j-ABCDEFGHIJKLM", "SessionStates": SESSION_STATES }),
        ),
        admitted_then_refused(
            "MaxResults at the range minimum",
            "ListSessions",
            "is not valid.",
            json!({
                "ClusterId": "j-ABCDEFGHIJKLM",
                "MaxResults": MAX_RESULTS_RANGE.0,
            }),
        ),
        admitted_then_refused(
            "MaxResults at the range maximum",
            "ListSessions",
            "is not valid.",
            json!({
                "ClusterId": "j-ABCDEFGHIJKLM",
                "MaxResults": MAX_RESULTS_RANGE.1,
            }),
        ),
        // `AddTags` is implemented, so a `ClusterId` the boundary admits reaches the
        // handler and is refused there for naming no cluster — the cluster-id sentence,
        // not the validator's member-path envelope.
        admitted_then_refused(
            "a ClusterId exactly at its maximum length",
            "AddTags",
            "is not valid.",
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
                says,
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
                    message.contains(says.as_str()),
                    "case {label:?}: the refusal must say {says:?}, got {message:?}"
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

// SPDX-License-Identifier: CC0-1.0

//! What this service answers a numeric member whose value lies OUTSIDE the
//! `@range` the model declares for it, driven over the wire through the shared
//! harness in [`super::emulator`]: every call below is a real `POST /` with
//! `X-Amz-Target: RedshiftData.<Operation>`, so what is asserted is the
//! emulator's HTTP behaviour and not a handler's return value.
//!
//! The RULE: a numeric member whose value falls outside its modelled `@range` is
//! refused at the boundary — before any handler runs — with HTTP 400,
//! `ValidationException` and the standard constraint envelope, which echoes the
//! value submitted, names the member by its lower-camel path and states the one
//! bound the value broke; a value the range admits is not refused for its
//! magnitude at all.
//!
//! It is asserted over a TABLE spanning every `@range` member this service
//! carries rather than one recorded request, because the rule is about the
//! CLASS: the rows differ along the operation, the member, the bounds the model
//! declares for it and which bound the value breaks, and each member is driven
//! on both of its published bounds and on values one step and far past each of
//! them.
//!
//! The wording of the pattern half of that envelope, and the target-selection and
//! missing-endpoint refusals of a submit, are [`super::error_surface`]'s rules.

use serde_json::{json, Value};

use super::emulator::{error_code, id_of, Emulator, QUICK_SQL};

/// The `ClusterIdentifier`, `Database` and `DbUser` every row that needs a
/// warehouse connects through. No Redshift control plane is mounted beside the
/// Data API here, so the target is UNRESOLVED rather than absent and a request
/// naming it is admitted — which is what lets this suite assert the bound and
/// nothing else.
fn connection() -> Value {
    json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"})
}

/// One member of this service carrying a `@range`, and the request that reaches
/// its bound.
struct Bounded {
    /// The operation the member belongs to.
    operation: &'static str,
    /// The member as the wire spells it.
    member: &'static str,
    /// The member's path in a constraint clause: its name, lower-camel.
    path: &'static str,
    /// The inclusive bounds the model declares.
    min: i64,
    max: i64,
    /// Whether a value the range ADMITS reaches a handler that serves the
    /// request. `false` for an operation still answering `501`, whose boundary
    /// check runs all the same.
    served: bool,
}

/// Every `@range` member of this service. Source:
/// `smithy_query(query="operation",operation="<Operation>")` -> the member's
/// `range` under `input`. Each member is backed by a shape that carries the
/// bound: member `WaitTimeSeconds` by shape `WaitTimeSeconds` (1..30), member
/// `SessionKeepAliveSeconds` by shape `SessionAliveSeconds` (0..86400), member
/// `MaxResults` by shape `ListStatementsLimit` (0..100) on the statement and
/// session lists and by shape `PageSize` (0..1000) on the metadata readers.
/// Every one of them is modelled `integer`, which is why the far-past-the-bound
/// values below are `i32`'s extremes.
///
/// `GetStatementResultV2` reads the CSV form of a result, so the statement it is
/// driven with is one submitted in that format; `ListSchemas` and `ListTables`
/// are the two operations of this service that still answer `501`, and their
/// bound is checked before their handler is reached, so they carry
/// `served: false` rather than being left out.
const BOUNDED: [Bounded; 13] = [
    Bounded {
        operation: "ExecuteStatement",
        member: "WaitTimeSeconds",
        path: "waitTimeSeconds",
        min: 1,
        max: 30,
        served: true,
    },
    Bounded {
        operation: "ExecuteStatement",
        member: "SessionKeepAliveSeconds",
        path: "sessionKeepAliveSeconds",
        min: 0,
        max: 86400,
        served: true,
    },
    Bounded {
        operation: "BatchExecuteStatement",
        member: "WaitTimeSeconds",
        path: "waitTimeSeconds",
        min: 1,
        max: 30,
        served: true,
    },
    Bounded {
        operation: "BatchExecuteStatement",
        member: "SessionKeepAliveSeconds",
        path: "sessionKeepAliveSeconds",
        min: 0,
        max: 86400,
        served: true,
    },
    Bounded {
        operation: "DescribeStatement",
        member: "WaitTimeSeconds",
        path: "waitTimeSeconds",
        min: 1,
        max: 30,
        served: true,
    },
    Bounded {
        operation: "GetStatementResult",
        member: "WaitTimeSeconds",
        path: "waitTimeSeconds",
        min: 1,
        max: 30,
        served: true,
    },
    Bounded {
        operation: "GetStatementResultV2",
        member: "WaitTimeSeconds",
        path: "waitTimeSeconds",
        min: 1,
        max: 30,
        served: true,
    },
    Bounded {
        operation: "ListStatements",
        member: "MaxResults",
        path: "maxResults",
        min: 0,
        max: 100,
        served: true,
    },
    Bounded {
        operation: "ListSessions",
        member: "MaxResults",
        path: "maxResults",
        min: 0,
        max: 100,
        served: true,
    },
    Bounded {
        operation: "ListDatabases",
        member: "MaxResults",
        path: "maxResults",
        min: 0,
        max: 1000,
        served: true,
    },
    Bounded {
        operation: "DescribeTable",
        member: "MaxResults",
        path: "maxResults",
        min: 0,
        max: 1000,
        served: true,
    },
    Bounded {
        operation: "ListSchemas",
        member: "MaxResults",
        path: "maxResults",
        min: 0,
        max: 1000,
        served: false,
    },
    Bounded {
        operation: "ListTables",
        member: "MaxResults",
        path: "maxResults",
        min: 0,
        max: 1000,
        served: false,
    },
];

/// The request `operation` is driven with, valid in every respect but the member
/// under test, so the only thing a refusal can be about is that member's bound.
/// `id` names a statement the service holds and has finished running, and
/// `csv_id` one of those whose result is read in the CSV form.
fn request_for(operation: &str, id: &str, csv_id: &str) -> Value {
    match operation {
        "ExecuteStatement" => {
            let mut request = connection();
            request["Sql"] = json!(QUICK_SQL);
            request
        }
        "BatchExecuteStatement" => {
            let mut request = connection();
            request["Sqls"] = json!([QUICK_SQL]);
            request
        }
        "DescribeStatement" | "GetStatementResult" => json!({"Id": id}),
        "GetStatementResultV2" => json!({"Id": csv_id}),
        "ListStatements" | "ListSessions" => json!({}),
        "ListDatabases" => connection(),
        // `Database` is required on the three metadata readers.
        "DescribeTable" | "ListSchemas" | "ListTables" => connection(),
        other => unreachable!("no request is built for {other}"),
    }
}

/// The constraint envelope's own message. The boundary refusal states it under
/// the LOWERCASE `message` key (a handler's own `ValidationException` carries
/// `Message`).
fn constraint_message(response: &Value) -> String {
    response["message"].as_str().unwrap_or_default().to_string()
}

/// A finished statement whose result is read in the CSV form, so the reader of
/// that form can be pointed at a statement it serves.
async fn csv_statement(emulator: &Emulator) -> String {
    let mut request = connection();
    request["Sql"] = json!(QUICK_SQL);
    request["ResultFormat"] = json!("CSV");
    id_of(&emulator.execute_and_wait(&request).await)
}

/// RULE: a value outside a member's modelled `@range` is refused with the
/// constraint envelope naming the member and the bound it broke.
#[tokio::test]
async fn a_value_outside_a_modelled_range_is_refused_naming_the_bound_it_broke() {
    let emulator = Emulator::new();
    // A statement the readers can be pointed at, so a refusal is about the
    // bound and never about an id the service holds no query for.
    let id = emulator.run(QUICK_SQL).await;
    let csv_id = csv_statement(&emulator).await;

    for bounded in &BOUNDED {
        // One step past each published bound, and far past each of them: the
        // bound that is broken decides the clause, and how far past it the value
        // lies decides nothing.
        let outside: [(&str, i64, String); 4] = [
            (
                "one below the minimum",
                bounded.min - 1,
                format!(
                    "Member must have value greater than or equal to {}",
                    bounded.min
                ),
            ),
            (
                "far below the minimum",
                i64::from(i32::MIN),
                format!(
                    "Member must have value greater than or equal to {}",
                    bounded.min
                ),
            ),
            (
                "one above the maximum",
                bounded.max + 1,
                format!(
                    "Member must have value less than or equal to {}",
                    bounded.max
                ),
            ),
            (
                "far above the maximum",
                i64::from(i32::MAX),
                format!(
                    "Member must have value less than or equal to {}",
                    bounded.max
                ),
            ),
        ];

        for (case, value, requirement) in &outside {
            let case = format!("{} {} {case}", bounded.operation, bounded.member);
            let mut request = request_for(bounded.operation, &id, &csv_id);
            request[bounded.member] = json!(value);
            let (status, response) = emulator.call(bounded.operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response}"
            );
            let expected = format!(
                "1 validation error detected: Value '{value}' at '{}' failed to satisfy \
                 constraint: {requirement}",
                bounded.path
            );
            let stated = constraint_message(&response);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?} in {response}"
            );
        }
    }
}

/// RULE, the other side of the same bound: each published bound is INSIDE the
/// range, so a request carrying one is never refused for the member's
/// magnitude — and the operations that have a handler serve it.
#[tokio::test]
async fn each_published_bound_is_admitted() {
    let emulator = Emulator::new();
    let id = emulator.run(QUICK_SQL).await;
    let csv_id = csv_statement(&emulator).await;

    for bounded in &BOUNDED {
        for (edge, value) in [("the minimum", bounded.min), ("the maximum", bounded.max)] {
            let case = format!("{} {} at {edge}", bounded.operation, bounded.member);
            let mut request = request_for(bounded.operation, &id, &csv_id);
            request[bounded.member] = json!(value);
            let (status, response) = emulator.call(bounded.operation, &request).await;
            let stated = constraint_message(&response);
            assert!(
                !stated.contains("failed to satisfy constraint"),
                "case {case:?}: a value the range admits must not be refused as a constraint \
                 violation, got {status} {response}"
            );
            if bounded.served {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the request to be served, got {status} {response}"
                );
            } else {
                // An operation with no handler yet cannot be asserted to serve
                // the request, but it can be asserted not to REFUSE it: the
                // bound was checked before the handler was reached, so a value
                // the range admits never produces a client refusal here.
                assert_ne!(
                    status, 400,
                    "case {case:?}: a value the range admits must not be refused, got {status} \
                     {response}"
                );
            }
        }
    }
}

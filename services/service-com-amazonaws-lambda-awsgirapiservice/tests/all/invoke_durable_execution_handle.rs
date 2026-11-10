// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the durable-execution HANDLE a synchronous `Invoke` answers.
//!
//! The rule under test: an invocation of a durable function — one whose resolved
//! configuration carries a `DurableConfig` — or one that names a `DurableExecutionName`
//! starts a durable execution and answers its `DurableExecutionArn` ("The ARN of the
//! durable execution that was started. This is returned when invoking a durable function"),
//! whatever the invocation type is; the execution that ARN names is readable through
//! `GetDurableExecution` and ends in the status the invocation came to. An invocation that
//! starts none answers no ARN.
//!
//! Every row drives real HTTP requests through the router and reads the execution back
//! through its own operation, so an ARN that names nothing fails here. The rows differ
//! along what the rule generalises over: the invocation type, whether the function is
//! durable, whether the request names the execution, and what the run came to.

use std::io::Write as _;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, router_on, send, zip_file, Answer, ACCOUNT,
    REGION, ROLE,
};

/// Where `Invoke` is bound (`POST /2015-03-31/functions/{FunctionName}/invocations`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The response header `Invoke$DurableExecutionArn` is bound to.
const ARN_HEADER: &str = "x-amz-durable-execution-arn";

/// The request header `Invoke$DurableExecutionName` is bound to.
const NAME_HEADER: &str = "x-amz-durable-execution-name";

/// The infix the model's `DurableExecutionArn` pattern puts between the function version
/// and the execution's name and id.
const ARN_INFIX: &str = "/durable-execution/";

/// A customer managed key ARN satisfying the model's `KMSKeyArn` pattern, which the durable
/// function here names in its `DurableConfig`.
const KMS_KEY: &str = "arn:aws:kms:us-east-1:000000000000:key/durable-executions";

/// The `ExecutionStatus` values this suite asserts, from `smithy_query(query="shape",
/// shape="ExecutionStatus")` (`RUNNING | SUCCEEDED | FAILED | TIMED_OUT | STOPPED`).
const STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// See [`STATUS_SUCCEEDED`].
const STATUS_FAILED: &str = "FAILED";

/// A .zip deployment package holding `entries`.
fn package(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (name, source) in entries {
            writer.start_file(*name, options).expect("the entry starts");
            writer
                .write_all(source.as_bytes())
                .expect("the entry's bytes");
        }
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a runnable `nodejs20.x` function holding `source` as `index.js`, durable when
/// `durable` says so, and bring it to the state an invocation needs.
async fn create_runnable_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    source: &str,
    durable: bool,
) {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "nodejs20.x",
        "Handler": "index.handler",
        "Timeout": 10,
        "MemorySize": 256,
        "Code": { "ZipFile": zip_file(&package(&[("index.js", source)])) },
    });
    if durable {
        body["DurableConfig"] = json!({ "KMSKeyArn": KMS_KEY, "ExecutionTimeout": 3_600 });
    }
    let created = create_function(router, &body).await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// `Invoke` on `name`, with the invocation type `invocation_type` and the durable execution
/// name `execution` when it names one.
async fn invoke(
    router: &MultiServiceRouter,
    name: &str,
    invocation_type: &str,
    execution: Option<&str>,
    payload: &[u8],
) -> Answer {
    let mut headers = vec![("x-amz-invocation-type", invocation_type.to_string())];
    if let Some(execution) = execution {
        headers.push((NAME_HEADER, execution.to_string()));
    }
    send(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        "application/octet-stream",
        payload.to_vec(),
        &headers,
    )
    .await
}

/// `GetDurableExecution` on `arn`. The label is not greedy, so the ARN's slashes are
/// percent-encoded in the path exactly as an SDK encodes them.
async fn get_execution(router: &MultiServiceRouter, arn: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/2025-12-01/durable-executions/{}",
            arn.replace('/', "%2F")
        ),
        &Value::Null,
    )
    .await
}

/// A handler that answers with what it was given — a durable execution's own result.
const RETURNING_HANDLER: &str = "exports.handler = async (event) => ({ echoed: event });";

/// A handler that fails, so the execution its invocation started ends FAILED.
const FAILING_HANDLER: &str = "exports.handler = async () => { throw new TypeError('no good'); };";

/// One row of the handle table.
struct HandleCase {
    case: &'static str,
    /// Whether the function carries a `DurableConfig`.
    durable: bool,
    /// The handler's source.
    source: &'static str,
    /// The invocation type the request names.
    invocation_type: &'static str,
    /// The `DurableExecutionName` the request names, if any.
    requested_name: Option<&'static str>,
    /// The HTTP status the invocation answers.
    status: u16,
    /// The `ExecutionStatus` the execution the ARN names must carry, or `None` where the
    /// invocation starts no execution at all.
    execution_status: Option<&'static str>,
}

/// The rows: each invocation type, a durable function and a plain one, a named execution
/// and a generated name, and a run that returned beside one that failed.
fn handle_cases() -> Vec<HandleCase> {
    vec![
        HandleCase {
            case: "a synchronous invocation of a durable function, naming no execution",
            durable: true,
            source: RETURNING_HANDLER,
            invocation_type: "RequestResponse",
            requested_name: None,
            status: 200,
            execution_status: Some(STATUS_SUCCEEDED),
        },
        HandleCase {
            case: "a synchronous invocation naming the execution",
            durable: true,
            source: RETURNING_HANDLER,
            invocation_type: "RequestResponse",
            requested_name: Some("named-synchronously"),
            status: 200,
            execution_status: Some(STATUS_SUCCEEDED),
        },
        HandleCase {
            case: "a synchronous invocation whose handler fails",
            durable: true,
            source: FAILING_HANDLER,
            invocation_type: "RequestResponse",
            requested_name: Some("failing-run"),
            status: 200,
            execution_status: Some(STATUS_FAILED),
        },
        HandleCase {
            case: "a synchronous invocation of a plain function naming an execution",
            durable: false,
            source: RETURNING_HANDLER,
            invocation_type: "RequestResponse",
            requested_name: Some("named-on-a-plain-function"),
            status: 200,
            execution_status: Some(STATUS_SUCCEEDED),
        },
        HandleCase {
            case: "an asynchronous invocation of a durable function",
            durable: true,
            source: RETURNING_HANDLER,
            invocation_type: "Event",
            requested_name: Some("named-asynchronously"),
            status: 202,
            execution_status: None,
        },
        HandleCase {
            case: "a synchronous invocation of a plain function naming no execution",
            durable: false,
            source: RETURNING_HANDLER,
            invocation_type: "RequestResponse",
            requested_name: None,
            status: 200,
            execution_status: None,
        },
        HandleCase {
            case: "a dry run, which validates the invocation and runs nothing",
            durable: true,
            source: RETURNING_HANDLER,
            invocation_type: "DryRun",
            requested_name: Some("dry-run-execution"),
            status: 204,
            execution_status: None,
        },
    ]
}

#[tokio::test]
async fn an_invocation_of_a_durable_function_answers_the_execution_it_started() {
    for case in handle_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "durable-handle";
        create_runnable_function(&router, &engine, function, case.source, case.durable).await;

        let answer = invoke(
            &router,
            function,
            case.invocation_type,
            case.requested_name,
            br#"{"order":1}"#,
        )
        .await;
        assert_eq!(
            answer.status,
            case.status,
            "case {:?}: expected HTTP {}, got {} {}",
            case.case,
            case.status,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let arn = answer.header(ARN_HEADER);
        // Which invocations start an execution: a durable function's, and one that names
        // an execution. A dry run "validate[s] parameter values" and runs nothing, so it
        // starts none.
        let starts = (case.durable || case.requested_name.is_some())
            && case.invocation_type != "DryRun";
        if !starts {
            assert!(
                arn.is_none(),
                "case {:?}: an invocation that starts no execution answers no ARN, got {arn:?}",
                case.case
            );
            continue;
        }
        let arn = arn.unwrap_or_default();
        let expected_prefix = format!(
            "arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}:$LATEST{ARN_INFIX}"
        );
        assert!(
            arn.starts_with(&expected_prefix),
            "case {:?}: the ARN is the one the model's DurableExecutionArn pattern admits, \
             got {arn:?}",
            case.case
        );
        if let Some(requested) = case.requested_name {
            assert!(
                arn.starts_with(&format!("{expected_prefix}{requested}/")),
                "case {:?}: the ARN carries the name the request gave the execution, got \
                 {arn:?}",
                case.case
            );
        }

        // The handle REALLY addresses an execution: it is read back through its own
        // operation, and a synchronous invocation's execution is already in the terminal
        // status its run came to.
        let read = get_execution(&router, &arn).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: the ARN the invocation answered must address an execution, got {} {}",
            case.case,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        if let Some(expected) = case.execution_status {
            assert_eq!(
                read.string("Status").as_deref(),
                Some(expected),
                "case {:?}: the execution ends in the status the invocation came to, got {}",
                case.case,
                String::from_utf8_lossy(&read.body)
            );
        }
        if case.execution_status == Some(STATUS_SUCCEEDED) {
            // "The JSON result returned by the durable execution if it completed
            // successfully": the run's own answer, which is what the invocation answered
            // too.
            assert_eq!(
                read.string("Result"),
                Some(String::from_utf8_lossy(&answer.body).to_string()),
                "case {:?}: the execution carries the result its run produced, got {}",
                case.case,
                String::from_utf8_lossy(&read.body)
            );
        }
        if case.execution_status == Some(STATUS_FAILED) {
            let error = read.json().get("Error").cloned().unwrap_or(Value::Null);
            assert_eq!(
                error.get("ErrorType").and_then(Value::as_str),
                Some("TypeError"),
                "case {:?}: the execution carries the error its run failed with, got {error}",
                case.case
            );
        }
    }
}

/// `GetDurableExecutionHistory` on `arn`, which is the independent read of how many times
/// the execution really started.
async fn get_history(router: &MultiServiceRouter, arn: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/2025-12-01/durable-executions/{}/history",
            arn.replace('/', "%2F")
        ),
        &Value::Null,
    )
    .await
}

/// How many `ExecutionStarted` events the execution's history holds: one per start, so a
/// de-duplicated repeat must leave it at one however the environment behaved.
fn starts(history: &Answer) -> usize {
    history
        .json()
        .get("Events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter(|event| {
                    event.get("EventType").and_then(Value::as_str) == Some("ExecutionStarted")
                })
                .count()
        })
        .unwrap_or_default()
}

/// A repeat naming a live execution with the same payload answers the execution already
/// started and runs nothing a second time: "Lambda returns the existing execution instead
/// of creating a duplicate." One row per outcome the execution being repeated came to.
#[tokio::test]
async fn a_synchronous_repeat_of_a_live_execution_name_runs_nothing_again() {
    // The case, the handler, and the `FunctionError` every answer for it must carry.
    let cases: &[(&str, &str, Option<&str>)] = &[
        (
            "an execution whose run returned",
            RETURNING_HANDLER,
            None,
        ),
        (
            "an execution whose run failed",
            FAILING_HANDLER,
            Some("Unhandled"),
        ),
    ];
    for (index, (case, source, function_error)) in cases.iter().enumerate() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = format!("durable-handle-repeat-{index}");
        create_runnable_function(&router, &engine, &function, source, true).await;

        let first = invoke(&router, &function, "RequestResponse", Some("once"), b"{}").await;
        assert_eq!(
            first.status,
            200,
            "case {case:?}: the first invocation must be served, got {} {}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let arn = first.header(ARN_HEADER).unwrap_or_default();
        assert!(
            !arn.is_empty(),
            "case {case:?}: the first invocation must answer an execution ARN, got {:?}",
            first.headers
        );

        for repeat in 1..=2 {
            let again = invoke(&router, &function, "RequestResponse", Some("once"), b"{}").await;
            assert_eq!(
                (again.status, again.header(ARN_HEADER)),
                (200, Some(arn.clone())),
                "case {case:?} repeat {repeat}: the same name and payload answer the \
                 execution already started, got {} {}",
                again.status,
                String::from_utf8_lossy(&again.body)
            );
            match function_error {
                // A handler that returned: the repeat answers the result the execution
                // holds, which is byte for byte what the first invocation answered.
                None => assert_eq!(
                    String::from_utf8_lossy(&again.body),
                    String::from_utf8_lossy(&first.body),
                    "case {case:?} repeat {repeat}: a de-duplicated invocation is the same \
                     invocation answered again, so the answer is what the execution came to"
                ),
                // A handler that failed: the repeat answers the error the execution holds,
                // which is the error class and message the first invocation reported (the
                // stack of a run that is over is not part of what the execution keeps).
                Some(_) => {
                    let member = |answer: &Answer, name: &str| {
                        answer
                            .json()
                            .get(name)
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    };
                    assert_eq!(
                        (
                            member(&again, "errorType"),
                            member(&again, "errorMessage")
                        ),
                        (
                            member(&first, "errorType"),
                            member(&first, "errorMessage")
                        ),
                        "case {case:?} repeat {repeat}: the repeat reports the failure the \
                         execution holds, got {}",
                        String::from_utf8_lossy(&again.body)
                    );
                }
            }
            assert_eq!(
                again.header("x-amz-function-error").as_deref(),
                *function_error,
                "case {case:?} repeat {repeat}: the repeat carries the signal the \
                 execution's own outcome carries, got {:?}",
                again.header("x-amz-function-error")
            );
            // The history is the independent read: one start, whatever the environment did.
            let history = get_history(&router, &arn).await;
            assert_eq!(
                (history.status, starts(&history)),
                (200, 1),
                "case {case:?} repeat {repeat}: the execution must still hold exactly one \
                 start, got {} {}",
                history.status,
                String::from_utf8_lossy(&history.body)
            );
        }

        // A repeat whose payload differs is the modelled refusal, and answers no handle.
        let differing = invoke(
            &router,
            &function,
            "RequestResponse",
            Some("once"),
            br#"{"other":true}"#,
        )
        .await;
        assert_eq!(
            differing.status,
            409,
            "case {case:?}: a repeat with a different payload is refused 409, got {} {}",
            differing.status,
            String::from_utf8_lossy(&differing.body)
        );
        assert!(
            differing
                .error_code()
                .contains("DurableExecutionAlreadyStartedException"),
            "case {case:?}: expected the modelled DurableExecutionAlreadyStartedException, \
             got {:?}",
            differing.error_code()
        );
    }
}

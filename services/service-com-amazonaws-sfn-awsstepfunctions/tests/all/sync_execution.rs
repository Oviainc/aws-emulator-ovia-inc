// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartSyncExecution` — the SYNCHRONOUS start of an Express
//! workflow — driven over this service's own wire protocol (`awsJson1_0`, `POST /`
//! with `X-Amz-Target: AWSStepFunctions.StartSyncExecution`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. a synchronous start INTERPRETS the state machine's Amazon States Language
//!    definition in the call itself and answers the TERMINAL outcome that walk
//!    computed — the `status`, the `output` or the `error`/`cause`, the input it ran
//!    on, the span it ran for and what that span is billed as — with HTTP 200 whether
//!    the execution succeeded or failed ("StartSyncExecution will return a 200 OK
//!    response, even if your execution fails"), and a definition this emulator cannot
//!    run inside its own process ends `FAILED` with ASL's `States.Runtime` rather
//!    than a fabricated success;
//! 2. it serves EXPRESS workflows only ("StartSyncExecution is not available for
//!    STANDARD workflows"), refusing any other type with the modelled
//!    `StateMachineTypeNotSupported`, and refusing a request it cannot serve for any
//!    other reason with that reason's own modelled error;
//! 3. every run is PERSISTED as a terminal execution of its state machine under its
//!    own Express execution ARN, whose trailing id is what tells two runs that reused
//!    one name apart ("for EXPRESS workflows, execution names can be reused
//!    immediately").
//!
//! Each table below spans one class of those rules. The recorded Express workflow
//! this suite's issue was filed about has a `Task` state whose `Resource` is a bare
//! Lambda function ARN, which names the FUNCTION the task invokes
//! (`state::integration`); that invocation is driven where Lambda is mounted beside
//! this service (`core/router/tests/all/sfn_lambda_task_invocation.rs`), and the row
//! here is the rule it shares with every other state — a `Resource` this emulator
//! makes no call for is reported rather than faked.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    string, Emulator, ACCOUNT, HELLO_DEFINITION, REGION, ROLE_ARN,
};

/// Create an EXPRESS state machine that must succeed, and return its ARN.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so the
/// suite that drives `StartSyncExecution` for ANOTHER rule — the diagnostic a
/// refusal quotes — reads this helper rather than carrying a second copy of it.
pub(crate) async fn create_express(emulator: &Emulator, name: &str, definition: &str) -> String {
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": definition,
                "roleArn": ROLE_ARN,
                "type": "EXPRESS",
            }),
        )
        .await;
    string(&created, "stateMachineArn")
}

/// The timestamp member `name` of `body`, in whole milliseconds since the epoch.
///
/// An `awsJson1_0` timestamp is epoch SECONDS with a fractional part, so the two
/// instants a run is measured between are read through this one reader.
fn epoch_millis(body: &Value, name: &str) -> i64 {
    let seconds = body
        .get(name)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a timestamp '{name}' in {body:?}"));
    (seconds * 1000.0).round() as i64
}

/// The long member `name` of the `billingDetails` of `body`.
fn billed(body: &Value, name: &str) -> i64 {
    body.get("billingDetails")
        .and_then(|details| details.get(name))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a billingDetails '{name}' in {body:?}"))
}

/// The JSON document a string member of `body` carries, or `None` when the member is
/// absent or null.
pub(crate) fn document(body: &Value, name: &str) -> Option<Value> {
    body.get(name)
        .filter(|value| !value.is_null())
        .map(|value| {
            let text = value
                .as_str()
                .unwrap_or_else(|| panic!("expected '{name}' to be a string in {body:?}"));
            serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("expected '{name}' to be JSON: {error} in {text}"))
        })
}

/// Whether `included` is set on the `name` details structure of `body`.
fn details_included(body: &Value, name: &str) -> Option<bool> {
    body.get(name)
        .and_then(|details| details.get("included"))
        .and_then(Value::as_bool)
}

// ---------------------------------------------------------------------------
// The outcome the call computed
// ---------------------------------------------------------------------------

/// One row of the synchronous-run table.
struct SyncCase {
    case: &'static str,
    definition: &'static str,
    /// The `input` the request carries, or `None` to send none at all.
    input: Option<&'static str>,
    /// The `SyncExecutionStatus` the run must end in.
    status: &'static str,
    /// The JSON output a succeeding run must answer, if any.
    output: Option<&'static str>,
    /// The `error` name a failing run must answer, if any.
    error: Option<&'static str>,
    /// The least the run can have taken, in milliseconds: the total of the `Wait`
    /// states its definition declares.
    least_millis: i64,
}

/// A synchronous start answers the terminal outcome it computed from the definition
/// and the input, with 200 whether the execution succeeded or failed.
///
/// Rows vary along what the definition does (a static result, data flow, a `Choice`,
/// a `Wait`, a `Fail`, iteration), the input it runs on (an object, an array, none at
/// all), the status it ends in, and whether it can be run in this process at all.
#[tokio::test]
async fn a_sync_start_answers_the_outcome_the_definition_computed() {
    let cases = [
        SyncCase {
            case: "a Pass state's static result",
            definition: HELLO_DEFINITION,
            input: Some("{}"),
            status: "SUCCEEDED",
            output: Some(r#"{"msg":"hi"}"#),
            error: None,
            least_millis: 0,
        },
        SyncCase {
            case: "no input at all runs on the empty object",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","End":true}}}"#,
            input: None,
            status: "SUCCEEDED",
            output: Some("{}"),
            error: None,
            least_millis: 0,
        },
        SyncCase {
            case: "data flow computes the output from the input",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"who.$":"$.name","size.$":"States.ArrayLength($.list)"},"End":true}}}"#,
            input: Some(r#"{"name":"ada","list":[1,2,3]}"#),
            status: "SUCCEEDED",
            output: Some(r#"{"who":"ada","size":3}"#),
            error: None,
            least_millis: 0,
        },
        SyncCase {
            case: "a Choice that reads the input picks the branch",
            definition: r#"{"StartAt":"C","States":{"C":{"Type":"Choice","Choices":[{"Variable":"$.n","NumericGreaterThan":1,"Next":"Big"}],"Default":"Small"},"Big":{"Type":"Pass","Result":"big","End":true},"Small":{"Type":"Pass","Result":"small","End":true}}}"#,
            input: Some(r#"{"n":7}"#),
            status: "SUCCEEDED",
            output: Some(r#""big""#),
            error: None,
            least_millis: 0,
        },
        SyncCase {
            case: "an inline Map iterates the array it was given",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","Result":"seen","End":true}}},"End":true}}}"#,
            input: Some("[1,2]"),
            status: "SUCCEEDED",
            output: Some(r#"["seen","seen"]"#),
            error: None,
            least_millis: 0,
        },
        SyncCase {
            // The walk never sleeps, so the span the definition declares is
            // REPORTED rather than slept through: the outcome is answered now and
            // dated at the instant the run reached it.
            case: "a Wait state's duration is in the span the run reports",
            definition: r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":1,"End":true}}}"#,
            input: Some(r#"{"kept":1}"#),
            status: "SUCCEEDED",
            output: Some(r#"{"kept":1}"#),
            error: None,
            least_millis: 1_000,
        },
        SyncCase {
            case: "a Fail state ends the run with its own error",
            definition: r#"{"StartAt":"F","States":{"F":{"Type":"Fail","Error":"Boom","Cause":"the definition said so"}}}"#,
            input: Some("{}"),
            status: "FAILED",
            output: None,
            error: Some("Boom"),
            least_millis: 0,
        },
        SyncCase {
            // A `Task` state whose `Resource` is a bare resource ARN of a service
            // whose resources are not work in themselves, which names none of the
            // service integrations this emulator calls (`state::integration`), so
            // nothing is run for it and nothing is faked.
            case: "a Task state whose Resource names no integration call is not run",
            definition: r#"{"StartAt":"T","States":{"T":{"Type":"Task","Resource":"arn:aws:dynamodb:us-west-2:000000000000:table/t","End":true}}}"#,
            input: Some(r#"{"first_name":"Alejandro"}"#),
            status: "FAILED",
            output: None,
            error: Some("States.Runtime"),
            least_millis: 0,
        },
        SyncCase {
            // Both query languages are evaluated, so a JSONata definition RUNS here
            // too: its one `Pass` carries the input through, which is what a
            // JSONata state with no `Output` puts out.
            case: "a JSONata definition is evaluated",
            definition: r#"{"QueryLanguage":"JSONata","StartAt":"A","States":{"A":{"Type":"Pass","Output":"{% { 'doubled': $states.input.n * 2 } %}","End":true}}}"#,
            input: Some(r#"{"n":21}"#),
            status: "SUCCEEDED",
            output: Some(r#"{"doubled":42}"#),
            error: None,
            least_millis: 0,
        },
        SyncCase {
            case: "a Choice no rule matched reports ASL's own error",
            definition: r#"{"StartAt":"C","States":{"C":{"Type":"Choice","Choices":[{"Variable":"$.n","NumericEquals":1,"Next":"P"}]},"P":{"Type":"Pass","End":true}}}"#,
            input: Some(r#"{"n":2}"#),
            status: "FAILED",
            output: None,
            error: Some("States.NoChoiceMatched"),
            least_millis: 0,
        },
    ];

    let emulator = Emulator::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.case;
        let machine_name = format!("sync-{index}");
        let machine = create_express(&emulator, &machine_name, case.definition).await;

        let mut request = json!({ "stateMachineArn": machine, "name": "run" });
        if let Some(input) = case.input {
            request["input"] = Value::String(input.to_string());
        }
        let answered = emulator.ok("StartSyncExecution", &request).await;

        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some(case.status),
            "case {label:?}: expected status {:?}, got {answered:?}",
            case.status
        );
        assert_eq!(
            document(&answered, "output"),
            case.output
                .map(|output| serde_json::from_str(output).expect("the expected output is JSON")),
            "case {label:?}: expected output {:?}, got {answered:?}",
            case.output
        );
        assert_eq!(
            answered.get("error").and_then(Value::as_str),
            case.error,
            "case {label:?}: expected error {:?}, got {answered:?}",
            case.error
        );
        if case.error.is_some_and(|error| error.starts_with("States.")) {
            assert!(
                answered
                    .get("cause")
                    .and_then(Value::as_str)
                    .is_some_and(|cause| !cause.is_empty()),
                "case {label:?}: a failure this emulator raised must say what happened, got \
                 {answered:?}"
            );
        }

        // The run's own data: the input it ran on comes back, and the details
        // structures say the data is included ("Always true for API calls"), the
        // output's only where there is an output to report.
        assert_eq!(
            document(&answered, "input"),
            Some(
                serde_json::from_str(case.input.unwrap_or("{}"))
                    .expect("the request input is JSON")
            ),
            "case {label:?}: expected the input the run ran on, got {answered:?}"
        );
        assert_eq!(
            details_included(&answered, "inputDetails"),
            Some(true),
            "case {label:?}: the input details must report the data as included, got {answered:?}"
        );
        assert_eq!(
            details_included(&answered, "outputDetails"),
            case.output.map(|_| true),
            "case {label:?}: the output details follow the output, got {answered:?}"
        );

        // The identity of the run: its state machine, the name it was given, and an
        // Express execution ARN whose trailing id makes it unique.
        assert_eq!(
            answered.get("stateMachineArn").and_then(Value::as_str),
            Some(machine.as_str()),
            "case {label:?}: expected the state machine's own ARN, got {answered:?}"
        );
        assert_eq!(
            answered.get("name").and_then(Value::as_str),
            Some("run"),
            "case {label:?}: expected the execution name the request gave, got {answered:?}"
        );
        let execution_arn = string(&answered, "executionArn");
        let fields: Vec<&str> = execution_arn.split(':').collect();
        assert_eq!(
            fields.len(),
            9,
            "case {label:?}: an Express execution ARN carries an id field, got {execution_arn}"
        );
        assert_eq!(
            &fields[..8],
            &[
                "arn",
                "aws",
                "states",
                REGION,
                ACCOUNT,
                "express",
                machine_name.as_str(),
                "run",
            ],
            "case {label:?}: unexpected execution ARN {execution_arn}"
        );
        assert!(
            fields[8].len() == 36
                && fields[8]
                    .chars()
                    .all(|character| character.is_ascii_hexdigit() || character == '-'),
            "case {label:?}: expected a UUID id field, got {execution_arn}"
        );

        // The span the run took, and what that span is billed as: the duration in
        // whole 100 ms increments and the memory in 64 MB increments, never less
        // than one increment of either.
        let span = epoch_millis(&answered, "stopDate") - epoch_millis(&answered, "startDate");
        assert!(
            span >= case.least_millis,
            "case {label:?}: expected a span of at least {} ms, got {span} ms in {answered:?}",
            case.least_millis
        );
        let duration = billed(&answered, "billedDurationInMilliseconds");
        assert!(
            duration >= case.least_millis.max(100) && duration % 100 == 0,
            "case {label:?}: expected a billed duration in whole 100 ms increments covering \
             {span} ms, got {duration}"
        );
        let memory = billed(&answered, "billedMemoryUsedInMB");
        assert!(
            memory >= 64 && memory % 64 == 0,
            "case {label:?}: expected billed memory in whole 64 MB increments, got {memory}"
        );
    }
}

// ---------------------------------------------------------------------------
// The workflow types it serves
// ---------------------------------------------------------------------------

/// `StartSyncExecution` serves EXPRESS workflows and refuses every other type with
/// the modelled `StateMachineTypeNotSupported`.
///
/// Rows span the `StateMachineType` enum the model declares (`EXPRESS`, `STANDARD`)
/// and the type a create that names none takes ("the default is STANDARD").
#[tokio::test]
async fn a_sync_start_serves_express_workflows_only() {
    // (case, the `type` the create names, whether the sync start is served)
    let cases: [(&str, Option<&str>, bool); 3] = [
        ("an EXPRESS workflow", Some("EXPRESS"), true),
        ("an explicit STANDARD workflow", Some("STANDARD"), false),
        ("a create that names no type defaults to STANDARD", None, false),
    ];

    let emulator = Emulator::new();
    for (index, (case, workflow_type, served)) in cases.iter().enumerate() {
        let mut create = json!({
            "name": format!("typed-{index}"),
            "definition": HELLO_DEFINITION,
            "roleArn": ROLE_ARN,
        });
        if let Some(workflow_type) = workflow_type {
            create["type"] = Value::String((*workflow_type).to_string());
        }
        let machine = string(
            &emulator.ok("CreateStateMachine", &create).await,
            "stateMachineArn",
        );

        let request = json!({ "stateMachineArn": machine, "input": "{}" });
        if *served {
            let answered = emulator.ok("StartSyncExecution", &request).await;
            assert_eq!(
                answered.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: expected the run's outcome, got {answered:?}"
            );
        } else {
            let (status, code) = emulator.err("StartSyncExecution", &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "StateMachineTypeNotSupported"),
                "case {case:?}: expected a 400 StateMachineTypeNotSupported, got {status} {code}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The requests it refuses
// ---------------------------------------------------------------------------

/// A request this operation cannot serve is refused with that reason's own modelled
/// error — never with a 501 and never with a fabricated run.
///
/// Rows vary along what is wrong with the request: the ARN's syntax, its service, its
/// region, the resource it qualifies, the execution name's characters, the input's
/// JSON, and a state machine that is not there.
#[tokio::test]
async fn a_sync_start_refuses_a_request_it_cannot_serve() {
    let emulator = Emulator::new();
    let machine = create_express(&emulator, "refusals", HELLO_DEFINITION).await;
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // (case, the request body, the expected error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "text that is not an ARN",
            json!({ "stateMachineArn": "not-an-arn" }),
            "InvalidArn",
        ),
        (
            "an ARN of another service",
            json!({ "stateMachineArn": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:f") }),
            "InvalidArn",
        ),
        (
            "an ARN of another region",
            json!({ "stateMachineArn": format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:elsewhere") }),
            "InvalidArn",
        ),
        (
            "an ARN naming another resource type",
            json!({ "stateMachineArn": format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:act") }),
            "InvalidArn",
        ),
        (
            "a map-run labeled state machine ARN",
            json!({ "stateMachineArn": format!("{machine}/mapStateLabel") }),
            "ValidationException",
        ),
        (
            "a state machine that was never created",
            json!({ "stateMachineArn": absent.clone() }),
            "StateMachineDoesNotExist",
        ),
        (
            "an execution name carrying whitespace",
            json!({ "stateMachineArn": machine.clone(), "name": "two words" }),
            "InvalidName",
        ),
        (
            "an execution name carrying a forbidden character",
            json!({ "stateMachineArn": machine.clone(), "name": "a*b" }),
            "InvalidName",
        ),
        (
            "an input that is not JSON",
            json!({ "stateMachineArn": machine.clone(), "input": "not json" }),
            "InvalidExecutionInput",
        ),
        (
            "an input that is a truncated document",
            json!({ "stateMachineArn": machine.clone(), "input": "{\"a\":" }),
            "InvalidExecutionInput",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, code) = emulator.err("StartSyncExecution", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected a 400 {expected}, got {status} {code}"
        );
    }
}

/// A state machine whose deletion has begun runs nothing: the start is refused with
/// the modelled `StateMachineDeleting` while the record is still there.
#[tokio::test]
async fn a_sync_start_on_a_deleting_state_machine_is_refused() {
    let emulator = Emulator::new();
    let machine = create_express(&emulator, "deleting", HELLO_DEFINITION).await;
    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": machine }))
        .await;

    let (status, code) = emulator
        .err(
            "StartSyncExecution",
            &json!({ "stateMachineArn": machine, "input": "{}" }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "StateMachineDeleting"),
        "a started deletion refuses a synchronous start, got {status} {code}"
    );
}

// ---------------------------------------------------------------------------
// What the run leaves behind
// ---------------------------------------------------------------------------

/// Every synchronous run is persisted as a TERMINAL execution of its state machine,
/// and a reused name is a second record rather than a refusal.
///
/// Rows vary along the name the request gives: the same name twice (which Express
/// workflows allow) and no name at all (which the service mints a UUID for).
#[tokio::test]
async fn every_sync_run_is_persisted_under_its_own_execution_arn() {
    let emulator = Emulator::new();
    let machine = create_express(&emulator, "persisted", HELLO_DEFINITION).await;

    // (case, the `name` the request gives, the name the response must report)
    let cases: [(&str, Option<&str>); 3] = [
        ("a named run", Some("twin")),
        ("the same name again", Some("twin")),
        ("a run with no name", None),
    ];

    let mut answered = Vec::with_capacity(cases.len());
    for (case, name) in cases {
        let mut request = json!({ "stateMachineArn": machine, "input": r#"{"n":1}"# });
        if let Some(name) = name {
            request["name"] = Value::String(name.to_string());
        }
        let response = emulator.ok("StartSyncExecution", &request).await;
        let reported = string(&response, "name");
        match name {
            Some(name) => assert_eq!(
                reported, name,
                "case {case:?}: expected the name the request gave, got {response:?}"
            ),
            // "If you don't provide a name for the execution, Step Functions
            // automatically generates a universally unique identifier (UUID) as the
            // execution name."
            None => assert!(
                reported.len() == 36
                    && reported
                        .chars()
                        .all(|character| character.is_ascii_hexdigit() || character == '-'),
                "case {case:?}: expected a minted UUID name, got {reported}"
            ),
        }
        answered.push((case, response));
    }

    let arns: Vec<String> = answered
        .iter()
        .map(|(_, response)| string(response, "executionArn"))
        .collect();
    for (index, arn) in arns.iter().enumerate() {
        assert!(
            arns.iter().skip(index + 1).all(|other| other != arn),
            "every run has its own execution ARN, got {arns:?}"
        );
    }

    // An Express execution is not readable through this API — `DescribeExecution` and
    // `ListExecutions` each refuse one, and `GetExecutionHistory` is "not supported by
    // EXPRESS state machines" — so what was persisted is asserted against the store
    // the service answers from.
    // The guard is scoped to this block, so it is released before the call below.
    {
        let store = emulator.store();
        let store = store.lock();
        for ((case, response), arn) in answered.iter().zip(&arns) {
            let record = store
                .executions
                .get(arn)
                .unwrap_or_else(|| panic!("case {case:?}: expected the store to hold {arn}"));
            assert_eq!(
                record.status,
                string(response, "status"),
                "case {case:?}: the stored status must be the one the call answered"
            );
            assert_eq!(
                record.state_machine_arn, machine,
                "case {case:?}: the record belongs to the state machine it ran"
            );
            assert_eq!(
                record.input,
                string(response, "input"),
                "case {case:?}: the stored input must be the one the run ran on"
            );
            assert_eq!(
                record.output.as_deref(),
                response.get("output").and_then(Value::as_str),
                "case {case:?}: the stored output must be the one the call answered"
            );
            assert!(
                record.stop_date.is_some() && record.pending.is_none(),
                "case {case:?}: a synchronous run is already terminal, with nothing owed to the \
                 clock, got {record:?}"
            );
            assert!(
                !record.history.is_empty(),
                "case {case:?}: the run's history is what it executed, got {record:?}"
            );
        }
    }

    let (status, code) = emulator
        .err("DescribeExecution", &json!({ "executionArn": arns[0] }))
        .await;
    assert_eq!(
        status, 400,
        "an Express execution is not described through this API, got {status} {code}"
    );
}

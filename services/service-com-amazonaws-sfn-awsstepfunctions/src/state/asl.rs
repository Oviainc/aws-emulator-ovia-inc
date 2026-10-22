// SPDX-License-Identifier: CC0-1.0

//! The Amazon States Language (ASL) this emulator EXECUTES.
//!
//! ASL is Step Functions' own definition language; no crates.io crate runs it, so
//! the walk below is written here, over `serde_json` — the maintained parser for the
//! JSON the language is written in — and [`serde_json_path`], the maintained RFC
//! 9535 implementation of JSONPath, which is the query language every `*Path` member
//! and every `"<name>.$"` payload-template key is written in.
//!
//! ## What is executed, and what is not
//!
//! Every state whose semantics are a function of the execution's own data is RUN,
//! not faked: `Pass`, `Wait`, `Choice`, `Succeed`, `Fail`, `Map` (inline and
//! Distributed) and `Parallel`, together with the data-flow members that decide what
//! each of them reads and writes (`InputPath`, `Parameters`, `ItemSelector`,
//! `ItemsPath`, `ResultSelector`, `ResultPath`, `OutputPath`), the context object
//! (`$$`), the intrinsic functions, `Retry` and `Catch`.
//!
//! What is NOT executed is what cannot be executed inside this process:
//!
//! * a `Task` state's service integration, which calls another service;
//! * a Distributed Map's `ItemReader`, `ItemBatcher` and `ResultWriter`, which read
//!   and write an S3 dataset;
//! * a JSONata-mode definition (`QueryLanguage: "JSONata"`) and the members that
//!   only exist in that mode (`Assign`, `Arguments`, `Output`).
//!
//! Reaching one of those ends the execution in the service's own failure state:
//! `FAILED`, with the ASL-documented [`RUNTIME_ERROR`] name and a cause that says
//! exactly what was not run. Nothing is synthesised to look as though it ran. One
//! documented intrinsic, `States.Hash`, is DECLINED for a different reason: the
//! algorithms it names include MD5 and SHA-1, and this emulator does not take a
//! dependency on broken hash functions to evaluate a payload template; it answers
//! [`INTRINSIC_ERROR`], as any unknown intrinsic does.
//!
//! ## What bounds a run
//!
//! Every input the walk reads — the definition, the execution input, each path and
//! each intrinsic invocation inside them — is a caller's, and the definition is
//! bounded only at the modelled 1 MiB, so each dimension of a run carries its own
//! ceiling and a run past one ends in a modelled failure rather than in the thread's
//! stack or the process's memory:
//!
//! * [`MAX_STATE_TRANSITIONS`] — steps, spent by every nested iteration, every
//!   branch and every retry, and charged BEFORE an iteration's own work is done;
//! * [`MAX_PAYLOAD_BYTES`] and [`MAX_PAYLOAD_DEPTH`] — what one state may produce,
//!   measured at EVERY state, as a `Map`'s result array grows, as a payload template
//!   renders each member, and over an intrinsic's arguments and its result, so the
//!   PRODUCT of a template's member count and each member's size is bounded as the
//!   members are built rather than after;
//! * [`MAX_NESTING_DEPTH`] — nested `Map`/`Parallel` levels;
//! * [`MAX_EXPRESSION_DEPTH`] and [`MAX_EXPRESSION_CHARS`] — one intrinsic
//!   invocation, whose nesting lives inside a single JSON string, and one `Choice`
//!   rule's `And`/`Or`/`Not` condition tree;
//! * [`MAX_PATH_CHARS`] and [`MAX_PATH_SEGMENTS`] — one path, checked BEFORE the
//!   JSONPath engine evaluates it, since a node list it has already built cannot be
//!   refused after the fact. Every path must be a REFERENCE path, which identifies
//!   one node, so the cost of an evaluation is its segment count rather than the
//!   size of the document;
//! * [`MAX_CHOICE_RULES`] and [`MAX_TEMPLATE_PATHS`] — how many expressions ONE
//!   visit to a state may evaluate, and [`MAX_GLOB_CHARS`] what one `StringMatches`
//!   may cost;
//! * [`MAX_INTERPRET_DURATION`] — the wall clock, checked at every transition and
//!   every `Map` item, which bounds the PRODUCT of all of the above; the walk runs on
//!   a blocking thread, so a run that reaches it never held an async worker;
//! * [`MAX_STATE_NAME_CHARS`] — a state's name, which every history event of that
//!   state carries, enforced by [`validate`] over every state list the definition
//!   declares (nested processors and branches included) and again in the walk.
//!
//! The two walks over caller-shaped data that could otherwise recurse as deeply as
//! the data nests — measuring a document and assigning through a reference path — are
//! written ITERATIVELY.
//!
//! A Distributed Map's items are interpreted in this process, by the same walk that
//! runs an inline Map: the state's output is therefore computed from the items the
//! definition selected. The child EXECUTION records and the Map Run a real
//! Distributed Map also creates are not stored yet — `DescribeMapRun` and
//! `ListMapRuns` are still scaffolded, and their own stub issues own that.
//!
//! ## Time
//!
//! The walk never sleeps. Every `Wait` it passes adds to the offset at which the
//! outcome becomes due ([`Run::elapsed`]), which `StartExecution` parks in a
//! `PendingOutcome`; the states of a `Map` iteration or a `Parallel` branch run
//! concurrently on the real service, so a batch's span is its longest member's
//! rather than the sum.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use serde_json_path::JsonPath;

/// The ASL predefined error name for a failure the interpreter itself raises
/// ("States.Runtime — an execution failed due to some exception that could not be
/// processed").
pub const RUNTIME_ERROR: &str = "States.Runtime";

/// The ASL predefined error name for a payload that outgrew what a state may carry
/// ("States.DataLimitExceeded").
pub const DATA_LIMIT_ERROR: &str = "States.DataLimitExceeded";

/// The ASL predefined error name for a `ResultPath` that could not be applied to the
/// state's input ("States.ResultPathMatchFailure").
pub const RESULT_PATH_ERROR: &str = "States.ResultPathMatchFailure";

/// The ASL predefined error name for a `Parameters`/`ItemSelector` payload template
/// whose path could not be resolved ("States.ParameterPathFailure").
pub const PARAMETER_PATH_ERROR: &str = "States.ParameterPathFailure";

/// The ASL predefined error name for an intrinsic function that could not be
/// evaluated ("States.IntrinsicFailure").
pub const INTRINSIC_ERROR: &str = "States.IntrinsicFailure";

/// The ASL predefined error name for a `Choice` state no rule matched and that
/// declares no `Default` ("States.NoChoiceMatched").
pub const NO_CHOICE_ERROR: &str = "States.NoChoiceMatched";

/// The ASL predefined error name for a Distributed Map whose failed items went past
/// its tolerated-failure threshold ("States.ExceedToleratedFailureThreshold").
pub const TOLERATED_FAILURE_ERROR: &str = "States.ExceedToleratedFailureThreshold";

/// The ASL predefined error name for an execution that ran longer than the
/// `TimeoutSeconds` its definition declares ("States.Timeout").
pub const TIMEOUT_ERROR: &str = "States.Timeout";

/// The most state transitions one execution makes before the interpreter stops it.
///
/// Emulator step budget, NOT an AWS quota: it is what bounds a definition whose
/// `Next` chain forms a cycle, and — because every `Map` iteration and `Parallel`
/// branch spends from the same budget — what bounds the total work one execution
/// does. An execution that exceeds it ends `FAILED` with [`RUNTIME_ERROR`].
pub const MAX_STATE_TRANSITIONS: usize = 1_000;

/// The most characters a state name may carry.
///
/// From the Amazon States Language: a state's name "must be unique within the scope
/// of the whole state machine and has a maximum length of 80 characters". A longer
/// one is refused at create time, which is also what keeps an execution's history
/// bounded in BYTES: every history event carries the name of the state it happened
/// in.
pub const MAX_STATE_NAME_CHARS: usize = 80;

/// The most bytes a state's output may carry: the maximum the modelled
/// `SensitiveData` shape that `input`/`output` target accepts (0..262144).
///
/// It is checked at EVERY state, on the document that state produced, not only on
/// the one the run ends with: a definition that grows its document — a `Pass` that
/// duplicates its own input and loops back to itself, say — is stopped at the first
/// state past the limit, with ASL's own [`DATA_LIMIT_ERROR`], rather than being
/// allowed to grow until the run finishes.
pub const MAX_PAYLOAD_BYTES: usize = 262_144;

/// The most levels a state's document may nest.
///
/// Emulator bound, NOT an AWS quota: `serde_json` refuses to PARSE a document nested
/// deeper than its own 128-level recursion limit, so this is the same ceiling applied
/// to a document the walk BUILT (a payload template or a `Map` result can nest one
/// level deeper per transition). Every later reader — `serde_json`'s own serializer
/// included — walks a document recursively, so a document past this is reported with
/// [`DATA_LIMIT_ERROR`] instead of being carried on.
pub const MAX_PAYLOAD_DEPTH: usize = 128;

/// The most levels of `Map` iterations and `Parallel` branches one run may nest.
///
/// Emulator bound: each level is an interpreter frame, so the nesting a caller's
/// definition can ask for is capped rather than left to the thread's stack. A
/// definition past it ends `FAILED` with [`RUNTIME_ERROR`].
pub const MAX_NESTING_DEPTH: usize = 16;

/// The most levels an intrinsic-function invocation may nest
/// (`States.Array(States.Array(…))`).
///
/// Emulator bound on a hand-written parser: the nesting lives inside ONE JSON string,
/// so `serde_json`'s own recursion limit does not bound it. A deeper invocation is
/// reported with [`INTRINSIC_ERROR`].
pub const MAX_EXPRESSION_DEPTH: usize = 16;

/// The most characters one intrinsic-function invocation may carry.
///
/// Emulator bound, for the same reason: the invocation text is a value of a
/// definition the model bounds only at 1 MiB, and this parser is this crate's own.
pub const MAX_EXPRESSION_CHARS: usize = 4_096;

/// The most segments a reference path (`ResultPath`, `$.a.b[0]`) may carry.
///
/// Emulator bound: ASL reference paths name a place in a document, so they are short,
/// and the path text comes from a definition bounded only at 1 MiB. A longer one is
/// reported with [`RESULT_PATH_ERROR`].
pub const MAX_PATH_SEGMENTS: usize = 64;

/// The most characters any `*Path` member, `Variable` or payload-template path may
/// carry.
///
/// Emulator bound on EVERY path, before it reaches the JSONPath engine: the text is a
/// value of a definition bounded only at 1 MiB and it is evaluated once per visit to
/// its state, so its length — and the reference-path form [`select_path`] requires —
/// is what bounds the work one path can ask for.
pub const MAX_PATH_CHARS: usize = 1_024;

/// The most choice rules one `Choice` state, or one `And`/`Or` condition, may carry —
/// and the most branches one `Parallel` state may declare.
///
/// Emulator bound: a `Choice` evaluates its rules until one matches and a `Parallel`
/// runs every branch, so the count multiplies the work one visit to that state does.
/// It is otherwise bounded only by the 1 MiB definition.
pub const MAX_CHOICE_RULES: usize = 64;

/// The most `"<name>.$"` members one payload template may carry.
///
/// Emulator bound, for the same reason: each one is an expression evaluated on every
/// visit to the state, so the member count multiplies that state's work.
pub const MAX_TEMPLATE_PATHS: usize = 64;

/// The most characters a `StringMatches` pattern may carry.
///
/// Emulator bound: matching a glob against a value costs the pattern's length times
/// the value's, and the value is bounded by [`MAX_PAYLOAD_BYTES`].
pub const MAX_GLOB_CHARS: usize = 256;

/// The longest this emulator INTERPRETS one definition for.
///
/// Emulator deadline, NOT an AWS duration: it bounds the PRODUCT the other ceilings
/// bound factor by factor (transitions x evaluations per visit x the work of one
/// evaluation), and it is what makes the walk interruptible — it is checked at every
/// transition and at every `Map` item, and a run past it ends `FAILED` with
/// [`RUNTIME_ERROR`] rather than holding the thread it runs on. The walk itself runs
/// on a blocking thread (`StartExecution` and `RedriveExecution` hand it to
/// `spawn_blocking`), so an over-budget interpretation never occupies an async
/// worker.
pub const MAX_INTERPRET_DURATION: Duration = Duration::from_secs(2);

/// The most values `States.ArrayRange` builds.
///
/// Emulator bound: the array is built from three numbers, so its length is not
/// bounded by the size of anything the caller sent. Each value is a number, so this
/// keeps the array inside what a state's output may carry.
pub const MAX_ARRAY_RANGE_VALUES: usize = 10_000;

/// The most items an array an intrinsic function walks may carry.
///
/// Emulator bound: an array an intrinsic is handed comes out of the execution input,
/// which is bounded in BYTES and not in element COUNT, so an array of tiny elements
/// can carry on the order of 10^5 of them. Every intrinsic that walks one is linear
/// in the count, and this is the ceiling on that count, reported with
/// [`INTRINSIC_ERROR`].
pub const MAX_ARRAY_ITEMS: usize = 10_000;

/// The most levels of nested state machines (`ItemProcessor`, `Iterator`,
/// `Branches`) a definition may declare for `validate` to walk.
///
/// `serde_json` refuses to parse a document nested deeper than its own 128-level
/// recursion limit, so this is that same ceiling stated explicitly, which is what
/// bounds the structural walk below.
pub const MAX_DEFINITION_DEPTH: usize = 128;

/// The most characters `States.Base64Encode`/`States.Base64Decode` accept: "You can
/// encode input data up to 10,000 characters."
pub const MAX_BASE64_CHARS: usize = 10_000;

/// The most bytes a `Fail` state's `Error` and `Cause` may carry: the maxima of the
/// modelled `SensitiveError` (0..256) and `SensitiveCause` (0..32768) shapes
/// `DescribeExecution` reports them through. A longer one is refused at create time.
pub const MAX_ERROR_BYTES: usize = 256;

/// See [`MAX_ERROR_BYTES`].
pub const MAX_CAUSE_BYTES: usize = 32_768;

/// The longest a `Wait` state waits: Step Functions' published maximum `Wait`
/// duration of one year, in seconds. A definition asking for more is out of range,
/// and ends the execution with [`RUNTIME_ERROR`] rather than being silently
/// shortened to this.
pub const MAX_WAIT_SECONDS: u64 = 31_622_400;

/// The state types this emulator executes.
pub const EXECUTED_STATE_TYPES: &[&str] =
    &["Choice", "Fail", "Map", "Parallel", "Pass", "Succeed", "Wait"];

/// The query language this interpreter evaluates. ASL's `QueryLanguage` member
/// selects between it and JSONata; a JSONata definition is not run.
const JSONPATH: &str = "JSONPath";

/// State types that end an execution, so they must NOT carry `Next`.
const TERMINAL_STATE_TYPES: &[&str] = &["Fail", "Succeed"];

/// State types that continue an execution, so each must carry `Next` or
/// `"End": true`. `Choice` is absent because it branches through its own
/// `Choices`/`Default` members instead.
const CONTINUING_STATE_TYPES: &[&str] = &["Map", "Parallel", "Pass", "Task", "Wait"];

/// `Retry`'s documented defaults: "IntervalSeconds (Optional) ... default 1",
/// "MaxAttempts (Optional) ... default 3", "BackoffRate (Optional) ... default 2.0".
const DEFAULT_RETRY_INTERVAL_SECONDS: f64 = 1.0;
const DEFAULT_RETRY_MAX_ATTEMPTS: u64 = 3;
const DEFAULT_RETRY_BACKOFF_RATE: f64 = 2.0;

/// The members each executed state type may carry. A state that carries any other
/// member is not executed: the member changes what the state does, and running the
/// state while ignoring it would report an outcome the definition does not describe.
fn executable_members(state_type: &str) -> &'static [&'static str] {
    match state_type {
        "Pass" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "InputPath",
            "Parameters",
            "Result",
            "ResultPath",
            "OutputPath",
            "Next",
            "End",
        ],
        "Wait" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "InputPath",
            "OutputPath",
            "Seconds",
            "SecondsPath",
            "Timestamp",
            "TimestampPath",
            "Next",
            "End",
        ],
        "Choice" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "InputPath",
            "OutputPath",
            "Choices",
            "Default",
        ],
        "Succeed" => &["Type", "Comment", "QueryLanguage", "InputPath", "OutputPath"],
        "Fail" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "Error",
            "ErrorPath",
            "Cause",
            "CausePath",
        ],
        // `ItemReader`, `ItemBatcher` and `ResultWriter` are deliberately absent:
        // each one reads or writes an S3 dataset, which is outside this process.
        "Map" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "InputPath",
            "Parameters",
            "ItemSelector",
            "ItemsPath",
            "Items",
            "ItemProcessor",
            "Iterator",
            "MaxConcurrency",
            "MaxConcurrencyPath",
            "ResultSelector",
            "ResultPath",
            "OutputPath",
            "Retry",
            "Catch",
            "ToleratedFailureCount",
            "ToleratedFailureCountPath",
            "ToleratedFailurePercentage",
            "ToleratedFailurePercentagePath",
            "Label",
            "Next",
            "End",
        ],
        "Parallel" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "InputPath",
            "Parameters",
            "Branches",
            "ResultSelector",
            "ResultPath",
            "OutputPath",
            "Retry",
            "Catch",
            "Next",
            "End",
        ],
        _ => &[],
    }
}

/// The first member of `state` this emulator does not execute, if any.
fn unexecutable_member(state_type: &str, state: &Map<String, Value>) -> Option<String> {
    let allowed = executable_members(state_type);
    state
        .keys()
        .find(|member| !allowed.contains(&member.as_str()))
        .cloned()
}

/// One history event the walk produced.
#[derive(Debug, Clone)]
pub struct Step {
    pub event_type: &'static str,
    pub state_name: Option<String>,
    /// How far into the run this event happened.
    pub after: Duration,
    /// Whether this is the event the execution ends on.
    pub terminal: bool,
}

/// What one walk of a definition produced.
#[derive(Debug, Clone)]
pub struct Run {
    /// `SUCCEEDED`, `FAILED` or `TIMED_OUT`.
    pub status: &'static str,
    pub output: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub steps: Vec<Step>,
    /// The offset into the run at which the outcome became due: the total of every
    /// `Wait` the walk passed.
    pub elapsed: Duration,
}

/// What the context object (`$$`) reports about the run being interpreted.
///
/// Every field is a value the caller's own request already fixed, so a definition
/// that reads `$$.Execution.Id` reads this execution's ARN rather than a placeholder.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub execution_arn: String,
    pub execution_name: String,
    pub state_machine_arn: String,
    pub state_machine_name: String,
    pub role_arn: String,
}

impl Context {
    /// The context object a definition reads through `$$`, without the per-state
    /// members [`Interpreter::context_for`] adds.
    fn value(&self, input: &Value, start: SystemTime) -> Value {
        json!({
            "Execution": {
                "Id": self.execution_arn,
                "Input": input,
                "Name": self.execution_name,
                "RoleArn": self.role_arn,
                "StartTime": iso(start),
            },
            "StateMachine": {
                "Id": self.state_machine_arn,
                "Name": self.state_machine_name,
            },
        })
    }
}

/// Why an execution, a `Map` iteration or a `Parallel` branch failed: the `error` and
/// `cause` the service reports, which `Catch` and `Retry` match against.
#[derive(Debug, Clone)]
pub struct Failure {
    pub error: Option<String>,
    pub cause: Option<String>,
}

impl Failure {
    /// This failure, at the offset into the run where it happened.
    fn at(self, at: Duration) -> Fault {
        Fault { at, failure: self }
    }

    /// The error name `Retry`'s and `Catch`'s `ErrorEquals` match against.
    fn name(&self) -> &str {
        self.error.as_deref().unwrap_or("")
    }

    /// The `{Error, Cause}` document a `Catch` hands to the state it transitions to.
    fn payload(&self) -> Value {
        let mut object = Map::new();
        if let Some(error) = &self.error {
            object.insert("Error".to_string(), Value::String(error.clone()));
        }
        if let Some(cause) = &self.cause {
            object.insert("Cause".to_string(), Value::String(cause.clone()));
        }
        Value::Object(object)
    }
}

/// A failure, and where in the run it happened.
#[derive(Debug, Clone)]
struct Fault {
    at: Duration,
    failure: Failure,
}

/// What one piece of a state's work produced: its outcome, and the offset into the
/// run at which it ended. This is what `Retry` re-runs and what `Catch` catches.
type Attempt = (Result<Value, Failure>, Duration);

/// A failure the interpreter itself raised, carrying ASL's own [`RUNTIME_ERROR`] name
/// and a cause that says what was not run.
fn runtime(cause: String) -> Failure {
    named(RUNTIME_ERROR, cause)
}

/// A failure carrying one of ASL's other predefined error names.
fn named(error: &str, cause: String) -> Failure {
    Failure {
        error: Some(error.to_string()),
        cause: Some(cause),
    }
}

/// What one state's turn produced.
struct StateStep {
    /// The state's output, which is the next state's input.
    value: Value,
    /// The offset into the run at which the state finished.
    at: Duration,
    next: Next,
}

/// Where the walk goes after a state.
enum Next {
    /// The named state.
    Named(String),
    /// Nowhere: this branch (or the execution) ends with the state's output.
    EndBranch,
}

/// What one walk of a state list produced.
struct Branch {
    outcome: Result<Value, Failure>,
    /// The offset into the run at which the branch ended.
    end: Duration,
}

/// The `$$.Map.Item` of the enclosing `Map` iteration, if the walk is inside one.
#[derive(Debug, Clone, Default)]
struct Scope {
    map_item: Option<Value>,
}

/// A `Map` state's plan: everything resolved from its members before its items run.
struct MapPlan<'a> {
    items: &'a [Value],
    /// The `ItemProcessor`'s (or legacy `Iterator`'s) own state list and entry point.
    states: &'a Map<String, Value>,
    start_at: &'a str,
    /// `ItemSelector`, or the legacy `Parameters` form of it.
    selector: Option<&'a Value>,
    /// Whether the processor declares `"Mode": "DISTRIBUTED"`.
    distributed: bool,
    /// How many items run at once; `0` is `MaxConcurrency`'s documented "no limit".
    concurrency: usize,
    tolerated_count: u64,
    tolerated_percentage: f64,
    /// Whether the state declares any tolerated-failure member at all.
    tolerance_declared: bool,
}

/// A `Parallel` state's branches: each one's state list and entry point.
struct ParallelPlan<'a> {
    branches: Vec<(&'a Map<String, Value>, &'a str)>,
}

/// The diagnostic code `INVALID_JSON_DESCRIPTION`: "JSON syntax problem found".
///
/// The four codes below are the ones this emulator's validation can reach, out of
/// the error-code list the model documents on
/// `ValidateStateMachineDefinitionDiagnostic`
/// (`smithy_query(query="doc",shape="ValidateStateMachineDefinitionDiagnostic")`).
/// That same documentation is why the code is the stable part of an
/// `InvalidDefinition` message and the detail after it is not: "might add new
/// diagnostics in the future, adjust diagnostic codes, or change the message
/// wording. […] Do not rely on the exact order, count, or wording of diagnostic
/// messages."
pub const INVALID_JSON_DESCRIPTION: &str = "INVALID_JSON_DESCRIPTION";

/// `SCHEMA_VALIDATION_FAILED`: "Schema validation reported errors" — the definition
/// is JSON, but not the shape the Amazon States Language declares.
pub const SCHEMA_VALIDATION_FAILED: &str = "SCHEMA_VALIDATION_FAILED";

/// `MISSING_TRANSITION_TARGET`: "The value of \"Next\" field doesn't match a known
/// state name" — reported for every transition target that names no declared state,
/// `StartAt`'s included.
pub const MISSING_TRANSITION_TARGET: &str = "MISSING_TRANSITION_TARGET";

/// `INVALID_STATE_NAME`: "The state name does not follow the naming convention".
pub const INVALID_STATE_NAME: &str = "INVALID_STATE_NAME";

/// One diagnostic as an `InvalidDefinition` message carries it: the code that
/// classifies the problem, then the detail describing it.
fn diagnostic(code: &str, detail: impl ::std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// Why `definition` is not a definition this service accepts, or `Ok` when it is.
///
/// These are the Amazon States Language's own structural rules, so the refusal is
/// `CreateStateMachine`'s modelled `InvalidDefinition`: the definition is a JSON
/// object carrying `StartAt` and a non-empty `States` map, `StartAt` names a declared
/// state, every state is an object with a string `Type`, every `Next` names a
/// declared state, a terminal state carries no `Next`, and a continuing state carries
/// `Next` or `"End": true`.
///
/// The `Err` is the SUBJECT of that error's message — one diagnostic, `<code>:
/// <detail>` — which the operation states through
/// [`crate::state::resource_error_message`] with
/// [`crate::state::INVALID_STATE_MACHINE_DEFINITION`].
///
/// A definition may use states this emulator does not EXECUTE and still be valid,
/// because it is valid ASL: what cannot be run surfaces at execution time, as a
/// `FAILED` execution, not as a rejected create.
///
/// The rules are applied to EVERY state list the definition declares, not only the
/// top-level one: a `Map` state's `ItemProcessor`/`Iterator` and a `Parallel` state's
/// `Branches` each declare their own state machine, whose state names reach the same
/// execution history.
pub fn validate(definition: &str) -> Result<(), String> {
    let document: Value = serde_json::from_str(definition).map_err(|error| {
        diagnostic(
            INVALID_JSON_DESCRIPTION,
            format!("the definition is not valid JSON: {error}"),
        )
    })?;
    validate_machine(&document, 1)
}

/// [`validate`] for one state machine — the definition itself, or one nested inside
/// it — carrying how deeply it is nested.
fn validate_machine(document: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEFINITION_DEPTH {
        return Err(diagnostic(
            SCHEMA_VALIDATION_FAILED,
            format!(
                "the definition nests state machines more than the {MAX_DEFINITION_DEPTH} levels \
                 this service reads"
            ),
        ));
    }
    let document = document.as_object().ok_or_else(|| {
        diagnostic(SCHEMA_VALIDATION_FAILED, "the definition must be a JSON object")
    })?;
    let start_at = document
        .get("StartAt")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            diagnostic(
                SCHEMA_VALIDATION_FAILED,
                "the definition must carry a string 'StartAt' member",
            )
        })?;
    let states = document
        .get("States")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            diagnostic(
                SCHEMA_VALIDATION_FAILED,
                "the definition must carry an object 'States' member",
            )
        })?;
    if states.is_empty() {
        return Err(diagnostic(
            SCHEMA_VALIDATION_FAILED,
            "'States' must declare at least one state",
        ));
    }
    if !states.contains_key(start_at) {
        return Err(diagnostic(
            MISSING_TRANSITION_TARGET,
            format!("'StartAt' names '{start_at}', which 'States' does not declare"),
        ));
    }
    for (name, state) in states {
        // A state name is a caller-chosen JSON object key and lands in every
        // history event of that state, so its LENGTH is bounded here, not only
        // the number of events: this is what keeps a history's bytes bounded.
        if name.chars().count() > MAX_STATE_NAME_CHARS {
            return Err(diagnostic(
                INVALID_STATE_NAME,
                format!(
                    "a state name may carry at most {MAX_STATE_NAME_CHARS} characters, and this \
                     definition declares one of {}",
                    name.chars().count()
                ),
            ));
        }
        let state = state.as_object().ok_or_else(|| {
            diagnostic(
                SCHEMA_VALIDATION_FAILED,
                format!("state '{name}' must be a JSON object"),
            )
        })?;
        let state_type = state.get("Type").and_then(Value::as_str).ok_or_else(|| {
            diagnostic(
                SCHEMA_VALIDATION_FAILED,
                format!("state '{name}' must carry a string 'Type' member"),
            )
        })?;
        // A `Fail` state's `Error` and `Cause` are reported verbatim by
        // `DescribeExecution`, through shapes with their own maxima, so a
        // definition carrying a longer one could never be reported.
        for (member, cap) in [("Error", MAX_ERROR_BYTES), ("Cause", MAX_CAUSE_BYTES)] {
            if let Some(value) = state.get(member).and_then(Value::as_str) {
                if value.len() > cap {
                    return Err(diagnostic(
                        SCHEMA_VALIDATION_FAILED,
                        format!(
                            "state '{name}' carries a '{member}' of {} bytes, more than the {cap} \
                             this service reports",
                            value.len()
                        ),
                    ));
                }
            }
        }
        match state.get("Next") {
            Some(next) => {
                let next = next.as_str().ok_or_else(|| {
                    diagnostic(
                        SCHEMA_VALIDATION_FAILED,
                        format!("state '{name}' has a non-string 'Next' member"),
                    )
                })?;
                if !states.contains_key(next) {
                    return Err(diagnostic(
                        MISSING_TRANSITION_TARGET,
                        format!(
                            "state '{name}' transitions to '{next}', which 'States' does not \
                             declare"
                        ),
                    ));
                }
                if TERMINAL_STATE_TYPES.contains(&state_type) {
                    return Err(diagnostic(
                        SCHEMA_VALIDATION_FAILED,
                        format!(
                            "state '{name}' is a '{state_type}' state, which ends an execution \
                             and must not carry 'Next'"
                        ),
                    ));
                }
            }
            None => {
                if CONTINUING_STATE_TYPES.contains(&state_type)
                    && state.get("End").and_then(Value::as_bool) != Some(true)
                {
                    return Err(diagnostic(
                        SCHEMA_VALIDATION_FAILED,
                        format!(
                            "state '{name}' is a '{state_type}' state, so it must carry either \
                             'Next' or \"End\": true"
                        ),
                    ));
                }
            }
        }
        // A nested state machine's names reach the same history as this one's, so
        // every rule above holds for it too. The diagnostic the nested machine
        // reported is the one the caller sees, prefixed with where it was found.
        for member in ["ItemProcessor", "Iterator"] {
            if let Some(processor) = state.get(member) {
                validate_machine(processor, depth + 1)
                    .map_err(|why| format!("{why} (in the '{member}' of state '{name}')"))?;
            }
        }
        if let Some(branches) = state.get("Branches").and_then(Value::as_array) {
            for (index, branch) in branches.iter().enumerate() {
                validate_machine(branch, depth + 1)
                    .map_err(|why| format!("{why} (in branch {index} of state '{name}')"))?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// What a state REFERENCES
// ---------------------------------------------------------------------------

/// `$` terms that are not variables.
///
/// `$states` is the reserved context object Step Functions exposes to an
/// expression (`$states.input`, `$states.context`, `$states.result`), not a
/// variable a state assigned.
const RESERVED_TERMS: &[&str] = &["states"];

/// The variables each state of `definition` references, keyed by state name.
///
/// `DescribeStateMachine`'s `variableReferences` is "a map of state name to a
/// list of variables referenced by that state. States that do not use variable
/// references will not be shown in the response" — so a state that references
/// none is absent, and a definition whose states reference none answers an empty
/// map, which the caller sees as no member at all.
///
/// A variable reference is a `$name` term of one of the state's own EXPRESSIONS,
/// which is where both of the service's query languages read one:
///
/// * a JSONata expression, which the language delimits with `{%` and `%}` (the
///   `Assign`, `Output`, `Arguments`, `Condition`, … members of a state whose
///   `QueryLanguage` is JSONata);
/// * a JSONPath reference field, which the Amazon States Language spells as a
///   member whose name ends in `.$` and whose value is a path or a reference.
///
/// Nothing else in the state is scanned, so a literal `Result` that happens to
/// carry a `$` is not mistaken for a reference. A nested `States` map — a
/// `Parallel` branch or a `Map` processor — is walked as the states it declares,
/// so each state's references are reported under ITS own name and never folded
/// into its parent's.
///
/// The definition is parsed with `serde_json`, whose own recursion limit bounds
/// how deep the walk below can go; the extraction of the `$name` terms from one
/// expression is lexical, which is why it needs no expression parser (and there
/// is no maintained Rust crate that parses JSONata).
pub fn variable_references(definition: &str) -> BTreeMap<String, Vec<String>> {
    let mut references = BTreeMap::new();
    let Ok(document) = serde_json::from_str::<Value>(definition) else {
        return references;
    };
    if let Some(states) = document.get("States").and_then(Value::as_object) {
        collect_state_references(states, &mut references);
    }
    references
}

/// Report the references of every state `states` declares, and of every state
/// nested inside one of them.
///
/// The Amazon States Language scopes a state name to the machine it is declared
/// in, so a `Parallel` branch may declare a name a top-level state also uses.
/// The map is flat, as the wire shape is, so two states of one name are MERGED
/// rather than one silently replacing the other.
fn collect_state_references(
    states: &Map<String, Value>,
    into: &mut BTreeMap<String, Vec<String>>,
) {
    for (name, state) in states {
        let mut own = BTreeSet::new();
        scan_state(state, false, &mut own, into);
        if own.is_empty() {
            continue;
        }
        let entry = into.entry(name.clone()).or_default();
        own.extend(entry.drain(..));
        entry.extend(own);
    }
}

/// Walk one state, collecting ITS references into `own` and the references of
/// any state nested inside it into `nested`.
///
/// `reference_field` is whether the value being walked is the value of a `.$`
/// member, which is what makes a JSONPath string a reference rather than a
/// literal.
fn scan_state(
    value: &Value,
    reference_field: bool,
    own: &mut BTreeSet<String>,
    nested: &mut BTreeMap<String, Vec<String>>,
) {
    match value {
        Value::String(text) => {
            for expression in jsonata_expressions(text) {
                scan_expression(expression, own);
            }
            if reference_field {
                scan_expression(text, own);
            }
        }
        Value::Array(items) => {
            for item in items {
                scan_state(item, reference_field, own, nested);
            }
        }
        Value::Object(members) => {
            // A nested state machine — a `Parallel` branch, a `Map`'s
            // `ItemProcessor`/`Iterator` — is an object carrying BOTH a string
            // `StartAt` and an object `States`, exactly as the top-level
            // definition does. Requiring the pair is what keeps a literal
            // payload member that happens to be called `States` from being
            // walked as states and injecting names of its own.
            let sub_machine = members
                .get("StartAt")
                .and_then(Value::as_str)
                .is_some_and(|start_at| !start_at.is_empty());
            for (member, child) in members {
                match child.as_object() {
                    Some(states) if member == "States" && sub_machine => {
                        collect_state_references(states, nested);
                    }
                    _ => scan_state(child, member.ends_with(".$"), own, nested),
                }
            }
        }
        _ => {}
    }
}

/// Every JSONata expression `text` carries: the language embeds one between
/// `{%` and `%}`.
fn jsonata_expressions(text: &str) -> Vec<&str> {
    let mut expressions = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{%") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("%}") else {
            break;
        };
        expressions.push(&after[..close]);
        rest = &after[close + 2..];
    }
    expressions
}

/// Every variable one expression references.
///
/// A term is `$` followed by an identifier. `$$` names the context, `$` alone
/// the state's input, `$name(` is a function call and [`RESERVED_TERMS`] names
/// the context object — none of them is a variable.
fn scan_expression(expression: &str, into: &mut BTreeSet<String>) {
    let bytes = expression.as_bytes();
    let mut index = 0;
    while let Some(offset) = expression[index..].find('$') {
        let start = index + offset;
        let mut end = start + 1;
        while bytes
            .get(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            end += 1;
        }
        index = end.max(start + 1);
        let name = &expression[start + 1..end];
        let identifier = name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_');
        if !identifier
            || (start > 0 && bytes[start - 1] == b'$')
            || bytes.get(end) == Some(&b'(')
            || RESERVED_TERMS.contains(&name)
        {
            continue;
        }
        into.insert(name.to_string());
    }
}

/// Walk `definition` from its `StartAt` state with `input` as the first state's
/// input, and report what happened.
pub fn run(definition: &str, input: &str, start: SystemTime, context: &Context) -> Run {
    let document: Value = match serde_json::from_str(definition) {
        Ok(document) => document,
        Err(error) => {
            return failed(
                Vec::new(),
                Duration::ZERO,
                runtime(format!(
                    "the state machine definition is not valid JSON: {error}"
                )),
            );
        }
    };
    let (Some(start_at), Some(states)) = (
        document.get("StartAt").and_then(Value::as_str),
        document.get("States").and_then(Value::as_object),
    ) else {
        return failed(
            Vec::new(),
            Duration::ZERO,
            runtime("the state machine definition carries no 'StartAt' and 'States' pair".to_string()),
        );
    };
    if let Some(language) = document.get("QueryLanguage").and_then(Value::as_str) {
        if language != JSONPATH {
            return failed(
                Vec::new(),
                Duration::ZERO,
                runtime(format!(
                    "the definition is written in '{language}', and this emulator evaluates only \
                     '{JSONPATH}' definitions, so nothing was run for it"
                )),
            );
        }
    }
    let input_value: Value = match serde_json::from_str(input) {
        Ok(value) => value,
        Err(error) => {
            return failed(
                Vec::new(),
                Duration::ZERO,
                runtime(format!("the execution input is not valid JSON: {error}")),
            );
        }
    };

    let mut interpreter = Interpreter {
        budget: MAX_STATE_TRANSITIONS,
        depth: 0,
        // The deadline is measured from NOW, not from the execution's own start: it
        // bounds how long this call spends interpreting, which is what makes the
        // walk interruptible.
        deadline: super::after(super::now(), MAX_INTERPRET_DURATION),
        retry_count: 0,
        steps: Vec::new(),
        start,
        base_context: context.value(&input_value, start),
    };
    let branch = interpreter.walk(
        states,
        start_at,
        input_value.clone(),
        Duration::ZERO,
        &Scope::default(),
    );
    let steps = interpreter.steps;

    // A definition-level `TimeoutSeconds` bounds the whole execution: a run that got
    // past it ended TIMED_OUT at the timeout, whatever it went on to compute.
    if let Some(seconds) = document.get("TimeoutSeconds").and_then(Value::as_u64) {
        let timeout = Duration::from_secs(seconds);
        if branch.end > timeout {
            let kept = steps.into_iter().filter(|step| step.after <= timeout).collect();
            return terminal(
                kept,
                timeout,
                "ExecutionTimedOut",
                super::EXECUTION_STATUS_TIMED_OUT,
                None,
                Some(TIMEOUT_ERROR.to_string()),
                Some(format!(
                    "the execution ran for longer than the {seconds} seconds its definition's \
                     'TimeoutSeconds' allows"
                )),
            );
        }
    }

    match branch.outcome {
        // A state's input and output is a JSON DOCUMENT: when the document the run
        // ended with is the one the caller wrote — no state transformed it — the
        // caller's own BYTES are what the execution reports, rather than a
        // re-encoding of a parsed copy. A document the definition built is reported
        // in the encoding the walk produced it in.
        Ok(value) if value == input_value => succeeded(steps, branch.end, input.to_string()),
        Ok(value) => succeeded(steps, branch.end, value.to_string()),
        Err(failure) => failed(steps, branch.end, failure),
    }
}

/// The run of a definition that ended at a `Succeed` state or at `"End": true`.
fn succeeded(steps: Vec<Step>, elapsed: Duration, output: String) -> Run {
    // A state's output is bounded by what the service can report it through, so a
    // run whose value outgrew that ends with ASL's own data-limit error rather than
    // storing a payload no response could carry.
    if output.len() > MAX_PAYLOAD_BYTES {
        return failed(
            steps,
            elapsed,
            named(
                DATA_LIMIT_ERROR,
                format!(
                    "the execution output is {} bytes, more than the {MAX_PAYLOAD_BYTES} a state's \
                     output may carry",
                    output.len()
                ),
            ),
        );
    }
    terminal(
        steps,
        elapsed,
        "ExecutionSucceeded",
        super::EXECUTION_STATUS_SUCCEEDED,
        Some(output),
        None,
        None,
    )
}

/// The run of a definition that ended at a `Fail` state, or that could not be run.
fn failed(steps: Vec<Step>, elapsed: Duration, failure: Failure) -> Run {
    // `error` and `cause` leave through the modelled `SensitiveError` (0..256) and
    // `SensitiveCause` (0..32768) shapes. A `Fail` state may read its error from the
    // execution's own data, and a cause the interpreter wrote quotes the value it
    // could not handle, so neither is bounded by the definition alone: an error name
    // past its maximum is reported as ASL's own data-limit error, since a name
    // carries meaning and cannot be shortened, while an over-long cause keeps the
    // error name and is clipped to what the service can report.
    let error = failure.error;
    let cause = failure.cause;
    if error.as_ref().is_some_and(|error| error.len() > MAX_ERROR_BYTES) {
        return terminal(
            steps,
            elapsed,
            "ExecutionFailed",
            super::EXECUTION_STATUS_FAILED,
            None,
            Some(DATA_LIMIT_ERROR.to_string()),
            Some(format!(
                "the execution's error name is longer than the {MAX_ERROR_BYTES} bytes this \
                 service reports it through"
            )),
        );
    }
    terminal(
        steps,
        elapsed,
        "ExecutionFailed",
        super::EXECUTION_STATUS_FAILED,
        None,
        error,
        cause.map(clip_cause),
    )
}

/// `cause` within the bytes the modelled `SensitiveCause` carries, cut at a character
/// boundary.
fn clip_cause(cause: String) -> String {
    if cause.len() <= MAX_CAUSE_BYTES {
        return cause;
    }
    let mut end = MAX_CAUSE_BYTES;
    while end > 0 && !cause.is_char_boundary(end) {
        end -= 1;
    }
    cause[..end].to_string()
}

/// One run, with the terminal history event the execution ends on appended.
fn terminal(
    mut steps: Vec<Step>,
    elapsed: Duration,
    event_type: &'static str,
    status: &'static str,
    output: Option<String>,
    error: Option<String>,
    cause: Option<String>,
) -> Run {
    steps.push(Step {
        event_type,
        state_name: None,
        after: elapsed,
        terminal: true,
    });
    Run {
        status,
        output,
        error,
        cause,
        steps,
        elapsed,
    }
}

/// The bytes `value` serialises to, refusing a document past what a state's output
/// may carry or deeper than [`MAX_PAYLOAD_DEPTH`]. `built_by` names what produced it,
/// for the cause the refusal carries.
///
/// Walked ITERATIVELY, over an explicit stack, so measuring a document is not itself
/// bounded by the thread's stack; and it stops at the first byte past the limit, so
/// the measurement of an over-large document costs no more than the limit.
fn measure_payload(value: &Value, built_by: &str) -> Result<usize, Failure> {
    let too_large = |bytes: usize| {
        named(
            DATA_LIMIT_ERROR,
            format!(
                "{built_by} produced a document of at least {bytes} bytes, more than the \
                 {MAX_PAYLOAD_BYTES} a state's output may carry"
            ),
        )
    };
    let mut bytes = 0usize;
    let mut pending: Vec<(&Value, usize)> = vec![(value, 1)];
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_PAYLOAD_DEPTH {
            return Err(named(
                DATA_LIMIT_ERROR,
                format!(
                    "{built_by} produced a document nested more than the {MAX_PAYLOAD_DEPTH} \
                     levels this service carries"
                ),
            ));
        }
        bytes = bytes.saturating_add(match node {
            Value::Null => 4,
            Value::Bool(flag) => {
                // "true" is four bytes and "false" five.
                if *flag {
                    4
                } else {
                    5
                }
            }
            Value::Number(number) => number.to_string().len(),
            Value::String(text) => text.len() + 2,
            Value::Array(items) => {
                for item in items {
                    pending.push((item, depth + 1));
                }
                items.len() + 2
            }
            Value::Object(members) => {
                let mut keys = 0usize;
                for (key, member) in members {
                    keys = keys.saturating_add(key.len() + 3);
                    pending.push((member, depth + 1));
                }
                keys.saturating_add(members.len() + 2)
            }
        });
        if bytes > MAX_PAYLOAD_BYTES {
            return Err(too_large(bytes));
        }
        // The pending list is bounded by the bytes already counted, so it cannot
        // outgrow the limit either.
        if pending.len() > MAX_PAYLOAD_BYTES {
            return Err(too_large(pending.len()));
        }
    }
    Ok(bytes)
}

/// One walk of one definition.
struct Interpreter {
    /// The state transitions still left in [`MAX_STATE_TRANSITIONS`]. Every nested
    /// `Map` iteration and `Parallel` branch spends from the same budget, so the
    /// total work one execution does is bounded however deeply it nests.
    budget: usize,
    /// How deeply the walk is nested inside `Map` iterations and `Parallel` branches,
    /// against [`MAX_NESTING_DEPTH`]: each level is an interpreter frame, so the
    /// nesting a definition can ask for is capped rather than left to the stack.
    depth: usize,
    /// When this interpretation must stop, whatever it is doing: see
    /// [`MAX_INTERPRET_DURATION`].
    deadline: SystemTime,
    /// Which `Retry` attempt the state being run is on, which it reads through
    /// `$$.State.RetryCount`. A nested `Map` iteration or `Parallel` branch runs its
    /// own states, each with its own count, so [`Interpreter::nested_walk`] starts
    /// them at zero.
    retry_count: u64,
    steps: Vec<Step>,
    /// When the execution started, which a `Wait` state's `Timestamp` is measured
    /// against.
    start: SystemTime,
    base_context: Value,
}

impl Interpreter {
    /// Whether this interpretation has run out of wall-clock time.
    ///
    /// The ceilings on steps, payloads, paths and nesting bound each factor of the
    /// work a definition can ask for; this bounds their PRODUCT, and it is what a
    /// run that is simply taking too long ends on.
    fn out_of_time(&self) -> Option<Failure> {
        (super::now() > self.deadline).then(|| {
            runtime(format!(
                "the execution was interpreted for longer than the {} seconds this emulator \
                 interprets one definition for, so it was stopped",
                MAX_INTERPRET_DURATION.as_secs()
            ))
        })
    }

    /// Walk a NESTED state list — a `Map` iteration or a `Parallel` branch — one
    /// level deeper, refusing a definition that nests past [`MAX_NESTING_DEPTH`].
    fn nested_walk(
        &mut self,
        states: &Map<String, Value>,
        start_at: &str,
        input: Value,
        at: Duration,
        scope: &Scope,
    ) -> Branch {
        if self.depth >= MAX_NESTING_DEPTH {
            return Branch {
                outcome: Err(runtime(format!(
                    "the definition nests 'Map' iterations and 'Parallel' branches more than the \
                     {MAX_NESTING_DEPTH} levels deep this emulator runs, so nothing was run for it"
                ))),
                end: at,
            };
        }
        self.depth += 1;
        // A nested machine's states carry their OWN retry counts; the enclosing
        // state's is not theirs.
        let enclosing = std::mem::take(&mut self.retry_count);
        let branch = self.walk(states, start_at, input, at, scope);
        self.retry_count = enclosing;
        self.depth -= 1;
        branch
    }

    /// Walk `states` from `start_at`, with `input` as the first state's input and
    /// `at` as the offset into the run at which this branch begins.
    fn walk(
        &mut self,
        states: &Map<String, Value>,
        start_at: &str,
        input: Value,
        at: Duration,
        scope: &Scope,
    ) -> Branch {
        let mut name = start_at.to_string();
        let mut current = input;
        let mut at = at;
        loop {
            if let Some(failure) = self.out_of_time() {
                return Branch {
                    outcome: Err(failure),
                    end: at,
                };
            }
            if self.budget == 0 {
                return Branch {
                    outcome: Err(runtime(format!(
                        "the execution made more than the {MAX_STATE_TRANSITIONS} state \
                         transitions this emulator runs, so it was stopped"
                    ))),
                    end: at,
                };
            }
            self.budget -= 1;
            let Some(state) = states.get(&name).and_then(Value::as_object) else {
                return Branch {
                    outcome: Err(runtime(format!(
                        "state '{name}' is not declared by the state machine"
                    ))),
                    end: at,
                };
            };
            let step = match self.step(&name, state, current, at, scope) {
                Ok(step) => step,
                Err(fault) => {
                    return Branch {
                        outcome: Err(fault.failure),
                        end: fault.at,
                    };
                }
            };
            at = step.at;
            current = step.value;
            // The document this state produced is measured HERE, at the state that
            // produced it: a definition that grows what it carries is stopped at the
            // first state past what a state's output may hold, rather than after the
            // whole run.
            if let Err(failure) = measure_payload(&current, &format!("state '{name}'")) {
                return Branch {
                    outcome: Err(failure),
                    end: at,
                };
            }
            match step.next {
                Next::EndBranch => {
                    return Branch {
                        outcome: Ok(current),
                        end: at,
                    };
                }
                Next::Named(next) => name = next,
            }
        }
    }

    /// Run one state: `raw` is its input, `at` the offset it is entered at.
    fn step(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: Value,
        at: Duration,
        scope: &Scope,
    ) -> Result<StateStep, Fault> {
        let state_type = state.get("Type").and_then(Value::as_str).unwrap_or_default();
        // A state's name is a caller-chosen object key that lands in every history
        // event of that state. `validate` refuses a longer one at create time; this
        // is the same bound made structural, so no name the history retains can be
        // longer than the Amazon States Language allows.
        if name.chars().count() > MAX_STATE_NAME_CHARS {
            return Err(runtime(format!(
                "a state name may carry at most {MAX_STATE_NAME_CHARS} characters, and this \
                 definition declares one of {}",
                name.chars().count()
            ))
            .at(at));
        }
        if !EXECUTED_STATE_TYPES.contains(&state_type) {
            return Err(runtime(format!(
                "state '{name}' is of type '{state_type}', which this emulator does not execute, \
                 so nothing was run for it"
            ))
            .at(at));
        }
        if let Some(member) = unexecutable_member(state_type, state) {
            return Err(runtime(format!(
                "state '{name}' carries the member '{member}', which this emulator does not \
                 interpret, so nothing was run for it"
            ))
            .at(at));
        }
        if let Some(language) = state.get("QueryLanguage").and_then(Value::as_str) {
            if language != JSONPATH {
                return Err(runtime(format!(
                    "state '{name}' is written in '{language}', and this emulator evaluates only \
                     '{JSONPATH}' states, so nothing was run for it"
                ))
                .at(at));
            }
        }

        let context = self.context_for(scope, name, at);
        let effective = self
            .input_value(state, &raw, &context)
            .map_err(|failure| failure.clone().at(at))?;

        match state_type {
            "Pass" => {
                self.push("PassStateEntered", name, at);
                // "Parameters ... Used to create a collection of key-value pairs
                // that will be passed as input": it transforms the state's effective
                // INPUT. `Result` is the state's RESULT, so a state that declares
                // both reports its `Result`, computed over the input `Parameters`
                // built. With neither, the result is the effective input unchanged.
                let payload = match state.get("Parameters") {
                    Some(template) => payload_template(template, &effective, &context)
                        .map_err(|failure| failure.at(at))?,
                    None => effective,
                };
                let result = state.get("Result").cloned().unwrap_or(payload);
                let value = self
                    .place_result(state, &raw, result, &context, false)
                    .map_err(|failure| failure.at(at))?;
                self.push("PassStateExited", name, at);
                Ok(StateStep {
                    value,
                    at,
                    next: self.next_of(name, state).map_err(|failure| failure.at(at))?,
                })
            }
            "Wait" => {
                self.push("WaitStateEntered", name, at);
                let waited = self
                    .wait_duration(state, &effective, &context, at)
                    .map_err(|why| runtime(format!("state '{name}': {why}")).at(at))?;
                let at = at + waited;
                let value = self
                    .output_value(state, effective, &context)
                    .map_err(|failure| failure.at(at))?;
                self.push("WaitStateExited", name, at);
                Ok(StateStep {
                    value,
                    at,
                    next: self.next_of(name, state).map_err(|failure| failure.at(at))?,
                })
            }
            "Choice" => {
                self.push("ChoiceStateEntered", name, at);
                let next = self
                    .choose(state, &effective, &context)
                    .map_err(|failure| failure.at(at))?;
                let value = self
                    .output_value(state, effective, &context)
                    .map_err(|failure| failure.at(at))?;
                self.push("ChoiceStateExited", name, at);
                Ok(StateStep {
                    value,
                    at,
                    next: Next::Named(next),
                })
            }
            "Succeed" => {
                self.push("SucceedStateEntered", name, at);
                let value = self
                    .output_value(state, effective, &context)
                    .map_err(|failure| failure.at(at))?;
                self.push("SucceedStateExited", name, at);
                Ok(StateStep {
                    value,
                    at,
                    next: Next::EndBranch,
                })
            }
            "Fail" => {
                self.push("FailStateEntered", name, at);
                // A `Fail` state ends the execution with the error and cause it
                // declares, both of which ASL leaves optional and either of which
                // may instead be read from the state's input through a `*Path`.
                let error = self
                    .fail_member(state, "Error", "ErrorPath", &effective, &context)
                    .map_err(|failure| failure.at(at))?;
                let cause = self
                    .fail_member(state, "Cause", "CausePath", &effective, &context)
                    .map_err(|failure| failure.at(at))?;
                Err(Failure { error, cause }.at(at))
            }
            "Map" => self.map_state(name, state, raw, effective, &context, at),
            // `Parallel` is the only type left: the list above is exhaustive.
            _ => self.parallel_state(name, state, raw, effective, &context, at),
        }
    }

    /// The context object (`$$`) a state reads: the run's own members, plus the state
    /// it is being read in and the `Map` item being iterated, when there is one.
    fn context_for(&self, scope: &Scope, name: &str, at: Duration) -> Value {
        let mut context = self.base_context.clone();
        if let Some(object) = context.as_object_mut() {
            object.insert(
                "State".to_string(),
                json!({
                    "Name": name,
                    "EnteredTime": iso(self.start + at),
                    "RetryCount": self.retry_count,
                }),
            );
            if let Some(item) = &scope.map_item {
                object.insert("Map".to_string(), json!({ "Item": item }));
            }
        }
        context
    }

    fn push(&mut self, event_type: &'static str, state_name: &str, after: Duration) {
        self.steps.push(Step {
            event_type,
            state_name: Some(state_name.to_string()),
            after,
            terminal: false,
        });
    }

    /// Where the walk goes after a state that carries `Next` or `"End": true`.
    fn next_of(&self, name: &str, state: &Map<String, Value>) -> Result<Next, Failure> {
        if state.get("End").and_then(Value::as_bool) == Some(true) {
            return Ok(Next::EndBranch);
        }
        match state.get("Next").and_then(Value::as_str) {
            Some(next) => Ok(Next::Named(next.to_string())),
            None => Err(runtime(format!(
                "state '{name}' carries neither 'Next' nor \"End\": true"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Data flow: InputPath -> Parameters -> work -> ResultSelector -> ResultPath
    //            -> OutputPath
    // -----------------------------------------------------------------------

    /// The state's EFFECTIVE input: `InputPath` applied to the raw input.
    fn input_value(
        &self,
        state: &Map<String, Value>,
        raw: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        match state.get("InputPath") {
            None => Ok(raw.clone()),
            // "If the value of InputPath is null, that means that the raw input is
            // discarded, and the effective input for the state is an empty JSON
            // object, {}."
            Some(Value::Null) => Ok(json!({})),
            Some(Value::String(path)) => select(path, raw, context),
            Some(other) => Err(runtime(format!(
                "'InputPath' must be a JSONPath string or null, not {other}"
            ))),
        }
    }

    /// `OutputPath` applied to a state's result.
    fn output_value(
        &self,
        state: &Map<String, Value>,
        value: Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        match state.get("OutputPath") {
            None => Ok(value),
            // "If the value of OutputPath is null, that means the input and result
            // are discarded, and the effective output from the state is an empty
            // JSON object, {}."
            Some(Value::Null) => Ok(json!({})),
            Some(Value::String(path)) => select(path, &value, context),
            Some(other) => Err(runtime(format!(
                "'OutputPath' must be a JSONPath string or null, not {other}"
            ))),
        }
    }

    /// `ResultSelector`, then `ResultPath` against the state's RAW input, then
    /// `OutputPath`: the three steps that turn a state's result into its output.
    fn place_result(
        &self,
        state: &Map<String, Value>,
        raw: &Value,
        result: Value,
        context: &Value,
        with_selector: bool,
    ) -> Result<Value, Failure> {
        let result = match state.get("ResultSelector").filter(|_| with_selector) {
            Some(template) => payload_template(template, &result, context)?,
            None => result,
        };
        let placed = match state.get("ResultPath") {
            // "ResultPath (Optional) ... The default value is $, which matches the
            // entire input."
            None => result,
            // "If the ResultPath is null, that means that the state's own raw input
            // is the state's output."
            Some(Value::Null) => raw.clone(),
            Some(Value::String(path)) => set_at_reference_path(raw, path, result)?,
            Some(other) => {
                return Err(runtime(format!(
                    "'ResultPath' must be a reference-path string or null, not {other}"
                )));
            }
        };
        self.output_value(state, placed, context)
    }

    /// A `Fail` state's `Error`/`Cause`, from the literal member or from its `*Path`.
    fn fail_member(
        &self,
        state: &Map<String, Value>,
        member: &str,
        path_member: &str,
        input: &Value,
        context: &Value,
    ) -> Result<Option<String>, Failure> {
        if let Some(literal) = state.get(member) {
            let text = literal.as_str().ok_or_else(|| {
                runtime(format!("a 'Fail' state's '{member}' must be a string"))
            })?;
            return Ok(Some(text.to_string()));
        }
        match state.get(path_member) {
            None => Ok(None),
            Some(Value::String(path)) => {
                let value = expression(path, input, context)?;
                let text = value.as_str().map(str::to_string).ok_or_else(|| {
                    runtime(format!(
                        "'{path_member}' selected {value}, and a 'Fail' state's '{member}' must \
                         be a string"
                    ))
                })?;
                Ok(Some(text))
            }
            Some(other) => Err(runtime(format!(
                "'{path_member}' must be a JSONPath string, not {other}"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Wait
    // -----------------------------------------------------------------------

    /// How long a `Wait` state waits.
    ///
    /// `Seconds` is the caller's own figure and keeps its value; `Timestamp` is an
    /// instant, so the wait is however much of it is still ahead of the run. Each has
    /// a `*Path` form that reads the figure from the state's input instead.
    fn wait_duration(
        &self,
        state: &Map<String, Value>,
        input: &Value,
        context: &Value,
        at: Duration,
    ) -> Result<Duration, String> {
        let seconds = match (state.get("Seconds"), state.get("SecondsPath")) {
            (Some(seconds), _) => Some(seconds.clone()),
            (None, Some(Value::String(path))) => Some(
                expression(path, input, context)
                    .map_err(|failure| failure.cause.unwrap_or_default())?,
            ),
            (None, Some(other)) => {
                return Err(format!("'SecondsPath' must be a JSONPath string, not {other}"));
            }
            (None, None) => None,
        };
        if let Some(seconds) = seconds {
            let seconds = seconds
                .as_u64()
                .ok_or_else(|| "'Seconds' must be a non-negative whole number".to_string())?;
            if seconds > MAX_WAIT_SECONDS {
                return Err(format!(
                    "'Seconds' is {seconds}, more than the {MAX_WAIT_SECONDS} seconds a 'Wait' \
                     state may wait for"
                ));
            }
            return Ok(Duration::from_secs(seconds));
        }

        let timestamp = match (state.get("Timestamp"), state.get("TimestampPath")) {
            (Some(timestamp), _) => Some(timestamp.clone()),
            (None, Some(Value::String(path))) => Some(
                expression(path, input, context)
                    .map_err(|failure| failure.cause.unwrap_or_default())?,
            ),
            (None, Some(other)) => {
                return Err(format!(
                    "'TimestampPath' must be a JSONPath string, not {other}"
                ));
            }
            (None, None) => None,
        };
        if let Some(timestamp) = timestamp {
            let text = timestamp
                .as_str()
                .ok_or_else(|| "'Timestamp' must be an ISO 8601 string".to_string())?;
            let due_millis = iso_millis(text)?;
            let reached = super::epoch_millis(self.start)
                .saturating_add(i64::try_from(at.as_millis()).unwrap_or(i64::MAX));
            // A `Timestamp` already behind the run is no wait at all; one further
            // ahead than a `Wait` may wait for is out of range, like an over-maximum
            // `Seconds`, and is NOT silently shortened to the maximum.
            let remaining = u64::try_from(due_millis.saturating_sub(reached)).unwrap_or(0);
            if remaining > MAX_WAIT_SECONDS.saturating_mul(1_000) {
                return Err(format!(
                    "'Timestamp' is {text}, further ahead than the {MAX_WAIT_SECONDS} seconds a \
                     'Wait' state may wait for"
                ));
            }
            return Ok(Duration::from_millis(remaining));
        }
        Err("a 'Wait' state must carry 'Seconds' or 'Timestamp'".to_string())
    }

    // -----------------------------------------------------------------------
    // Choice
    // -----------------------------------------------------------------------

    /// The state the first matching choice rule names, or the `Default`.
    fn choose(
        &self,
        state: &Map<String, Value>,
        input: &Value,
        context: &Value,
    ) -> Result<String, Failure> {
        let choices = state
            .get("Choices")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                runtime("a 'Choice' state must carry a non-empty 'Choices' array".to_string())
            })?;
        if choices.is_empty() {
            return Err(runtime(
                "a 'Choice' state must carry a non-empty 'Choices' array".to_string(),
            ));
        }
        // Every rule is evaluated until one matches, so the rule count multiplies the
        // paths one visit to this state evaluates.
        if choices.len() > MAX_CHOICE_RULES {
            return Err(runtime(format!(
                "a 'Choice' state carries {} rules, more than the {MAX_CHOICE_RULES} this service \
                 evaluates",
                choices.len()
            )));
        }
        for rule in choices {
            let rule = rule.as_object().ok_or_else(|| {
                runtime("every member of 'Choices' must be a JSON object".to_string())
            })?;
            if condition_matches(rule, input, context)? {
                let next = rule.get("Next").and_then(Value::as_str).ok_or_else(|| {
                    runtime("a top-level choice rule must carry a string 'Next'".to_string())
                })?;
                return Ok(next.to_string());
            }
        }
        match state.get("Default").and_then(Value::as_str) {
            Some(default) => Ok(default.to_string()),
            None => Err(named(
                NO_CHOICE_ERROR,
                "no choice rule matched the state's input and the state declares no 'Default'"
                    .to_string(),
            )),
        }
    }

    // -----------------------------------------------------------------------
    // Map
    // -----------------------------------------------------------------------

    fn map_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: Value,
        effective: Value,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        self.push("MapStateEntered", name, at);
        let items = self
            .map_items(name, state, &effective, context)
            .map_err(|failure| failure.at(at))?;
        let (processor, distributed) = map_processor(name, state).map_err(|failure| failure.at(at))?;
        let (processor_states, processor_start) =
            states_of(processor, &format!("the 'ItemProcessor' of state '{name}'"))
                .map_err(|failure| failure.at(at))?;
        let plan = MapPlan {
            items: &items,
            states: processor_states,
            start_at: processor_start,
            selector: state.get("ItemSelector").or_else(|| state.get("Parameters")),
            distributed,
            concurrency: self
                .number_member(state, "MaxConcurrency", &effective, context, 0.0)
                .map_err(|failure| failure.at(at))? as usize,
            tolerated_count: self
                .number_member(state, "ToleratedFailureCount", &effective, context, 0.0)
                .map_err(|failure| failure.at(at))? as u64,
            tolerated_percentage: self
                .number_member(state, "ToleratedFailurePercentage", &effective, context, 0.0)
                .map_err(|failure| failure.at(at))?,
            tolerance_declared: ["ToleratedFailureCount", "ToleratedFailureCountPath",
                "ToleratedFailurePercentage", "ToleratedFailurePercentagePath"]
                .iter()
                .any(|member| state.contains_key(*member)),
        };

        let (outcome, end) = self.with_retries(state, at, &mut |me, at| {
            // This attempt's own members read `$$.State.RetryCount`, so they are
            // evaluated with the context of the attempt they are on.
            let context = with_retry_count(context, me.retry_count);
            me.run_map_items(&plan, &effective, &context, at)
        });
        match outcome {
            Ok(results) => {
                let value = self
                    .place_result(state, &raw, results, context, true)
                    .map_err(|failure| failure.at(end))?;
                self.push("MapStateExited", name, end);
                Ok(StateStep {
                    value,
                    at: end,
                    next: self
                        .next_of(name, state)
                        .map_err(|failure| failure.at(end))?,
                })
            }
            Err(failure) => {
                self.catch_or_fail(
                    (name, "MapStateExited"),
                    state,
                    &raw,
                    failure,
                    context,
                    end,
                )
            }
        }
    }

    /// The items a `Map` state iterates: `Items` when it declares one, else what
    /// `ItemsPath` (default `$`) selects from the state's effective input.
    fn map_items(
        &self,
        name: &str,
        state: &Map<String, Value>,
        effective: &Value,
        context: &Value,
    ) -> Result<Vec<Value>, Failure> {
        let selected = match state.get("Items") {
            Some(items) => payload_template(items, effective, context)?,
            None => match state.get("ItemsPath") {
                None => effective.clone(),
                Some(Value::String(path)) => select(path, effective, context)?,
                Some(other) => {
                    return Err(runtime(format!(
                        "'ItemsPath' must be a reference-path string, not {other}"
                    )));
                }
            },
        };
        match selected {
            Value::Array(items) => Ok(items),
            other => Err(runtime(format!(
                "the 'Map' state '{name}' was given {other}, which is not the array of items a \
                 'Map' state iterates"
            ))),
        }
    }

    /// Run every item of a `Map` state and collect what the iterations produced.
    ///
    /// Items run concurrently on the real service, in batches of `MaxConcurrency`, so
    /// a batch's span is its longest item's rather than the sum: that is what the
    /// returned offset reports. The ITEMS themselves are interpreted one after the
    /// other, because the interpreter is a single walk — the result is the same, and
    /// no iteration can observe another's.
    fn run_map_items(
        &mut self,
        plan: &MapPlan<'_>,
        effective: &Value,
        context: &Value,
        at: Duration,
    ) -> Attempt {
        let batch = if plan.concurrency == 0 {
            plan.items.len().max(1)
        } else {
            plan.concurrency
        };
        let mut results: Vec<Value> = Vec::new();
        let mut results_bytes = 0usize;
        let mut failures: u64 = 0;
        let mut first_failure: Option<Failure> = None;
        let mut at = at;
        let mut index = 0usize;
        for chunk in plan.items.chunks(batch) {
            let batch_start = at;
            let mut batch_end = at;
            for item in chunk {
                // Every item's turn is charged to the step budget BEFORE the work it
                // needs is done, so the per-item work (the context the iteration
                // reads, the `ItemSelector` it is given) stops with the budget rather
                // than being done for every remaining item of a long array.
                if let Some(failure) = self.out_of_time() {
                    return (Err(failure), batch_end);
                }
                if self.budget == 0 {
                    return (
                        Err(runtime(format!(
                            "the execution made more than the {MAX_STATE_TRANSITIONS} state \
                             transitions this emulator runs, so it was stopped"
                        ))),
                        batch_end,
                    );
                }
                // `$$.Map.Item` is what the iteration reads the item and its
                // position through, both in the `ItemSelector` evaluated here and in
                // every state of the iteration.
                let map_item = json!({ "Index": index, "Value": item });
                let item_context = with_map_item(context, &map_item);
                let scope = Scope {
                    map_item: Some(map_item),
                };
                let item_input = match plan.selector {
                    Some(template) => match payload_template(template, effective, &item_context) {
                        Ok(value) => value,
                        Err(failure) => return (Err(failure), batch_end),
                    },
                    None => item.clone(),
                };
                let branch = self.nested_walk(
                    plan.states,
                    plan.start_at,
                    item_input,
                    batch_start,
                    &scope,
                );
                batch_end = batch_end.max(branch.end);
                match branch.outcome {
                    Ok(value) => {
                        // The array the iterations are building is a document this
                        // state will put out, so it is bounded as it grows rather
                        // than only once it is whole.
                        match measure_payload(&value, "a 'Map' iteration") {
                            Ok(bytes) => results_bytes = results_bytes.saturating_add(bytes),
                            Err(failure) => return (Err(failure), batch_end),
                        }
                        if results_bytes > MAX_PAYLOAD_BYTES {
                            return (
                                Err(named(
                                    DATA_LIMIT_ERROR,
                                    format!(
                                        "the 'Map' state's iterations produced at least \
                                         {results_bytes} bytes, more than the \
                                         {MAX_PAYLOAD_BYTES} a state's output may carry"
                                    ),
                                )),
                                batch_end,
                            );
                        }
                        results.push(value);
                    }
                    Err(failure) => {
                        failures += 1;
                        if first_failure.is_none() {
                            first_failure = Some(failure);
                        }
                    }
                }
                index += 1;
            }
            at = batch_end;
        }

        if failures == 0 {
            return (Ok(Value::Array(results)), at);
        }
        let total = plan.items.len() as f64;
        let within_count = failures <= plan.tolerated_count;
        let within_percentage = plan.tolerated_percentage > 0.0
            && (failures as f64) * 100.0 <= plan.tolerated_percentage * total;
        if within_count || within_percentage {
            return (Ok(Value::Array(results)), at);
        }
        // A Distributed Map reports its own tolerated-failure error; an inline Map
        // has no threshold to exceed, so the iteration's own failure is the state's.
        let failure = if plan.distributed || plan.tolerance_declared {
            named(
                TOLERATED_FAILURE_ERROR,
                format!(
                    "{failures} of {} items failed, more than this 'Map' state tolerates",
                    plan.items.len()
                ),
            )
        } else {
            first_failure.unwrap_or_else(|| runtime("a 'Map' iteration failed".to_string()))
        };
        (Err(failure), at)
    }

    // -----------------------------------------------------------------------
    // Parallel
    // -----------------------------------------------------------------------

    fn parallel_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: Value,
        effective: Value,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        self.push("ParallelStateEntered", name, at);
        let branches = state
            .get("Branches")
            .and_then(Value::as_array)
            .filter(|branches| !branches.is_empty())
            .ok_or_else(|| {
                runtime(format!(
                    "the 'Parallel' state '{name}' must carry a non-empty 'Branches' array"
                ))
                .at(at)
            })?;
        // Every branch is run on every visit to this state, so the branch count
        // multiplies that visit's work exactly as a `Choice`'s rule count does.
        if branches.len() > MAX_CHOICE_RULES {
            return Err(runtime(format!(
                "the 'Parallel' state '{name}' declares {} branches, more than the \
                 {MAX_CHOICE_RULES} this service runs",
                branches.len()
            ))
            .at(at));
        }
        let mut plan = ParallelPlan {
            branches: Vec::with_capacity(branches.len()),
        };
        for (index, branch) in branches.iter().enumerate() {
            let label = format!("branch {index} of the 'Parallel' state '{name}'");
            plan.branches
                .push(states_of(branch, &label).map_err(|failure| failure.at(at))?);
        }
        let (outcome, end) = self.with_retries(state, at, &mut |me, at| {
            // "Parameters ... The effective input each branch receives." It is
            // re-evaluated per attempt, because this attempt's own members read
            // `$$.State.RetryCount`.
            let context = with_retry_count(context, me.retry_count);
            let branch_input = match state.get("Parameters") {
                Some(template) => match payload_template(template, &effective, &context) {
                    Ok(value) => value,
                    Err(failure) => return (Err(failure), at),
                },
                None => effective.clone(),
            };
            me.run_branches(&plan, &branch_input, at)
        });
        match outcome {
            Ok(results) => {
                let value = self
                    .place_result(state, &raw, results, context, true)
                    .map_err(|failure| failure.at(end))?;
                self.push("ParallelStateExited", name, end);
                Ok(StateStep {
                    value,
                    at: end,
                    next: self
                        .next_of(name, state)
                        .map_err(|failure| failure.at(end))?,
                })
            }
            Err(failure) => self.catch_or_fail(
                (name, "ParallelStateExited"),
                state,
                &raw,
                failure,
                context,
                end,
            ),
        }
    }

    /// Run every branch of a `Parallel` state with the same input, and collect their
    /// outputs in branch order. The branches run concurrently on the real service, so
    /// the state's span is the longest branch's.
    ///
    /// The array the branches are building is a document this state will put out, so
    /// it is bounded as it GROWS rather than only once it is whole: a definition that
    /// declares many branches cannot hold more than a state's output may carry while
    /// it runs.
    fn run_branches(
        &mut self,
        plan: &ParallelPlan<'_>,
        input: &Value,
        at: Duration,
    ) -> Attempt {
        let mut results = Vec::with_capacity(plan.branches.len());
        let mut results_bytes = 0usize;
        let mut end = at;
        for (states, start_at) in &plan.branches {
            let branch = self.nested_walk(states, start_at, input.clone(), at, &Scope::default());
            end = end.max(branch.end);
            match branch.outcome {
                Ok(value) => {
                    match measure_payload(&value, "a 'Parallel' branch") {
                        Ok(bytes) => results_bytes = results_bytes.saturating_add(bytes),
                        Err(failure) => return (Err(failure), end),
                    }
                    if results_bytes > MAX_PAYLOAD_BYTES {
                        return (
                            Err(named(
                                DATA_LIMIT_ERROR,
                                format!(
                                    "the 'Parallel' state's branches produced at least \
                                     {results_bytes} bytes, more than the {MAX_PAYLOAD_BYTES} a \
                                     state's output may carry"
                                ),
                            )),
                            end,
                        );
                    }
                    results.push(value);
                }
                // "If any branch fails ... the entire Parallel state is considered to
                // have failed", with that branch's error.
                Err(failure) => return (Err(failure), end),
            }
        }
        (Ok(Value::Array(results)), end)
    }

    // -----------------------------------------------------------------------
    // Retry and Catch
    // -----------------------------------------------------------------------

    /// Run `work`, re-running it while one of the state's `Retry` retriers matches
    /// what it failed with and has attempts left.
    ///
    /// The work is really RE-RUN, and the interval each retrier declares really
    /// lengthens the run, so a definition that retries reports an outcome that came
    /// due later than one that does not. The attempt the work is on is what the
    /// retried state reads through `$$.State.RetryCount`.
    fn with_retries(
        &mut self,
        state: &Map<String, Value>,
        at: Duration,
        work: &mut dyn FnMut(&mut Self, Duration) -> Attempt,
    ) -> Attempt {
        let enclosing = self.retry_count;
        let answer = self.retry_loop(state, at, work);
        self.retry_count = enclosing;
        answer
    }

    /// [`Interpreter::with_retries`]' loop, which leaves the attempt count for its
    /// caller to restore.
    fn retry_loop(
        &mut self,
        state: &Map<String, Value>,
        at: Duration,
        work: &mut dyn FnMut(&mut Self, Duration) -> Attempt,
    ) -> Attempt {
        let mut at = at;
        let mut attempts: Vec<u64> = state
            .get("Retry")
            .and_then(Value::as_array)
            .map(|retriers| vec![0; retriers.len()])
            .unwrap_or_default();
        self.retry_count = 0;
        loop {
            let (outcome, end) = work(self, at);
            at = end;
            let Err(failure) = outcome else {
                return (outcome, at);
            };
            let Some(retriers) = state.get("Retry").and_then(Value::as_array) else {
                return (Err(failure), at);
            };
            let mut retried = false;
            for (index, retrier) in retriers.iter().enumerate() {
                let Some(retrier) = retrier.as_object() else {
                    continue;
                };
                if !errors_match(retrier, failure.name()) {
                    continue;
                }
                let max_attempts = retrier
                    .get("MaxAttempts")
                    .and_then(Value::as_u64)
                    .unwrap_or(DEFAULT_RETRY_MAX_ATTEMPTS);
                let taken = attempts.get(index).copied().unwrap_or(0);
                if taken >= max_attempts {
                    break;
                }
                let interval = finite_seconds(retrier, "IntervalSeconds")
                    .unwrap_or(DEFAULT_RETRY_INTERVAL_SECONDS);
                let backoff =
                    finite_seconds(retrier, "BackoffRate").unwrap_or(DEFAULT_RETRY_BACKOFF_RATE);
                let mut delay = interval * backoff.powi(i32::try_from(taken).unwrap_or(0));
                if let Some(cap) = finite_seconds(retrier, "MaxDelaySeconds") {
                    delay = delay.min(cap);
                }
                // `delay` is a product of two numbers the definition chose, so it can
                // overflow to infinity (or to NaN, when a zero interval meets an
                // infinite backoff). A wait no longer than the longest one this
                // service waits for is what such a retrier gets: the conversion below
                // is total only for a finite, in-range value.
                let delay = if delay.is_finite() {
                    delay.clamp(0.0, MAX_WAIT_SECONDS as f64)
                } else {
                    MAX_WAIT_SECONDS as f64
                };
                at += Duration::from_secs_f64(delay);
                if let Some(slot) = attempts.get_mut(index) {
                    *slot = taken + 1;
                }
                retried = true;
                break;
            }
            if !retried {
                return (Err(failure), at);
            }
            // A retry re-runs the work, so it spends from the same step budget: a
            // definition cannot ask for an unbounded number of attempts.
            if self.budget == 0 {
                return (Err(failure), at);
            }
            self.budget -= 1;
            self.retry_count += 1;
        }
    }

    /// The state the first matching `Catch` catcher names, or the failure itself when
    /// none does.
    ///
    /// `left` is the event the state records when a catcher takes over, so a caught
    /// state's history pairs the event it was entered with.
    fn catch_or_fail(
        &mut self,
        caught: (&str, &'static str),
        state: &Map<String, Value>,
        raw: &Value,
        failure: Failure,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        let (name, left) = caught;
        let catchers = state
            .get("Catch")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for catcher in catchers {
            let Some(catcher) = catcher.as_object() else {
                continue;
            };
            if !errors_match(catcher, failure.name()) {
                continue;
            }
            let Some(next) = catcher.get("Next").and_then(Value::as_str) else {
                return Err(
                    runtime("a 'Catch' catcher must carry a string 'Next'".to_string()).at(at),
                );
            };
            // "When a state reports an error and it matches a catcher, the state's
            // output is the error output", placed through the catcher's own
            // `ResultPath`.
            let value = self
                .place_result(catcher, raw, failure.payload(), context, true)
                .map_err(|failure| failure.at(at))?;
            self.push(left, name, at);
            return Ok(StateStep {
                value,
                at,
                next: Next::Named(next.to_string()),
            });
        }
        Err(failure.at(at))
    }

    /// A numeric member of a state, from the literal member or from its `*Path` form.
    fn number_member(
        &self,
        state: &Map<String, Value>,
        member: &str,
        input: &Value,
        context: &Value,
        default: f64,
    ) -> Result<f64, Failure> {
        let path_member = format!("{member}Path");
        let value = match (state.get(member), state.get(&path_member)) {
            (Some(value), _) => value.clone(),
            (None, Some(Value::String(path))) => expression(path, input, context)?,
            (None, Some(other)) => {
                return Err(runtime(format!(
                    "'{path_member}' must be a JSONPath string, not {other}"
                )));
            }
            (None, None) => return Ok(default),
        };
        value.as_f64().filter(|number| *number >= 0.0).ok_or_else(|| {
            runtime(format!(
                "'{member}' must be a non-negative number, not {value}"
            ))
        })
    }
}

/// The `states` map and `StartAt` of a nested state machine (an `ItemProcessor`, an
/// `Iterator` or a `Parallel` branch).
fn states_of<'a>(node: &'a Value, label: &str) -> Result<(&'a Map<String, Value>, &'a str), Failure> {
    let object = node
        .as_object()
        .ok_or_else(|| runtime(format!("{label} must be a JSON object")))?;
    let states = object
        .get("States")
        .and_then(Value::as_object)
        .ok_or_else(|| runtime(format!("{label} must carry an object 'States' member")))?;
    let start_at = object
        .get("StartAt")
        .and_then(Value::as_str)
        .ok_or_else(|| runtime(format!("{label} must carry a string 'StartAt' member")))?;
    Ok((states, start_at))
}

/// A `Map` state's `ItemProcessor` (or the legacy `Iterator`), and whether it runs in
/// Distributed mode.
fn map_processor<'a>(
    name: &str,
    state: &'a Map<String, Value>,
) -> Result<(&'a Value, bool), Failure> {
    let processor = state
        .get("ItemProcessor")
        .or_else(|| state.get("Iterator"))
        .ok_or_else(|| {
            runtime(format!(
                "the 'Map' state '{name}' carries neither 'ItemProcessor' nor 'Iterator'"
            ))
        })?;
    let distributed = processor
        .get("ProcessorConfig")
        .and_then(|config| config.get("Mode"))
        .and_then(Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("DISTRIBUTED"));
    Ok((processor, distributed))
}

/// A retrier's numeric member, when it is a finite, non-negative number.
///
/// A member that is absent, is not a number, or is negative or non-finite leaves the
/// retrier on its documented default rather than reaching the arithmetic below.
fn finite_seconds(retrier: &Map<String, Value>, member: &str) -> Option<f64> {
    retrier
        .get(member)
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && *number >= 0.0)
}

/// Whether a retrier's or catcher's `ErrorEquals` names `error`.
///
/// "States.ALL — A wildcard that matches any known error name", with one documented
/// exception: "A States.Runtime error isn't retriable, and will always cause the
/// execution to fail. A retry or catch on States.ALL won't catch States.Runtime
/// errors." A definition that names `States.Runtime` itself does not catch it
/// either, because it is not an error the execution can recover from.
fn errors_match(entry: &Map<String, Value>, error: &str) -> bool {
    if error == RUNTIME_ERROR {
        return false;
    }
    entry
        .get("ErrorEquals")
        .and_then(Value::as_array)
        .is_some_and(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .any(|name| name == error || name == "States.ALL")
        })
}

/// `context` with `$$.State.RetryCount` set to the attempt the state is on.
fn with_retry_count(context: &Value, attempt: u64) -> Value {
    let mut context = context.clone();
    if let Some(state) = context.get_mut("State").and_then(Value::as_object_mut) {
        state.insert("RetryCount".to_string(), json!(attempt));
    }
    context
}

/// `context` with `$$.Map.Item` set to the item being iterated.
fn with_map_item(context: &Value, map_item: &Value) -> Value {
    let mut context = context.clone();
    if let Some(object) = context.as_object_mut() {
        object.insert("Map".to_string(), json!({ "Item": map_item }));
    }
    context
}

// ---------------------------------------------------------------------------
// Payload templates, paths and intrinsic functions
// ---------------------------------------------------------------------------

/// Evaluate a payload template (`Parameters`, `ItemSelector`, `ResultSelector`).
///
/// "If any field within the Payload Template ... has a name ending with the
/// characters '.$', its value is transformed according to rules below and the field
/// is renamed to strip the '.$' suffix": the value is either a path or an intrinsic
/// function invocation.
///
/// A template is a CALLER's, and so is the document each of its members selects
/// from, so the PRODUCT of the two — how many members it declares, how many of them
/// are expressions to evaluate, and how large each member's value is — is what has to
/// be bounded. Every member is measured as it is rendered and spent from one budget
/// of [`MAX_PAYLOAD_BYTES`], and at most [`MAX_TEMPLATE_PATHS`] of them may be
/// expressions, so the template can never build more than a state's output may carry
/// nor evaluate more expressions than one visit to a state is allowed.
fn payload_template(node: &Value, input: &Value, context: &Value) -> Result<Value, Failure> {
    let mut budget = TemplateBudget {
        bytes: MAX_PAYLOAD_BYTES,
        paths: MAX_TEMPLATE_PATHS,
    };
    render_template(node, input, context, 1, &mut budget)
}

/// What one payload template may still build, and still evaluate.
struct TemplateBudget {
    bytes: usize,
    paths: usize,
}

impl TemplateBudget {
    /// Spend `bytes` of what the template may still build.
    fn spend(&mut self, bytes: usize) -> Result<(), Failure> {
        match self.bytes.checked_sub(bytes) {
            Some(left) => {
                self.bytes = left;
                Ok(())
            }
            None => Err(named(
                DATA_LIMIT_ERROR,
                format!(
                    "the payload template builds more than the {MAX_PAYLOAD_BYTES} bytes a \
                     state's output may carry"
                ),
            )),
        }
    }

    /// Charge one expression evaluation.
    fn charge_path(&mut self) -> Result<(), Failure> {
        match self.paths.checked_sub(1) {
            Some(left) => {
                self.paths = left;
                Ok(())
            }
            None => Err(named(
                PARAMETER_PATH_ERROR,
                format!(
                    "the payload template carries more than the {MAX_TEMPLATE_PATHS} paths this \
                     service evaluates"
                ),
            )),
        }
    }
}

/// [`payload_template`], carrying how deeply the template itself nests so the
/// descent is bounded by [`MAX_PAYLOAD_DEPTH`] rather than by the thread's stack, and
/// what the whole template may still build.
fn render_template(
    node: &Value,
    input: &Value,
    context: &Value,
    depth: usize,
    budget: &mut TemplateBudget,
) -> Result<Value, Failure> {
    if depth > MAX_PAYLOAD_DEPTH {
        return Err(named(
            PARAMETER_PATH_ERROR,
            format!(
                "the payload template nests more than the {MAX_PAYLOAD_DEPTH} levels this service \
                 evaluates"
            ),
        ));
    }
    match node {
        Value::Object(members) => {
            let mut rendered = Map::with_capacity(members.len());
            for (key, value) in members {
                budget.spend(key.len() + 3)?;
                match key.strip_suffix(".$") {
                    Some(stripped) => {
                        let text = value.as_str().ok_or_else(|| {
                            named(
                                PARAMETER_PATH_ERROR,
                                format!(
                                    "'{key}' must be a JSONPath or intrinsic-function string, not \
                                     {value}"
                                ),
                            )
                        })?;
                        budget.charge_path()?;
                        let built = expression(text, input, context)?;
                        budget.spend(measure_payload(&built, "a payload template")?)?;
                        rendered.insert(stripped.to_string(), built);
                    }
                    None => {
                        rendered.insert(
                            key.clone(),
                            render_template(value, input, context, depth + 1, budget)?,
                        );
                    }
                }
            }
            Ok(Value::Object(rendered))
        }
        Value::Array(items) => {
            let mut rendered = Vec::with_capacity(items.len());
            for item in items {
                budget.spend(1)?;
                rendered.push(render_template(item, input, context, depth + 1, budget)?);
            }
            Ok(Value::Array(rendered))
        }
        other => {
            budget.spend(measure_payload(other, "a payload template")?)?;
            Ok(other.clone())
        }
    }
}

/// Evaluate the value of a `"<name>.$"` member: a path or an intrinsic function.
fn expression(text: &str, input: &Value, context: &Value) -> Result<Value, Failure> {
    if text.starts_with("States.") {
        return intrinsic(text, input, context);
    }
    select(text, input, context)
}

/// Evaluate a JSONPath against the state's input, or against the context object when
/// it begins with `$$`.
fn select(path: &str, input: &Value, context: &Value) -> Result<Value, Failure> {
    let selected = select_optional(path, input, context)?;
    selected.ok_or_else(|| runtime(format!("the path '{path}' selected no value")))
}

/// [`select`], telling a value the document does not carry (`Ok(None)`) apart from a
/// path this service REFUSES (`Err`).
///
/// ASL's presence and type tests (`IsPresent`, `IsNull`, …) are the only comparators
/// defined for a path the input may not carry, so they are the only caller that reads
/// the `None`: a path the service refuses stays a failure there too, exactly as it is
/// under every other comparator.
fn select_optional(
    path: &str,
    input: &Value,
    context: &Value,
) -> Result<Option<Value>, Failure> {
    match path.strip_prefix('$') {
        // "paths that begin with $$ ... are evaluated against the Context Object."
        Some(rest) if rest.starts_with('$') => select_path(rest, context),
        Some(_) => select_path(path, input),
        None => Err(runtime(format!(
            "'{path}' is not a JSONPath: a path begins with '$'"
        ))),
    }
}

/// Evaluate `expression` against `root`, which must select exactly one value — ASL
/// paths are reference paths.
///
/// The expression is a CALLER's, so what it may ask for is bounded BEFORE the engine
/// evaluates it, because a node list the engine has already built cannot be refused
/// after the fact: it must be no longer than [`MAX_PATH_CHARS`] and it must be a
/// REFERENCE path — `$`, `.name`, `['name']`, `[index]`, at most
/// [`MAX_PATH_SEGMENTS`] of them — which is what the Amazon States Language's own
/// paths are ("a Path with syntax limited so that it can identify only a single
/// node"). A reference path identifies ONE node, so the cost of evaluating it is its
/// segment count and does NOT grow with the size of the document: a descendant
/// (`..`), a wildcard or a filter would walk the whole document and is refused here.
fn select_path(expression: &str, root: &Value) -> Result<Option<Value>, Failure> {
    if expression.len() > MAX_PATH_CHARS {
        return Err(runtime(format!(
            "a path of {} bytes is longer than the {MAX_PATH_CHARS} this service evaluates",
            expression.len()
        )));
    }
    reference_segments(expression).map_err(|why| {
        runtime(format!(
            "'{expression}' is not a reference path this service evaluates: {why}"
        ))
    })?;
    let parsed = JsonPath::parse(expression).map_err(|error| {
        runtime(format!(
            "'{expression}' is not a JSONPath this service can evaluate: {error}"
        ))
    })?;
    let nodes = parsed.query(root);
    match nodes.len() {
        1 => Ok(nodes.first().cloned()),
        // The document does not carry this path, which is an ANSWER for ASL's
        // presence and type tests and a failure for every other comparator.
        0 => Ok(None),
        many => Err(runtime(format!(
            "the path '{expression}' selected {many} values, and a reference path must select \
             exactly one"
        ))),
    }
}

/// One segment of a reference path.
enum Segment {
    Key(String),
    Index(usize),
}

/// Split a reference path (`$`, `$.a.b`, `$['a'][0]`) into its segments.
///
/// The path text comes from a definition the model bounds only at 1 MiB, so the
/// number of segments is capped at [`MAX_PATH_SEGMENTS`]: a reference path names a
/// place in a document, so it is short, and a longer one is refused rather than
/// walked.
fn reference_segments(path: &str) -> Result<Vec<Segment>, String> {
    let mut rest = path
        .strip_prefix('$')
        .ok_or_else(|| format!("'{path}' is not a reference path: one begins with '$'"))?;
    let mut segments = Vec::new();
    while !rest.is_empty() {
        if segments.len() >= MAX_PATH_SEGMENTS {
            return Err(format!(
                "'{path}' names more than the {MAX_PATH_SEGMENTS} reference-path segments this \
                 service follows"
            ));
        }
        if let Some(tail) = rest.strip_prefix('.') {
            let end = tail
                .find(['.', '['])
                .unwrap_or(tail.len());
            let (name, tail) = tail.split_at(end);
            if name.is_empty() {
                return Err(format!("'{path}' carries an empty member name"));
            }
            segments.push(Segment::Key(name.to_string()));
            rest = tail;
            continue;
        }
        if let Some(tail) = rest.strip_prefix('[') {
            let end = tail
                .find(']')
                .ok_or_else(|| format!("'{path}' has an unclosed '['"))?;
            let (inside, tail) = tail.split_at(end);
            rest = &tail[1..];
            let inside = inside.trim();
            let quoted = inside
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
                .or_else(|| {
                    inside
                        .strip_prefix('"')
                        .and_then(|value| value.strip_suffix('"'))
                });
            match quoted {
                Some(name) => segments.push(Segment::Key(name.to_string())),
                None => {
                    let index = inside
                        .parse::<usize>()
                        .map_err(|_| format!("'{path}' carries the subscript '{inside}', which is neither a quoted member name nor an array index"))?;
                    segments.push(Segment::Index(index));
                }
            }
            continue;
        }
        return Err(format!(
            "'{path}' is not a reference path: a segment is either '.name' or '[...]'"
        ));
    }
    Ok(segments)
}

/// `raw` with `result` placed at the reference path `path`.
///
/// "If the ResultPath matches an item in the state's input, only that input item is
/// overwritten", and a path the input does not carry yet is created.
fn set_at_reference_path(raw: &Value, path: &str, result: Value) -> Result<Value, Failure> {
    let segments = reference_segments(path).map_err(|why| named(RESULT_PATH_ERROR, why))?;
    let mut target = raw.clone();
    assign(&mut target, &segments, result, path).map_err(|why| named(RESULT_PATH_ERROR, why))?;
    Ok(target)
}

/// Place `result` at `segments` within `target`, creating the objects it names.
///
/// Walked ITERATIVELY, so the descent costs no stack: the path's segment count is a
/// caller's choice, and [`MAX_PATH_SEGMENTS`] bounds the WORK rather than having to
/// bound the recursion depth.
fn assign(
    target: &mut Value,
    segments: &[Segment],
    result: Value,
    path: &str,
) -> Result<(), String> {
    let mut cursor = target;
    for segment in segments {
        cursor = match segment {
            Segment::Key(key) => {
                if cursor.is_null() {
                    *cursor = Value::Object(Map::new());
                }
                if !cursor.is_object() {
                    return Err(format!(
                        "'{path}' names a member of {cursor}, which is not a JSON object"
                    ));
                }
                let Some(object) = cursor.as_object_mut() else {
                    return Err(format!("'{path}' names a member of a document that is not an object"));
                };
                object.entry(key.clone()).or_insert(Value::Null)
            }
            Segment::Index(index) => {
                if !cursor.is_array() {
                    return Err(format!(
                        "'{path}' names index {index} of {cursor}, which is not a JSON array"
                    ));
                }
                let Some(array) = cursor.as_array_mut() else {
                    return Err(format!("'{path}' names an index of a document that is not an array"));
                };
                let length = array.len();
                let Some(slot) = array.get_mut(*index) else {
                    return Err(format!(
                        "'{path}' names index {index}, past the end of an array of {length} items"
                    ));
                };
                slot
            }
        };
    }
    *cursor = result;
    Ok(())
}

// ---------------------------------------------------------------------------
// Choice conditions
// ---------------------------------------------------------------------------

/// Whether one choice rule matches.
fn condition_matches(
    rule: &Map<String, Value>,
    input: &Value,
    context: &Value,
) -> Result<bool, Failure> {
    condition_matches_at(rule, input, context, 1)
}

/// [`condition_matches`] for one rule — or one nested `And`/`Or`/`Not` condition —
/// carrying how deeply the condition tree nests, so the descent is bounded by
/// [`MAX_EXPRESSION_DEPTH`] rather than by the thread's stack: the tree is a
/// caller's, in a definition bounded only in bytes.
fn condition_matches_at(
    rule: &Map<String, Value>,
    input: &Value,
    context: &Value,
    depth: usize,
) -> Result<bool, Failure> {
    if depth > MAX_EXPRESSION_DEPTH {
        return Err(runtime(format!(
            "a choice rule nests conditions more than the {MAX_EXPRESSION_DEPTH} levels this \
             service evaluates"
        )));
    }
    if let Some(conditions) = rule.get("And") {
        for condition in nested_conditions(conditions, "And")? {
            if !condition_matches_at(condition, input, context, depth + 1)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if let Some(conditions) = rule.get("Or") {
        for condition in nested_conditions(conditions, "Or")? {
            if condition_matches_at(condition, input, context, depth + 1)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if let Some(condition) = rule.get("Not") {
        let condition = condition
            .as_object()
            .ok_or_else(|| runtime("'Not' must carry a single condition object".to_string()))?;
        return Ok(!condition_matches_at(condition, input, context, depth + 1)?);
    }

    let variable = rule
        .get("Variable")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            runtime("a choice rule must carry a string 'Variable' and one comparator".to_string())
        })?;
    let (operator, operand) = rule
        .iter()
        .find(|(key, _)| !matches!(key.as_str(), "Variable" | "Next" | "Comment"))
        .ok_or_else(|| {
            runtime(format!(
                "the choice rule on '{variable}' carries no comparator"
            ))
        })?;
    compare(operator, variable, operand, input, context)
}

/// The condition objects of an `And`/`Or` member.
fn nested_conditions<'a>(
    node: &'a Value,
    member: &str,
) -> Result<Vec<&'a Map<String, Value>>, Failure> {
    let items = node.as_array().filter(|items| !items.is_empty()).ok_or_else(|| {
        runtime(format!("'{member}' must carry a non-empty array of conditions"))
    })?;
    if items.len() > MAX_CHOICE_RULES {
        return Err(runtime(format!(
            "'{member}' carries {} conditions, more than the {MAX_CHOICE_RULES} this service \
             evaluates",
            items.len()
        )));
    }
    items
        .iter()
        .map(|item| {
            item.as_object().ok_or_else(|| {
                runtime(format!("every condition of '{member}' must be a JSON object"))
            })
        })
        .collect()
}

/// Evaluate one comparator of a choice rule.
fn compare(
    operator: &str,
    variable: &str,
    operand: &Value,
    input: &Value,
    context: &Value,
) -> Result<bool, Failure> {
    // The presence and type tests are the only comparators ASL defines for a path the
    // input may not carry at all, so they are answered before the variable is read.
    if let Some(test) = operator.strip_prefix("Is") {
        let expected = operand.as_bool().ok_or_else(|| {
            runtime(format!("'{operator}' must be compared against a boolean"))
        })?;
        // A path this service REFUSES is a failure here too; only a value the
        // document does not carry is the absence these tests are defined for.
        let value = select_optional(variable, input, context)?;
        let holds = match (test, &value) {
            ("Present", value) => value.is_some(),
            (_, None) => false,
            ("Null", Some(value)) => value.is_null(),
            ("Numeric", Some(value)) => value.is_number(),
            ("String", Some(value)) => value.is_string(),
            ("Boolean", Some(value)) => value.is_boolean(),
            ("Timestamp", Some(value)) => value
                .as_str()
                .is_some_and(|text| iso_millis(text).is_ok()),
            (other, _) => {
                return Err(runtime(format!(
                    "'Is{other}' is not a comparator this service evaluates"
                )));
            }
        };
        return Ok(holds == expected);
    }

    let value = select(variable, input, context)?;
    let (family, operand) = match operator.strip_suffix("Path") {
        Some(family) => {
            let path = operand.as_str().ok_or_else(|| {
                runtime(format!("'{operator}' must be given a JSONPath string"))
            })?;
            (family, select(path, input, context)?)
        }
        None => (operator, operand.clone()),
    };

    let mismatch = |expected: &str| {
        runtime(format!(
            "the choice rule on '{variable}' compares {value} with {operand} through \
             '{operator}', which compares {expected}"
        ))
    };
    if let Some(comparison) = family.strip_prefix("String") {
        if comparison == "Matches" {
            let (Some(text), Some(pattern)) = (value.as_str(), operand.as_str()) else {
                return Err(mismatch("strings"));
            };
            return glob_matches(pattern, text);
        }
        let (Some(left), Some(right)) = (value.as_str(), operand.as_str()) else {
            return Err(mismatch("strings"));
        };
        return ordering(comparison, left.cmp(right)).ok_or_else(|| mismatch("strings"));
    }
    if let Some(comparison) = family.strip_prefix("Numeric") {
        let (Some(left), Some(right)) = (value.as_f64(), operand.as_f64()) else {
            return Err(mismatch("numbers"));
        };
        let order = left
            .partial_cmp(&right)
            .ok_or_else(|| mismatch("numbers"))?;
        return ordering(comparison, order).ok_or_else(|| mismatch("numbers"));
    }
    if let Some(comparison) = family.strip_prefix("Boolean") {
        let (Some(left), Some(right)) = (value.as_bool(), operand.as_bool()) else {
            return Err(mismatch("booleans"));
        };
        if comparison != "Equals" {
            return Err(mismatch("booleans"));
        }
        return Ok(left == right);
    }
    if let Some(comparison) = family.strip_prefix("Timestamp") {
        let (Some(left), Some(right)) = (value.as_str(), operand.as_str()) else {
            return Err(mismatch("ISO 8601 timestamps"));
        };
        let (left, right) = (
            iso_millis(left).map_err(runtime)?,
            iso_millis(right).map_err(runtime)?,
        );
        return ordering(comparison, left.cmp(&right)).ok_or_else(|| mismatch("timestamps"));
    }
    Err(runtime(format!(
        "'{operator}' is not a comparator this service evaluates"
    )))
}

/// Whether `order` satisfies one of ASL's four ordering comparisons.
fn ordering(comparison: &str, order: std::cmp::Ordering) -> Option<bool> {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match comparison {
        "Equals" => Some(order == Equal),
        "LessThan" => Some(order == Less),
        "LessThanEquals" => Some(order != Greater),
        "GreaterThan" => Some(order == Greater),
        "GreaterThanEquals" => Some(order != Less),
        _ => None,
    }
}

/// One token of a `StringMatches` glob.
enum Glob {
    /// A literal character, which an escape (`\\*`) also produces.
    Char(char),
    /// `*`, which "matches zero or more characters".
    Star,
}

/// `StringMatches`' glob: "The only character that has any special meaning is '*',
/// which matches zero or more characters", escaped as `\\*`.
///
/// Matched with explicit BACKTRACKING over the last wildcard, so a pattern whose
/// literal also occurs earlier in the text still matches: a leftmost-first scan that
/// never reconsidered would answer `false` for `a*bc` against `abcbc`, which the glob
/// does match, and a `Choice` would then take a branch the definition does not
/// dictate. A pattern longer than [`MAX_GLOB_CHARS`] is REFUSED rather than reported
/// as no match, for the same reason: a ceiling this emulator sets is a failure, never
/// an answer about the data.
fn glob_matches(pattern: &str, text: &str) -> Result<bool, Failure> {
    if pattern.chars().count() > MAX_GLOB_CHARS {
        return Err(runtime(format!(
            "a 'StringMatches' pattern of {} characters is longer than the {MAX_GLOB_CHARS} this \
             service matches",
            pattern.chars().count()
        )));
    }
    let mut tokens: Vec<Glob> = Vec::new();
    let mut characters = pattern.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                if let Some(escaped) = characters.next() {
                    tokens.push(Glob::Char(escaped));
                }
            }
            '*' => tokens.push(Glob::Star),
            other => tokens.push(Glob::Char(other)),
        }
    }

    let text: Vec<char> = text.chars().collect();
    let mut token = 0usize;
    let mut at = 0usize;
    // Where to resume if the current attempt fails: the wildcard last passed, and
    // the position it had consumed up to.
    let mut star: Option<usize> = None;
    let mut after_star = 0usize;
    while at < text.len() {
        match tokens.get(token) {
            Some(Glob::Char(expected)) if *expected == text[at] => {
                token += 1;
                at += 1;
            }
            Some(Glob::Star) => {
                star = Some(token);
                after_star = at;
                token += 1;
            }
            _ => match star {
                // The wildcard consumes one more character and the attempt is made
                // again from there.
                Some(resume) => {
                    token = resume + 1;
                    after_star += 1;
                    at = after_star;
                }
                None => return Ok(false),
            },
        }
    }
    // Trailing wildcards may match nothing at all.
    while matches!(tokens.get(token), Some(Glob::Star)) {
        token += 1;
    }
    Ok(token == tokens.len())
}

// ---------------------------------------------------------------------------
// Intrinsic functions
// ---------------------------------------------------------------------------

/// Evaluate an intrinsic function invocation (`States.Format('{}', $.name)`).
fn intrinsic(text: &str, input: &Value, context: &Value) -> Result<Value, Failure> {
    invoke_intrinsic(text, input, context, 1)
}

/// [`intrinsic`], carrying how deeply the invocation nests.
///
/// This is this crate's own parser for ASL's intrinsic syntax, over text that is a
/// value of a definition the model bounds only at 1 MiB — and the nesting lives
/// inside ONE JSON string, so `serde_json`'s own recursion limit does not bound it.
/// Both the length of the invocation ([`MAX_EXPRESSION_CHARS`]) and how deeply it
/// nests ([`MAX_EXPRESSION_DEPTH`]) are therefore bounded here, and an invocation
/// past either is reported with [`INTRINSIC_ERROR`] rather than parsed.
fn invoke_intrinsic(
    text: &str,
    input: &Value,
    context: &Value,
    depth: usize,
) -> Result<Value, Failure> {
    let text = text.trim();
    if depth > MAX_EXPRESSION_DEPTH {
        return Err(named(
            INTRINSIC_ERROR,
            format!(
                "an intrinsic-function invocation nests more than the {MAX_EXPRESSION_DEPTH} \
                 levels this service evaluates"
            ),
        ));
    }
    if text.len() > MAX_EXPRESSION_CHARS {
        return Err(named(
            INTRINSIC_ERROR,
            format!(
                "an intrinsic-function invocation of {} bytes is longer than the \
                 {MAX_EXPRESSION_CHARS} this service evaluates",
                text.len()
            ),
        ));
    }
    let open = text.find('(').ok_or_else(|| {
        named(
            INTRINSIC_ERROR,
            format!("'{text}' is not an intrinsic-function invocation"),
        )
    })?;
    if !text.ends_with(')') {
        return Err(named(
            INTRINSIC_ERROR,
            format!("the intrinsic invocation '{text}' is not closed"),
        ));
    }
    let name = text[..open].trim();
    let arguments = split_arguments(&text[open + 1..text.len() - 1])
        .map_err(|why| named(INTRINSIC_ERROR, format!("'{text}': {why}")))?;
    // Each argument may select a whole document, and an invocation may carry as many
    // arguments as its text has room for, so the arguments are measured as they are
    // evaluated and bounded together: the function below cannot be handed more than a
    // state's output may carry, so it cannot build more either.
    let mut spent = 0usize;
    let mut values = Vec::with_capacity(arguments.len());
    for argument in &arguments {
        let value = argument_value(argument, input, context, depth)?;
        spent = spent.saturating_add(measure_payload(&value, "an intrinsic argument")?);
        if spent > MAX_PAYLOAD_BYTES {
            return Err(named(
                INTRINSIC_ERROR,
                format!(
                    "'{name}' was given at least {spent} bytes of arguments, more than the \
                     {MAX_PAYLOAD_BYTES} a state's output may carry"
                ),
            ));
        }
        values.push(value);
    }
    let result = apply_intrinsic(name, &values)
        .map_err(|why| named(INTRINSIC_ERROR, format!("'{text}': {why}")))?;
    // And what it built is bounded too, because a function may expand what it was
    // given.
    measure_payload(&result, "an intrinsic function")?;
    Ok(result)
}

/// Split an intrinsic's argument list on the commas that are not inside a nested
/// invocation or a single-quoted string.
fn split_arguments(text: &str) -> Result<Vec<String>, String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' if quoted => {
                current.push(character);
                if let Some(escaped) = characters.next() {
                    current.push(escaped);
                }
            }
            '\'' => {
                quoted = !quoted;
                current.push(character);
            }
            '(' if !quoted => {
                depth += 1;
                current.push(character);
            }
            ')' if !quoted => {
                depth = depth.checked_sub(1).ok_or("unbalanced ')'")?;
                current.push(character);
            }
            ',' if !quoted && depth == 0 => {
                arguments.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    if quoted {
        return Err("an unterminated quoted string".to_string());
    }
    if depth != 0 {
        return Err("an unclosed '('".to_string());
    }
    if !current.trim().is_empty() || !arguments.is_empty() {
        arguments.push(current);
    }
    Ok(arguments)
}

/// One intrinsic argument: a nested invocation, a path, a quoted string, or a JSON
/// literal. `depth` is the nesting of the invocation this argument belongs to.
fn argument_value(
    text: &str,
    input: &Value,
    context: &Value,
    depth: usize,
) -> Result<Value, Failure> {
    let text = text.trim();
    if text.starts_with("States.") && text.contains('(') {
        return invoke_intrinsic(text, input, context, depth + 1);
    }
    if text.starts_with('$') {
        return select(text, input, context);
    }
    if let Some(quoted) = text.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')) {
        // "escape ... the following characters: ' { } \" with a backslash.
        let mut unescaped = String::with_capacity(quoted.len());
        let mut characters = quoted.chars();
        while let Some(character) = characters.next() {
            if character == '\\' {
                if let Some(escaped) = characters.next() {
                    unescaped.push(escaped);
                }
            } else {
                unescaped.push(character);
            }
        }
        return Ok(Value::String(unescaped));
    }
    serde_json::from_str(text).map_err(|error| {
        named(
            INTRINSIC_ERROR,
            format!("'{text}' is not a path, a quoted string or a JSON literal: {error}"),
        )
    })
}

/// Apply one intrinsic function to its evaluated arguments.
///
/// The set below is every documented intrinsic except `States.Hash`, which is
/// DECLINED: the algorithms it names include MD5 and SHA-1, and this emulator does
/// not take a dependency on broken hash functions to evaluate a state's payload
/// template. Like any other name this closed allow-list does not carry, it is
/// reported with [`INTRINSIC_ERROR`] rather than approximated.
fn apply_intrinsic(name: &str, arguments: &[Value]) -> Result<Value, String> {
    let argument = |index: usize| -> Result<&Value, String> {
        arguments
            .get(index)
            .ok_or_else(|| format!("'{name}' needs at least {} arguments", index + 1))
    };
    // A JSON number is finite by construction, but the arithmetic below is only
    // total for a finite value, so the guard is stated rather than assumed.
    let number = |index: usize| -> Result<f64, String> {
        argument(index)?
            .as_f64()
            .filter(|number| number.is_finite())
            .ok_or_else(|| format!("argument {} of '{name}' must be a finite number", index + 1))
    };
    let string = |index: usize| -> Result<String, String> {
        argument(index)?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("argument {} of '{name}' must be a string", index + 1))
    };
    let array = |index: usize| -> Result<Vec<Value>, String> {
        let items = argument(index)?
            .as_array()
            .cloned()
            .ok_or_else(|| format!("argument {} of '{name}' must be an array", index + 1))?;
        if items.len() > MAX_ARRAY_ITEMS {
            return Err(format!(
                "argument {} of '{name}' carries {} items, more than the {MAX_ARRAY_ITEMS} this \
                 function walks",
                index + 1,
                items.len()
            ));
        }
        Ok(items)
    };

    match name {
        "States.Format" => {
            let template = string(0)?;
            let mut rendered = String::with_capacity(template.len());
            let mut next = 1usize;
            let mut characters = template.chars().peekable();
            while let Some(character) = characters.next() {
                match character {
                    '\\' => {
                        if let Some(escaped) = characters.next() {
                            rendered.push(escaped);
                        }
                    }
                    '{' if characters.peek() == Some(&'}') => {
                        characters.next();
                        let value = argument(next)?;
                        next += 1;
                        match value {
                            Value::String(text) => rendered.push_str(text),
                            other => rendered.push_str(&other.to_string()),
                        }
                    }
                    other => rendered.push(other),
                }
            }
            Ok(Value::String(rendered))
        }
        "States.StringToJson" => {
            let text = string(0)?;
            serde_json::from_str(&text).map_err(|error| format!("{text:?} is not JSON: {error}"))
        }
        "States.JsonToString" => Ok(Value::String(argument(0)?.to_string())),
        "States.Array" => Ok(Value::Array(arguments.to_vec())),
        "States.ArrayPartition" => {
            let items = array(0)?;
            let size = number(1)?;
            if size < 1.0 {
                return Err("the chunk size must be a positive whole number".to_string());
            }
            Ok(Value::Array(
                items
                    .chunks(size as usize)
                    .map(|chunk| Value::Array(chunk.to_vec()))
                    .collect(),
            ))
        }
        "States.ArrayContains" => {
            let items = array(0)?;
            let looked_for = argument(1)?;
            Ok(Value::Bool(items.iter().any(|item| item == looked_for)))
        }
        "States.ArrayRange" => {
            let (start, end, step) = (number(0)?, number(1)?, number(2)?);
            if step == 0.0 {
                return Err("the step must not be zero".to_string());
            }
            let mut values = Vec::new();
            let mut current = start;
            while (step > 0.0 && current <= end) || (step < 0.0 && current >= end) {
                values.push(json_number(current));
                current += step;
                // The array is built from three numbers, so nothing the caller sent
                // bounds its length: this does.
                if values.len() > MAX_ARRAY_RANGE_VALUES {
                    return Err(format!(
                        "the range is longer than the {MAX_ARRAY_RANGE_VALUES} values this \
                         function builds"
                    ));
                }
            }
            Ok(Value::Array(values))
        }
        "States.ArrayGetItem" => {
            let items = array(0)?;
            let index = number(1)?;
            if index < 0.0 {
                return Err("the index must not be negative".to_string());
            }
            items
                .get(index as usize)
                .cloned()
                .ok_or_else(|| format!("index {index} is past the end of the array"))
        }
        "States.ArrayLength" => Ok(json!(array(0)?.len())),
        "States.ArrayUnique" => {
            // Compared by the canonical TEXT of each value, through a set, so the
            // pass is O(n log n) rather than the O(n^2) a linear scan per item would
            // cost on an array the execution input sized.
            let mut seen = BTreeSet::new();
            let mut unique: Vec<Value> = Vec::new();
            for item in array(0)? {
                if seen.insert(item.to_string()) {
                    unique.push(item);
                }
            }
            Ok(Value::Array(unique))
        }
        "States.JsonMerge" => {
            let mut merged = argument(0)?
                .as_object()
                .cloned()
                .ok_or_else(|| format!("argument 1 of '{name}' must be a JSON object"))?;
            let second = argument(1)?
                .as_object()
                .ok_or_else(|| format!("argument 2 of '{name}' must be a JSON object"))?;
            let deep = argument(2)?.as_bool().unwrap_or(false);
            if deep {
                return Err("a deep merge is not one this service performs".to_string());
            }
            for (key, value) in second {
                merged.insert(key.clone(), value.clone());
            }
            Ok(Value::Object(merged))
        }
        "States.MathAdd" => Ok(json_number(number(0)? + number(1)?)),
        "States.MathRandom" => {
            let (start, end) = (number(0)?, number(1)?);
            if end < start {
                return Err("the end of the range must not be below its start".to_string());
            }
            let mut seed = match arguments.get(2).and_then(Value::as_f64) {
                Some(seed) => seed as u64,
                None => super::now_nanos(),
            };
            // Both bounds are caller-chosen finite numbers, so every step of this is
            // saturating: a float-to-integer cast saturates, and the two additions
            // would otherwise overflow for a range near the integer maxima.
            let span = ((end - start) as u64).saturating_add(1);
            let drawn = super::splitmix64(&mut seed) % span.max(1);
            let drawn = i64::try_from(drawn).unwrap_or(i64::MAX);
            Ok(json!((start as i64).saturating_add(drawn)))
        }
        // "States.Base64Encode ... encode data based on MIME Base64 encoding scheme.
        // You can use this function to pass data to other AWS services without using
        // an AWS Lambda function", with a documented 10,000-character input limit.
        "States.Base64Encode" => {
            let text = string(0)?;
            if text.len() > MAX_BASE64_CHARS {
                return Err(format!(
                    "the input is {} characters, more than the {MAX_BASE64_CHARS} this function \
                     encodes",
                    text.len()
                ));
            }
            Ok(Value::String(::aws_smithy_types::base64::encode(text)))
        }
        "States.Base64Decode" => {
            let text = string(0)?;
            if text.len() > MAX_BASE64_CHARS {
                return Err(format!(
                    "the input is {} characters, more than the {MAX_BASE64_CHARS} this function \
                     decodes",
                    text.len()
                ));
            }
            let decoded = ::aws_smithy_types::base64::decode(&text)
                .map_err(|error| format!("the input is not MIME Base64: {error}"))?;
            String::from_utf8(decoded)
                .map(Value::String)
                .map_err(|error| format!("the decoded data is not UTF-8 text: {error}"))
        }
        "States.StringSplit" => {
            let text = string(0)?;
            let delimiters: Vec<char> = string(1)?.chars().collect();
            let parts = text
                .split(|character| delimiters.contains(&character))
                .filter(|part| !part.is_empty())
                .map(|part| Value::String(part.to_string()))
                .collect();
            Ok(Value::Array(parts))
        }
        "States.UUID" => {
            let mut seed = super::now_nanos();
            let high = super::splitmix64(&mut seed);
            let low = super::splitmix64(&mut seed);
            Ok(Value::String(super::uuid_v4(high, low)))
        }
        other => Err(format!(
            "'{other}' is an intrinsic function this emulator does not evaluate, so nothing was \
             run for it"
        )),
    }
}

/// A number an intrinsic computed, as the JSON value the wire carries: a whole
/// result is a whole number, which is what the service's own intrinsics return.
fn json_number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        return json!(value as i64);
    }
    json!(value)
}

// ---------------------------------------------------------------------------
// Time helpers
// ---------------------------------------------------------------------------

/// `time` as the ISO 8601 instant the context object reports.
fn iso(time: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from_millis(super::epoch_millis(time))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// An ISO 8601 instant in whole milliseconds since the Unix epoch.
fn iso_millis(text: &str) -> Result<i64, String> {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .map_err(|error| format!("'{text}' is not an ISO 8601 instant: {error}"))?;
    parsed
        .to_millis()
        .map_err(|error| format!("'{text}' is out of range: {error}"))
}

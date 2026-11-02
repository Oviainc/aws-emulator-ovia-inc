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
//! A `Task` state's work is RUN too, in the one way this process can run each kind:
//!
//! * a `Resource` naming a service INTEGRATION is called — the call goes to the
//!   other service's own handler in this same emulator, with the document the state
//!   built as that operation's request and the operation's response as the state's
//!   result (see [`super::integration`], which also states which integrations are
//!   made and which are not). A `Resource` asking for the run-a-job (`.sync`)
//!   pattern is called the same way, and the task is then PARKED at the job that
//!   call started ([`ParkedJob`]) until the invoked service reports it finished,
//!   which is what resolves the task ([`super::run_a_job`]);
//! * a `Resource` naming an ACTIVITY ARN belongs to a worker that polls this
//!   emulator, so the state SCHEDULES a task carrying the state's effective input and
//!   the walk SUSPENDS there ([`Run::waiting`]), which is the execution waiting for a
//!   worker exactly as it does on the real service. `GetActivityTask` hands that task
//!   out; the worker's reply closes it.
//!
//! Two things decide whether that call is made at all, and both answer before it
//! would be: the WORKFLOW TYPE, since the run-a-job pattern is served for `STANDARD`
//! only and an EXPRESS execution fails at the state that asks for it
//! ([`EXPRESS_RUN_A_JOB_UNSUPPORTED`]); and what the state machine's EXECUTION ROLE
//! may do, since the call is made as that role and one its policies do not allow
//! fails the task with the invoked service's own refusal
//! ([`super::task_authorization`], asked through [`Tasks::authorize`]). Neither
//! starts anything first.
//!
//! Every state records the pair of events the language names it with — entered, and
//! left — each carrying the state's own data: the document it was entered WITH, and
//! the one it EMITTED once its result-side filters had been applied, which is what a
//! caller reconstructs an execution's data flow from.
//!
//! What is NOT executed is what cannot be executed inside this process:
//!
//! * an integration [`super::integration::read`] makes no call for — a service whose
//!   request is not one JSON document, the callback (`.waitForTaskToken`) pattern,
//!   a run-a-job pattern over an integration whose job cannot be reported or
//!   stopped, a `Resource` naming no integration at all;
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
//! [`RUNTIME_ERROR`], as any unknown intrinsic does.
//!
//! ## What is validated
//!
//! A definition is VALIDATED in both of the language's query languages, whether or
//! not this emulator evaluates its expressions: [`diagnose`] reports every problem
//! one carries — the errors that would stop `CreateStateMachine` from accepting it
//! and the warnings that would not — and [`validate`] is the first of those errors,
//! which is what the create and the update refuse a definition with. A definition
//! this emulator cannot RUN is still valid ASL, so it is created and the refusal
//! surfaces at execution time instead.
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
//! definition selected. Each Distributed Map state the walk entered is reported as a
//! [`StartedMapRun`], which the call that started the execution stores as a Map Run
//! of its own, and each of that state's items is reported as the child EXECUTION it
//! started ([`StartedChildExecution`]): one per item, carrying the `ItemSelector`
//! projection as its own input and the item's own result as its output, under an ARN
//! of its own that the item's states read through `$$.Execution.Id`. An INLINE Map
//! starts neither, exactly as the service starts neither for one.
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

use super::integration::{Failed, Tasks};

/// The ASL predefined error name for a failure the interpreter itself raises
/// ("States.Runtime — an execution failed due to some exception that could not be
/// processed").
///
/// It is the name an INTRINSIC-function invocation that could not be evaluated
/// carries too — a definition the walk could not run, exactly as the rest of this
/// name's failures are — and it is TERMINAL: it is neither retried nor caught
/// ([`errors_match`]), so a definition cannot recover from one.
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

/// The ASL predefined error name for a `Choice` state no rule matched and that
/// declares no `Default` ("States.NoChoiceMatched").
pub const NO_CHOICE_ERROR: &str = "States.NoChoiceMatched";

/// The ASL predefined error name for a Distributed Map whose failed items went past
/// its tolerated-failure threshold ("States.ExceedToleratedFailureThreshold").
pub const TOLERATED_FAILURE_ERROR: &str = "States.ExceedToleratedFailureThreshold";

/// The ASL predefined error name for an execution that ran longer than the
/// `TimeoutSeconds` its definition declares ("States.Timeout").
pub const TIMEOUT_ERROR: &str = "States.Timeout";

/// The refusal an EXPRESS workflow answers a `Task` state that asks for the
/// run-a-job (`.sync`) integration pattern with.
///
/// The pattern is served only for the STANDARD workflow type: an Express state
/// machine is created from such a definition, and every execution of it fails AT that
/// state with [`RUNTIME_ERROR`] carrying this sentence, having started no job. The
/// state it failed at is named beside it ([`express_run_a_job_refusal`]), because a
/// definition may carry several.
pub const EXPRESS_RUN_A_JOB_UNSUPPORTED: &str =
    "Express state machine does not support '.sync' service integration";

/// [`EXPRESS_RUN_A_JOB_UNSUPPORTED`] as the state `name` reports it.
fn express_run_a_job_refusal(name: &str) -> Failure {
    runtime(format!(
        "An error occurred while executing the state '{name}'. \
         {EXPRESS_RUN_A_JOB_UNSUPPORTED}"
    ))
}

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
/// reported with [`RUNTIME_ERROR`].
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
/// [`RUNTIME_ERROR`].
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

/// Every state type the Amazon States Language DEFINES: the language's `Type` member
/// selects one of these eight and nothing else, so a state declaring any other type
/// is not a state the language describes and no state machine can be created with it
/// ([`diagnose_state`] reports it).
///
/// The list is the union of the two groups the transition rule is keyed on —
/// [`TERMINAL_STATE_TYPES`] and [`CONTINUING_STATE_TYPES`] — plus `Choice`, which
/// transitions through its own `Choices`/`Default` members.
pub const STATE_TYPES: &[&str] = &[
    "Choice", "Fail", "Map", "Parallel", "Pass", "Succeed", "Task", "Wait",
];

/// The state types this emulator executes, which is every type the language defines.
///
/// `Task` is one of them for both kinds of work a `Resource` can name: an activity
/// task, which a worker polls for, and a service-integration call this emulator
/// makes ([`super::integration`]). A `Resource` naming neither still ends the
/// execution with [`RUNTIME_ERROR`].
pub const EXECUTED_STATE_TYPES: &[&str] = STATE_TYPES;

/// The query language this interpreter evaluates. ASL's `QueryLanguage` member
/// selects between it and JSONata; a JSONata definition is not run.
pub const JSONPATH: &str = "JSONPath";

/// The other query language ASL's `QueryLanguage` member selects: its expressions
/// are not evaluated here, but a definition written in it is still VALIDATED,
/// because its members are the ones the language declares rather than JSONPath's.
pub const JSONATA: &str = "JSONata";

/// The two query languages `QueryLanguage` selects between, at the definition
/// level and at one state's.
pub const QUERY_LANGUAGES: &[&str] = &[JSONPATH, JSONATA];

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
///
/// A `Task`'s members depend on WHICH work its `Resource` names, so the type alone
/// does not decide them: this answers the ACTIVITY task's set, and
/// [`INTEGRATION_TASK_MEMBERS`] the service-integration call's.
/// [`unexecutable_member`] is what picks between the two.
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
        // An ACTIVITY task's own work is done by a worker, so the members below are
        // the ones that decide what is scheduled for it, what the state does with the
        // reply, and what happens when the task fails or runs out of time: a `Retry`
        // re-schedules the task for its next attempt, which is a task a worker polls
        // for exactly as the attempt before it was. `Catch` is deliberately absent:
        // handing the failure to a catcher continues the walk from the catcher's
        // `Next`, and a state after it may be a service integration whose call is made
        // from the REQUEST the walk runs inside — a task the clock closes has no
        // request around it, so a state that declares one is not run rather than run as
        // though it declared none. `Credentials` is absent for the same reason it is on
        // every other type: it changes who the invocation is made as.
        "Task" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "Resource",
            "InputPath",
            "Parameters",
            "TimeoutSeconds",
            "TimeoutSecondsPath",
            "HeartbeatSeconds",
            "HeartbeatSecondsPath",
            "ResultSelector",
            "ResultPath",
            "OutputPath",
            "Retry",
            "Next",
            "End",
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

/// The members a `Task` state whose `Resource` names a SERVICE INTEGRATION may
/// carry: the ones that decide the request the call carries, what becomes of its
/// answer, and how a failed call is handled.
///
/// `HeartbeatSeconds`/`HeartbeatSecondsPath` and `Credentials` are deliberately
/// absent: a heartbeat is owed by an activity or a callback task, whose outcome
/// arrives after the call this emulator makes, and `Credentials` asks for the call to
/// be made under another role, which this emulator does not assume.
const INTEGRATION_TASK_MEMBERS: &[&str] = &[
    "Type",
    "Comment",
    "QueryLanguage",
    "Resource",
    "InputPath",
    "Parameters",
    "ResultSelector",
    "ResultPath",
    "OutputPath",
    "Retry",
    "Catch",
    "TimeoutSeconds",
    "TimeoutSecondsPath",
    "Next",
    "End",
];

/// The first member of `state` this emulator does not execute, if any.
fn unexecutable_member(state_type: &str, state: &Map<String, Value>) -> Option<String> {
    let allowed = match state_type {
        // The two kinds of `Task` work carry different members, and the `Resource`
        // is what says which kind this state is.
        "Task" if !super::names_an_activity(task_resource(state)) => INTEGRATION_TASK_MEMBERS,
        _ => executable_members(state_type),
    };
    state
        .keys()
        .find(|member| !allowed.contains(&member.as_str()))
        .cloned()
}

/// The `Resource` a `Task` state names, as the text the language writes it as — the
/// empty string for a state that carries none, which names no work either way.
fn task_resource(state: &Map<String, Value>) -> &str {
    state
        .get("Resource")
        .and_then(Value::as_str)
        .unwrap_or_default()
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
    /// What the walk knew about this event, which is what the history reports as that
    /// event type's own detail member: the work a `Task` state named and the document
    /// it was handed, the error and cause its work failed with, the item one `Map`
    /// iteration was handed and how many a `Map` state is iterating. A step whose
    /// event type carries no detail of its own leaves every member unset.
    pub details: super::EventDetails,
}

/// One item of a `Map` state's iteration, as the walk ran it.
///
/// The offset is into the RUN (the total of every `Wait` the walk passed before the
/// iteration ended), so the caller that stores it turns it into an instant against
/// the execution's own start — the walk never sleeps. It is what tells the item's
/// outcome apart from the ones that are still to come: an item is finished from this
/// offset on and not before.
#[derive(Debug, Clone, Copy)]
pub struct MapItemOutcome {
    /// How far into the run this item's iteration finished.
    pub finished_at: Duration,
    /// Whether the iteration FAILED, which is what a Map Run's failure budget counts.
    pub failed: bool,
}

/// One Distributed Map state the walk ran, as the Map Run it started.
///
/// A Distributed Map "starts a Map Run", a resource of its own that reports the
/// state's configuration and what its items produced. The offsets are into the RUN
/// (the total of every `Wait` the walk passed), so the caller that stores this turns
/// them into instants against the execution's own start — the walk never sleeps.
#[derive(Debug, Clone)]
pub struct StartedMapRun {
    /// The state's `Label` when it declares one, else the state's own name: "a
    /// user-defined or an auto-generated string that identifies a Map state".
    pub label: String,
    /// The status the state's items left the Map Run in: `SUCCEEDED` when the
    /// iterations produced a result, `FAILED` when more of them failed than the
    /// state tolerates.
    pub status: &'static str,
    /// How far into the run the state was entered.
    pub started_at: Duration,
    /// How far into the run its last item finished.
    pub stopped_at: Duration,
    /// `MaxConcurrency` as the state resolved it; `0` is "no limit", which is the
    /// member's own default.
    pub max_concurrency: i32,
    /// `ToleratedFailureCount` as the state resolved it.
    pub tolerated_failure_count: i64,
    /// `ToleratedFailurePercentage` as the state resolved it.
    pub tolerated_failure_percentage: f32,
    /// How many items the state iterated.
    pub total_items: i64,
    /// How each item the state iterated turned out, in the order the iterations
    /// finished: what the Map Run counts an item under, and from which offset.
    pub items: Vec<MapItemOutcome>,
    /// The child execution each item of the state was run as, in the order the
    /// iterations were started: what `ListExecutions` reports for this Map Run.
    pub children: Vec<StartedChildExecution>,
}

/// One child workflow execution a Distributed Map state started: the execution ONE
/// of its items ran as.
///
/// A Distributed Map "starts a child workflow execution for each item", so each item
/// is an execution of its own — with its own ARN, which that item's states read
/// through `$$.Execution.Id`, the `ItemSelector` projection as its own input and
/// what its iteration produced as its own output. The offsets are into the RUN (the
/// total of every `Wait` the walk passed), so the caller that stores this turns them
/// into instants against the parent execution's own start — the walk never sleeps.
#[derive(Debug, Clone)]
pub struct StartedChildExecution {
    /// The execution name, which the ARN below carries as its last field.
    pub name: String,
    /// The child's own execution ARN, under the state machine the Map Run runs and
    /// the Map state's label.
    pub arn: String,
    /// "The string that contains the JSON input data" of this execution: the
    /// `ItemSelector` projection of its item, or the item itself where the state
    /// declares no selector.
    pub input: String,
    /// What its iteration produced, for a child that SUCCEEDED.
    pub output: Option<String>,
    /// The error and cause its iteration failed with, for one that did not.
    pub error: Option<String>,
    pub cause: Option<String>,
    /// `SUCCEEDED` or `FAILED`, as its own iteration ended.
    pub status: &'static str,
    /// How far into the run its iteration started, and how far into the run it
    /// finished.
    pub started_at: Duration,
    pub finished_at: Duration,
    /// "The total number of items processed in a child workflow execution": one,
    /// because every child this emulator starts is handed exactly one item of the
    /// state's array (`ItemBatcher`, which batches several into one child, is not
    /// interpreted — see the module docs).
    pub item_count: i32,
}

/// What one walk of a definition produced.
#[derive(Debug, Clone)]
pub struct Run {
    /// `SUCCEEDED`, `FAILED` or `TIMED_OUT` — or `RUNNING`, when the walk reached an
    /// activity task and [`Run::waiting`] carries it: the execution has no outcome
    /// yet, because the next thing that happens to it is a worker's reply.
    pub status: &'static str,
    pub output: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub steps: Vec<Step>,
    /// The offset into the run at which the outcome became due: the total of every
    /// `Wait` the walk passed.
    pub elapsed: Duration,
    /// The activity task the walk SUSPENDED on, when it reached one. A run that
    /// carries this reached no terminal status: the task is what the execution is
    /// waiting for.
    pub waiting: Option<ScheduledActivity>,
    /// The run-a-job integration's job the walk PARKED at, when it started one. A run
    /// that carries this reached no terminal status either: what the execution is
    /// waiting for is that job finishing.
    pub parked: Option<ParkedJob>,
    /// The Map Run each Distributed Map state the walk entered started, in the order
    /// they were entered.
    pub map_runs: Vec<StartedMapRun>,
}

impl Run {
    /// This run, with the Map Runs its Distributed Map states started.
    fn with_map_runs(mut self, map_runs: Vec<StartedMapRun>) -> Self {
        self.map_runs = map_runs;
        self
    }
}

/// The activity task a `Task` state scheduled, as the walk computed it.
///
/// The offsets are into the RUN (the total of every `Wait` the walk passed before the
/// state), so the caller that parks this turns them into instants against the
/// execution's own start — the walk never sleeps.
#[derive(Debug, Clone)]
pub struct ScheduledActivity {
    /// The activity ARN the state's `Resource` named, which is the queue a worker
    /// polls with `GetActivityTask`.
    pub activity_arn: String,
    /// The state that scheduled it, so the reply resumes the execution there.
    pub state_name: String,
    /// "The string that contains the JSON input data for the task": the state's
    /// effective input, after `InputPath` and the `Parameters` payload template.
    pub input: Value,
    /// The RAW input of the state that scheduled it, before `InputPath` and
    /// `Parameters`: what the state's `ResultPath` combines a worker's reply with, so
    /// the reply can be placed when it arrives rather than guessed at.
    pub raw_input: Value,
    /// How far into the run the state scheduled the task.
    pub at: Duration,
    /// `TimeoutSeconds`: "If the task runs longer than the specified seconds, this
    /// state fails with a States.Timeout error name."
    pub timeout_seconds: Option<u64>,
    /// `HeartbeatSeconds`: "If more time than the specified seconds elapses between
    /// heartbeats from the task, this state fails with a States.Timeout error name."
    pub heartbeat_seconds: Option<u64>,
}

/// The job a `Task` state's run-a-job (`.sync`) integration STARTED, which parked the
/// walk: the task is finished when that job is, so what happens next is the job
/// reaching a terminal status rather than another state of this definition.
///
/// The offset is into the RUN (the total of every `Wait` the walk passed before the
/// state), so the caller that parks this turns it into an instant against the
/// execution's own start — the walk never sleeps.
#[derive(Debug, Clone)]
pub struct ParkedJob {
    /// The state that started the job, so the job's outcome resumes the execution
    /// there.
    pub state_name: String,
    /// The RAW input of that state, before `InputPath` and `Parameters`: what its
    /// `ResultPath` combines the job's own report with once the job has ended.
    pub raw_input: Value,
    /// The integration the job belongs to, and the service whose API reports and
    /// stops it.
    pub job: &'static super::integration::RunAJob,
    pub api: &'static super::integration::Api,
    /// Whether the `Resource` asked for the pattern's second revision.
    pub parsed_output: bool,
    /// The job's own identifier, as the call that started it reported it.
    pub job_id: String,
    /// What the starting call's own request said the job belongs to, for an
    /// integration whose reporting and stopping calls do not name it by its
    /// identifier alone ([`super::integration::RunAJob::naming`]).
    pub naming: super::integration::JobNaming,
    /// The document that call answered, which is what the task reports as its
    /// SUBMISSION.
    pub submitted: Value,
    /// How far into the run the state started the job.
    pub at: Duration,
    /// `TimeoutSeconds`: "If the task runs longer than the specified seconds, this
    /// state fails with a States.Timeout error name" — which for this pattern bounds
    /// how long the job may stay unfinished, because the task is not finished until
    /// it is.
    pub timeout_seconds: Option<u64>,
}

/// What the job an execution was PARKED at ended as, which is the result of the state
/// that started it.
///
/// `Ok` carries the document the job's own reporting call answered — the job did its
/// work, so that report IS the task's result — and `Err` why it ended without doing
/// it, which is the task's failure.
#[derive(Debug, Clone)]
pub struct JobReply {
    /// The state that started the job, as the parked record names it.
    pub state_name: String,
    /// The RAW input that state was entered with, as the parked record carries it.
    pub state_input: String,
    /// What the job ended as.
    pub outcome: Result<Value, Failure>,
}

/// A worker's reply to the activity task an execution SUSPENDED at.
///
/// `SendTaskSuccess` is "used by activity workers [...] to report that the task
/// identified by the taskToken completed successfully": the output it carries IS the
/// result of the state that scheduled the task, so the state's own result-side data
/// flow is applied to it and the execution continues from there rather than waiting a
/// second time.
#[derive(Debug, Clone)]
pub struct ActivityReply {
    /// The state that scheduled the task being answered, which the stored task names.
    pub state_name: String,
    /// The RAW input that state was entered with, as the stored task carries it: what
    /// the state's `ResultPath` combines the result with.
    pub state_input: String,
    /// "The JSON output of the task", as the worker reported it.
    pub result: Value,
}

/// The step a redrive continues an execution from: the state the attempt stopped at,
/// and the raw input it was entered with.
///
/// Recorded when an attempt stops at a state rather than getting past it, so a later
/// `RedriveExecution` continues "the failed execution from the unsuccessful step" with
/// the data that step saw instead of running the whole definition again.
#[derive(Debug, Clone)]
pub struct RedriveStep {
    /// The state the attempt stopped at, which the redriven attempt re-enters.
    pub state_name: String,
    /// The RAW input that state was entered with, as the attempt recorded it: what the
    /// results of the steps before it produced, so those steps are not re-run to reach
    /// them.
    pub state_input: String,
}

impl RedriveStep {
    /// The bytes this record retains, counted into the execution's own.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Self>() + self.state_name.len() + self.state_input.len()
    }
}

/// What the context object (`$$`) reports about the run being interpreted.
///
/// Every field is a value the caller's own request or the execution's own record
/// already fixed, so a definition that reads `$$.Execution.Id` reads this execution's
/// ARN rather than a placeholder.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub execution_arn: String,
    pub execution_name: String,
    pub state_machine_arn: String,
    pub state_machine_name: String,
    pub role_arn: String,
    /// How many times the execution being interpreted has been redriven, which it
    /// reads through `$$.Execution.RedriveCount`: `0` on the attempt a start began,
    /// and the count the record carries on an attempt a redrive began. It is the SAME
    /// integer `DescribeExecution` projects as `redriveCount`, because both come from
    /// the one execution record.
    pub redrive_count: i32,
    /// Whether the state machine being run is an EXPRESS workflow.
    ///
    /// Not a member of the context object a definition reads: it is the workflow type
    /// the execution's own record carries, and the walk needs it because one
    /// integration PATTERN is served only for the other workflow type — a `Task`
    /// state asking for the run-a-job (`.sync`) pattern inside an Express workflow
    /// fails at that state instead of starting a job
    /// ([`EXPRESS_RUN_A_JOB_UNSUPPORTED`]).
    pub express: bool,
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
                "RedriveCount": self.redrive_count,
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
    /// Nowhere YET: the state scheduled an activity task
    /// ([`Interpreter::waiting`]) or started the job of a run-a-job integration
    /// ([`Interpreter::parked`]), so the walk stops there and the execution waits —
    /// for the worker that polls for the task, or for the job to finish.
    Suspended,
}

/// What one walk of a state list produced.
struct Branch {
    outcome: Result<Value, Failure>,
    /// The offset into the run at which the branch ended.
    end: Duration,
}

/// One unit of a state's CONCURRENT work — a `Parallel` branch, or one iteration of a
/// `Map` state — as the walk ran it, for [`Interpreter::abort_units`].
struct Unit {
    /// Where this unit's own history events begin in the walk's list.
    from: usize,
    /// Where the Map Runs this unit's work records begin in the walk's list of them,
    /// so a failure that stops these units judges only the Map Runs THEY started and
    /// leaves the ones recorded before they began alone.
    map_runs_from: usize,
    /// The event this unit records when the state's failure stopped it, for a kind of
    /// unit the language records one for: a `Map` iteration is aborted in its own
    /// right, while a `Parallel` branch is reported through the states it was in.
    aborted: Option<Step>,
}

/// The suffix of the event type every state records when it is ENTERED, and the one it
/// records when it is LEFT: the prefix is the state's own type, which is what says
/// which aborted event an in-flight state records
/// ([`aborted_event`]).
const STATE_ENTERED: &str = "StateEntered";
const STATE_EXITED: &str = "StateExited";

/// The event a state of a given type records when the work around it was aborted,
/// where the service models one.
///
/// Only a state that can still be RUNNING when the abort reaches it has one:
/// `HistoryEventType` models `WaitStateAborted`, `TaskStateAborted`, `MapStateAborted`
/// and `ParallelStateAborted` and no others
/// (`smithy_query(query="shape",shape="HistoryEventType")`), because a `Pass`, a
/// `Choice`, a `Succeed` and a `Fail` leave in the same instant they are entered.
fn aborted_event(state_type: &str) -> Option<&'static str> {
    match state_type {
        "Wait" => Some("WaitStateAborted"),
        "Task" => Some("TaskStateAborted"),
        "Map" => Some("MapStateAborted"),
        "Parallel" => Some("ParallelStateAborted"),
        _ => None,
    }
}

/// The `$$.Map.Item` of the enclosing `Map` iteration, if the walk is inside one.
#[derive(Debug, Clone, Default)]
struct Scope {
    map_item: Option<Value>,
}

/// A `Map` state's plan: everything resolved from its members before its items run.
struct MapPlan<'a> {
    items: &'a [Value],
    /// The state's own name: "the name of the iteration's parent Map state", which
    /// every one of its iterations' events reports.
    name: &'a str,
    /// The state's `Label`, else its own name: the field the Map Run's ARN and its
    /// children's own ARNs carry.
    label: &'a str,
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
/// The codes below are the ones this emulator's validation can reach, out of
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

/// `UNSUPPORTED_JSONATA_EXPRESSION`: the definition is the shape the language
/// declares, but one of its JSONata expressions is not one the service accepts —
/// a reserved term read where the language does not bind it, which is a problem with
/// the EXPRESSION rather than with the shape of the document around it. The model's
/// own list of codes is not closed ("might add new diagnostics in the future"), so a
/// code it does not list is still a code the service reports.
pub const UNSUPPORTED_JSONATA_EXPRESSION: &str = "UNSUPPORTED_JSONATA_EXPRESSION";

/// `MISSING_TRANSITION_TARGET`: "The value of \"Next\" field doesn't match a known
/// state name" — reported for every transition target that names no declared state,
/// `StartAt`'s included.
pub const MISSING_TRANSITION_TARGET: &str = "MISSING_TRANSITION_TARGET";

/// `INVALID_STATE_NAME`: "The state name does not follow the naming convention".
pub const INVALID_STATE_NAME: &str = "INVALID_STATE_NAME";

/// `MISSING_END_STATE`: "The workflow does not have a terminal state" — no state the
/// state machine DECLARES ends an execution, so nothing it can run ever stops.
pub const MISSING_END_STATE: &str = "MISSING_END_STATE";

/// `MISSING_DESCRIPTION`: "Received a null or empty workflow input" — the request
/// carried no document to validate at all.
pub const MISSING_DESCRIPTION: &str = "MISSING_DESCRIPTION";

/// `TOO_DEEPLY_NESTED`: "The states are too deeply nested".
pub const TOO_DEEPLY_NESTED: &str = "TOO_DEEPLY_NESTED";

/// `NO_DOLLAR`: "No `.$` on a field that appears to be a JSONPath or Intrinsic
/// Function" — a WARNING, because the field is rendered as the literal it is
/// rather than refused.
pub const NO_DOLLAR: &str = "NO_DOLLAR";

/// `NO_PATH`: "Field value looks like a path, but field name does not end with
/// 'Path'" — a WARNING, for the same reason.
pub const NO_PATH: &str = "NO_PATH";

/// `PASS_RESULT_IS_STATIC`: "Attempt to use a path in the result of a pass state"
/// — a WARNING: a `Pass` state's `Result` is its literal result.
pub const PASS_RESULT_IS_STATIC: &str = "PASS_RESULT_IS_STATIC";

/// `POSSIBLY_UNDEFINED_VARIABLE`: a field reads a variable that some run of the
/// definition can reach that field without having assigned — a WARNING, because the
/// read resolves to nothing rather than preventing the definition from being created.
/// The model's list of codes is not closed ("might add new diagnostics in the
/// future"), so a code it does not list is still a code the service reports.
pub const POSSIBLY_UNDEFINED_VARIABLE: &str = "POSSIBLY_UNDEFINED_VARIABLE";

/// The most diagnostics `ValidateStateMachineDefinition` reports in one call, which
/// is also what it reports when the request asks for none in particular
/// (`maxResults`: "The maximum number of diagnostics that are returned per call.
/// The default and maximum value is 100. Setting the value to 0 will also use the
/// default of 100.").
pub const DEFAULT_MAX_DIAGNOSTICS: usize = 100;

/// How many diagnostics of ONE severity a walk collects.
///
/// A definition is caller-sized (the modelled `Definition` is 1 MiB), so the list
/// this walk builds carries its own ceiling like every other walk here. It is one
/// past [`DEFAULT_MAX_DIAGNOSTICS`], which is `maxResults`' modelled maximum: no
/// request can ask for more than that, so one more than the maximum is all it takes
/// to report `truncated`.
///
/// The budget is per SEVERITY, and the walk stops only once the ERROR budget is
/// spent: a definition's warnings can therefore never crowd out an error, which is
/// what [`validate`] — and so what `CreateStateMachine` refuses — reads.
const MAX_DIAGNOSTICS: usize = DEFAULT_MAX_DIAGNOSTICS + 1;

/// The most characters of a caller's own value one diagnostic echoes.
///
/// A definition is caller-sized and its member names, state names and string values
/// are caller-chosen, so a message that echoed one whole would let one request be
/// amplified into a much larger response. The cap is the longest a state name may
/// be ([`MAX_STATE_NAME_CHARS`]), so a value inside the language's own bounds is
/// quoted in full and only one past them is clipped.
const MAX_ECHOED_CHARS: usize = MAX_STATE_NAME_CHARS;
/// The most characters one diagnostic's `location` carries where that location is a
/// PATH into the definition, and the most any path the walk builds carries: the path
/// to a state nests a caller-chosen name per level, so it is clipped AS IT IS BUILT
/// ([`nested_at`]) rather than only where it is reported.
///
/// ONE diagnostic is located somewhere else: a definition that is not JSON at all has
/// no path into it, so it is located at the position the parse ended at
/// ([`crate::state::json_parse_definition_location`]), whose own quoting of the
/// document is what bounds it.
pub const MAX_LOCATION_CHARS: usize = 256;

/// `value` as a diagnostic echoes it: itself when it is short, and its first
/// [`MAX_ECHOED_CHARS`] characters followed by an ellipsis when it is not.
///
/// Public so the one clipping of a quoted caller value serves every cause this
/// crate words, including the ones [`super::integration`] builds for a `Task`
/// state's `Resource`.
pub fn echoed(value: &str) -> String {
    clipped(value, MAX_ECHOED_CHARS)
}

/// `value` clipped to `cap` characters, with an ellipsis marking what was dropped.
///
/// The ellipsis is one of the `cap`, so the result never carries more characters
/// than the cap allows.
///
/// Not [`clip_cause`], which is the other truncation this module does and a
/// different one: that cuts a `cause` to the BYTES the modelled `SensitiveCause`
/// carries and must leave the text it keeps exactly as the run produced it, since
/// it is a reported value rather than a quotation of one.
fn clipped(value: &str, cap: usize) -> String {
    if value.chars().count() <= cap {
        return value.to_string();
    }
    let kept: String = value.chars().take(cap.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

/// One problem found in a definition: what it is, where it is, and whether it
/// prevents creating or updating a state machine with that definition.
///
/// `severity` is [`crate::state::SEVERITY_ERROR`] or
/// [`crate::state::SEVERITY_WARNING`], `code` one of the codes above, and
/// `location` the `/States/<StateName>/<FieldName>` form the `location` member of
/// `ValidateStateMachineDefinitionDiagnostic` documents, for a diagnostic a field
/// or a state owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: &'static str,
    pub code: &'static str,
    pub message: String,
    pub location: Option<String>,
}

impl Diagnostic {
    /// Whether this diagnostic prevents a create or an update: "A value of `ERROR`
    /// means that you cannot create or update a state machine with this
    /// definition."
    pub fn is_error(&self) -> bool {
        self.severity == crate::state::SEVERITY_ERROR
    }

    /// This diagnostic as the SUBJECT of an `InvalidDefinition` message: the code
    /// that classifies it, the detail describing it, and where it was found.
    ///
    /// The place is joined to the detail by the bare word ` at ` and nothing else, so
    /// the subject reads as one sentence and a client that parses the location out of
    /// a refusal finds it unwrapped.
    fn stated(&self) -> String {
        match &self.location {
            Some(location) => format!("{}: {} at {location}", self.code, self.message),
            None => format!("{}: {}", self.code, self.message),
        }
    }
}

/// One diagnostic of the given severity.
fn diagnostic(
    severity: &'static str,
    code: &'static str,
    location: Option<String>,
    message: impl ::std::fmt::Display,
) -> Diagnostic {
    Diagnostic {
        severity,
        code,
        message: message.to_string(),
        location,
    }
}

/// One ERROR diagnostic: a definition carrying it cannot be created or updated.
fn error(code: &'static str, location: Option<String>, message: impl ::std::fmt::Display) -> Diagnostic {
    diagnostic(crate::state::SEVERITY_ERROR, code, location, message)
}

/// One WARNING diagnostic: a definition carrying it is still created.
fn warning(code: &'static str, location: Option<String>, message: impl ::std::fmt::Display) -> Diagnostic {
    diagnostic(crate::state::SEVERITY_WARNING, code, location, message)
}

/// Where one diagnostic was found: the enclosing machine's own location followed
/// by `member`, or `None` for the definition as a whole.
///
/// `prefix` is itself bounded (every caller builds one through [`nested_at`]), so
/// the only caller-sized part of a location is the one member named here.
fn at(prefix: &str, member: &str) -> Option<String> {
    let location = format!("{prefix}{member}");
    (!location.is_empty()).then(|| clipped(&location, MAX_LOCATION_CHARS))
}

/// The location to hand to something nested inside `prefix`: `prefix` followed by
/// `member`, clipped AS IT IS BUILT.
///
/// The path to a state nests one caller-chosen name per level and is then built
/// once per state, so a path that grew with the definition would make the work of
/// one request the PRODUCT of the state count and the nesting; clipping here is
/// what keeps every path handed down O([`MAX_LOCATION_CHARS`]).
fn nested_at(prefix: &str, member: impl ::std::fmt::Display) -> String {
    clipped(&format!("{prefix}{member}"), MAX_LOCATION_CHARS)
}

/// Where a diagnostic about the state machine OBJECT itself was found: the object's
/// own location, which for the definition as a whole is the document's root.
///
/// [`at`] answers `None` for the root, because a diagnostic about a MEMBER of the
/// root carries that member's name; a diagnostic about the object carries the object,
/// and the root object is `/`.
fn machine_at(prefix: &str) -> Option<String> {
    Some(if prefix.is_empty() {
        "/".to_string()
    } else {
        clipped(prefix, MAX_LOCATION_CHARS)
    })
}

/// The fields the Amazon States Language declares on a state machine object, in the
/// order its schema states them.
///
/// From the language specification's "State Machine Structure": a state machine
/// carries `Comment`, `StartAt`, `TimeoutSeconds`, `Version`, `States` and
/// `QueryLanguage`. The set is CLOSED — a field outside it is a field the language
/// does not define — which is what [`field_not_supported_message`] reports.
const MACHINE_FIELDS: &[&str] = &[
    "Comment",
    "QueryLanguage",
    "StartAt",
    "States",
    "TimeoutSeconds",
    "Version",
];

/// The fields a state machine declared as a `Map` state's processor carries: the
/// machine's own, and the `ProcessorConfig` that says how that state runs it.
///
/// A processor is the one position the language declares that field at, so a
/// definition or a `Parallel` branch carrying it is carrying a field the language
/// does not declare THERE — which is what [`machine_fields`] keys on.
const PROCESSOR_FIELDS: &[&str] = &[
    "Comment",
    "ProcessorConfig",
    "QueryLanguage",
    "StartAt",
    "States",
    "TimeoutSeconds",
    "Version",
];

/// Which closed set a state machine object's fields are judged against: a `Map`
/// state's processor admits one field more than a definition or a branch does.
fn machine_fields(processor: bool) -> &'static [&'static str] {
    if processor {
        PROCESSOR_FIELDS
    } else {
        MACHINE_FIELDS
    }
}

/// The fields a state machine object must carry, in the order the refusal lists them.
///
/// "A State Machine MUST have an object field named `States`" and "a string field
/// named `StartAt`", which are the two the schema requires; the order here is the
/// order the service lists them in, and it is the LIST that is reported rather than
/// one diagnostic per field.
const MACHINE_REQUIRED_FIELDS: &[&str] = &["States", "StartAt"];

/// What a schema diagnostic says about required fields an object does not carry: the
/// fields, in brackets, as a list.
///
/// RECORDED text. One diagnostic names them all, so a caller is told everything the
/// object owes rather than one field per attempt.
fn required_fields_message(fields: &[&str]) -> String {
    format!("These fields are required: [{}]", fields.join(", "))
}

/// What a schema diagnostic says about a field the language does not declare: the
/// field, quoted, and that it is not supported.
///
/// RECORDED text. The field name is the caller's own JSON key, so it is clipped by
/// [`echoed`] like every other caller value a diagnostic quotes.
fn field_not_supported_message(field: &str) -> String {
    format!("Field '{}' is not supported", echoed(field))
}

/// What a diagnostic says about a transition target no state declares: the member
/// that names the target, and the name it named.
///
/// RECORDED text. One construction serves every member a transition is named
/// through — a state's own `Next`, a `Choice` rule's, a `Catch` catcher's and the
/// `Default` beside them, and the machine's own `StartAt` — because they are one
/// problem: a name that resolves to no state of the enclosing `States` map. The
/// target is the caller's own string, so it is clipped by [`echoed`].
fn missing_target_message(member: &str, target: &str) -> String {
    format!("Missing '{member}' target: {}", echoed(target))
}

/// What the diagnostic for a state machine that cannot stop says.
///
/// RECORDED text. It names no state and no member: what is missing is a terminal
/// state anywhere in the machine, so there is no position in the document to place it
/// at.
const MISSING_END_STATE_MESSAGE: &str = "Workflow has no terminal state";

/// What a diagnostic says about a variable a field reads and nothing reaching that
/// field assigns: the variable, quoted.
///
/// RECORDED text. The name is the caller's own, so it is clipped by [`echoed`].
fn possibly_undefined_variable_message(name: &str) -> String {
    format!("Variable '{}' is possibly not defined.", echoed(name))
}

/// Everything ONE walk of a definition found: the diagnostics it reports, and
/// whether it had to drop any of them.
///
/// The walk collects at most [`MAX_DIAGNOSTICS`] of each severity, so a caller is
/// told that more exist (`truncated`) rather than being handed a list that grows
/// with the definition.
#[derive(Debug, Clone)]
pub struct Diagnosis {
    /// What was found, in the order it was found.
    pub diagnostics: Vec<Diagnostic>,
    /// Whether an ERROR was found and not reported, the error budget being spent.
    pub errors_dropped: bool,
    /// Whether a WARNING was found and not reported, that budget being spent.
    pub warnings_dropped: bool,
    /// How many of each severity have been reported, which is what spends each
    /// budget.
    errors: usize,
    warnings: usize,
    /// Whether this walk reports no WARNING at all — what reads it ([`validate`],
    /// [`validate_single_state`]) refuses a definition for its ERRORS, so a warning
    /// would be built only to be discarded.
    errors_only: bool,
    /// Whether this walk reads the FIRST error and nothing else, so there is no reason
    /// to keep walking past that error.
    first_error_only: bool,
    /// What is LEFT of this diagnosis's variable-scope budget
    /// ([`MAX_SCOPE_VISITS`]), spent by every state that analysis visits, in this
    /// definition and in every machine nested in it.
    scope_visits: usize,
}

/// A walk with every budget of it unspent.
impl Default for Diagnosis {
    fn default() -> Self {
        Self {
            diagnostics: Vec::new(),
            errors_dropped: false,
            warnings_dropped: false,
            errors: 0,
            warnings: 0,
            errors_only: false,
            first_error_only: false,
            scope_visits: MAX_SCOPE_VISITS,
        }
    }
}

impl Diagnosis {
    /// A walk that stops at the first ERROR, for [`validate_single_state`]: no warning
    /// is built and nothing past that error is looked at. What it reports about a
    /// definition is the same verdict a full walk reports, because an error is an error
    /// wherever in the definition it was found.
    fn until_first_error() -> Self {
        Self {
            errors_only: true,
            first_error_only: true,
            ..Self::default()
        }
    }

    /// A walk that reports every ERROR and no warning, for [`validate`]: the refusal it
    /// words states one subject per error the definition carries, and a warning is not
    /// one of them.
    fn errors_only() -> Self {
        Self {
            errors_only: true,
            ..Self::default()
        }
    }

    /// Whether the definition carries a problem that prevents creating or updating
    /// a state machine with it — what `result` reports as `FAIL`.
    pub fn failed(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }

    /// Whether a diagnostic of a severity this request reports was dropped, so the
    /// reported list is short of what the definition carries.
    pub fn dropped(&self, severity: &str) -> bool {
        self.errors_dropped
            || (severity == crate::state::SEVERITY_WARNING && self.warnings_dropped)
    }

    /// Record one diagnostic, or that its severity's budget is spent.
    fn push(&mut self, found: Diagnostic) {
        let (reported, dropped) = if found.is_error() {
            (&mut self.errors, &mut self.errors_dropped)
        } else {
            (&mut self.warnings, &mut self.warnings_dropped)
        };
        if *reported >= MAX_DIAGNOSTICS || (self.errors_only && !found.is_error()) {
            *dropped = true;
            return;
        }
        *reported += 1;
        self.diagnostics.push(found);
    }

    /// Whether this walk reports WARNINGS at all: the lints that cost a walk of their
    /// own are built only where one can be reported.
    fn warnings_wanted(&self) -> bool {
        !self.errors_only
    }

    /// Spend `visits` states of the variable-scope budget, or report that what is left
    /// does not cover them.
    ///
    /// The budget is the ONE this diagnosis has for every machine it walks, so the
    /// analysis's cost cannot be the product of the machines a definition declares and
    /// the states each of them holds.
    fn spend_scope_visits(&mut self, visits: usize) -> bool {
        match self.scope_visits.checked_sub(visits) {
            Some(left) => {
                self.scope_visits = left;
                true
            }
            None => false,
        }
    }

    /// Whether the walk keeps looking: it stops once it can report no further ERROR
    /// — its error budget spent, or, for a [`validate_single_state`], the one error it
    /// reads already found. Every error past that point would change neither the result
    /// nor the reported list.
    fn looking(&self) -> bool {
        !(self.errors_dropped || (self.first_error_only && self.errors > 0))
    }
}

/// What separates the subjects of an `InvalidDefinition` message: a definition
/// carrying several problems is refused with one subject per problem, written one
/// after another in the order the walk found them.
const SUBJECT_SEPARATOR: &str = ", ";

/// Why `definition` is not a definition this service accepts, or `Ok` when it is.
///
/// These are the Amazon States Language's own structural rules, so the refusal is
/// `CreateStateMachine`'s modelled `InvalidDefinition`: the definition is a JSON
/// object carrying `StartAt` and a non-empty `States` map, `StartAt` names a declared
/// state, every state is an object with a string `Type`, every `Next` names a
/// declared state, a terminal state carries no `Next`, and a continuing state carries
/// `Next` or `"End": true`.
///
/// The `Err` is the SUBJECT of that error's message, which the operation states
/// through [`crate::state::resource_error_message`] with
/// [`crate::state::INVALID_STATE_MACHINE_DEFINITION`]: ONE subject per error
/// [`diagnose`] found, each `<code>: <detail> at <location>` ([`Diagnostic::stated`]),
/// separated by [`SUBJECT_SEPARATOR`]. Every error the definition carries is stated,
/// so a caller fixing a definition is told all of them rather than one per attempt,
/// and what a create refuses a definition for cannot disagree with what
/// `ValidateStateMachineDefinition` reports `FAIL` for.
///
/// The message is bounded by the walk itself rather than by the definition: the walk
/// reports at most [`MAX_DIAGNOSTICS`] errors, each one's quoted value is capped by
/// [`MAX_ECHOED_CHARS`] and its location by [`MAX_LOCATION_CHARS`], so a caller-sized
/// definition cannot be amplified into a larger refusal.
///
/// A definition may use states this emulator does not EXECUTE and still be valid,
/// because it is valid ASL: what cannot be run surfaces at execution time, as a
/// `FAILED` execution, not as a rejected create.
pub fn validate(definition: &str) -> Result<(), String> {
    let stated: Vec<String> = collect(definition, Diagnosis::errors_only())
        .diagnostics
        .iter()
        .filter(|found| found.is_error())
        .map(Diagnostic::stated)
        .collect();
    if stated.is_empty() {
        return Ok(());
    }
    Err(stated.join(SUBJECT_SEPARATOR))
}

/// Every problem `definition` carries: the errors that would prevent creating or
/// updating a state machine with it, and the warnings that would not.
///
/// This is what `ValidateStateMachineDefinition` reports — "Validation will look
/// for problems in your state machine definition and return a result and a list of
/// diagnostic elements" — and, through [`validate`], what `CreateStateMachine` and
/// `UpdateStateMachine` refuse a definition for.
///
/// The rules are applied to EVERY state list the definition declares, not only the
/// top-level one: a `Map` state's `ItemProcessor`/`Iterator` and a `Parallel` state's
/// `Branches` each declare their own state machine, whose state names reach the same
/// execution history and whose states inherit the enclosing query language.
///
/// The list is bounded by [`MAX_DIAGNOSTICS`] per severity; a definition carrying
/// more problems than that is reported as truncated rather than reported in full.
///
/// The walk costs the same ORDER as the parse that precedes it: it visits each
/// member once, every path it builds is bounded by [`MAX_LOCATION_CHARS`] and every
/// value it quotes by [`MAX_ECHOED_CHARS`], so no cost that is the product of the
/// state count and the nesting can be built from a caller's definition.
pub fn diagnose(definition: &str) -> Diagnosis {
    collect(definition, Diagnosis::default())
}

/// [`diagnose`] into the walk `found` describes: every problem, only the errors — or,
/// for [`validate_single_state`], the first error and nothing else.
fn collect(definition: &str, mut found: Diagnosis) -> Diagnosis {
    if definition.trim().is_empty() {
        found.push(error(
            MISSING_DESCRIPTION,
            None,
            "the request carried no definition to validate",
        ));
        return found;
    }
    match serde_json::from_str::<Value>(definition) {
        Ok(document) => diagnose_machine(
            &document,
            "",
            JSONPATH,
            1,
            false,
            Outer::Nothing,
            &mut found,
        ),
        // The diagnostic is the service's own, naming where the definition stopped
        // parsing, the mark it left unmatched and what was expected there — the one
        // construction every document this service reads is reported through — and
        // it is LOCATED at the position in the definition the parse ended at, which
        // is where a client has to look.
        Err(why) => found.push(error(
            INVALID_JSON_DESCRIPTION,
            Some(crate::state::json_parse_definition_location(definition, &why)),
            crate::state::json_parse_definition_diagnostic(definition, &why),
        )),
    }
    found
}

/// [`diagnose`] for one state machine — the definition itself, or one nested inside
/// it — carrying where it was found (`prefix`), the query language it inherits from
/// the state that declares it, how deeply it is nested, and whether it is a `Map`
/// state's PROCESSOR, which admits one field more than a definition or a `Parallel`
/// branch does ([`machine_fields`]), and what was already in scope where it is
/// declared ([`Outer`]).
fn diagnose_machine(
    document: &Value,
    prefix: &str,
    inherited: &str,
    depth: usize,
    processor: bool,
    outer: Outer<'_>,
    into: &mut Diagnosis,
) {
    if !into.looking() {
        return;
    }
    if depth > MAX_DEFINITION_DEPTH {
        into.push(error(
            TOO_DEEPLY_NESTED,
            machine_at(prefix),
            format!(
                "the definition nests state machines more than the {MAX_DEFINITION_DEPTH} levels \
                 this service reads"
            ),
        ));
        return;
    }
    let Some(document) = document.as_object() else {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            machine_at(prefix),
            "a state machine definition must be a JSON object",
        ));
        return;
    };
    // `QueryLanguage` selects which members the states below are written with, so
    // it is read before them; a definition that names neither language is written
    // in the one `QueryLanguage` defaults to.
    let language = match declared_language(document.get("QueryLanguage")) {
        Ok(declared) => declared.unwrap_or(inherited),
        Err(why) => {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(prefix, "/QueryLanguage"),
                why,
            ));
            inherited
        }
    };
    // The language's schema for a state machine object is CLOSED: it declares the
    // fields a definition may carry ([`MACHINE_FIELDS`]) and the two it must
    // ([`MACHINE_REQUIRED_FIELDS`]). Fields the object does not carry are reported
    // TOGETHER, in one diagnostic that lists them, and each field the schema does
    // not declare is reported on its own — both located at the OBJECT, which is
    // what a field belongs to, rather than at a position the document has not got.
    let object = machine_at(prefix);
    let missing: Vec<&str> = MACHINE_REQUIRED_FIELDS
        .iter()
        .copied()
        .filter(|field| !document.contains_key(*field))
        .collect();
    if !missing.is_empty() {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            object.clone(),
            required_fields_message(&missing),
        ));
    }
    for field in document.keys() {
        if !machine_fields(processor).contains(&field.as_str()) {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                object.clone(),
                field_not_supported_message(field),
            ));
        }
    }
    let states = document.get("States").and_then(Value::as_object);
    // A field the object DOES carry is judged on its own: that it is absent is
    // already stated above, so what is left to say about it is its value.
    let mut start_at: Option<&str> = None;
    if let Some(declared) = document.get("StartAt") {
        match declared.as_str() {
            Some(declared) => {
                start_at = Some(declared);
                if states.is_some_and(|states| !states.contains_key(declared)) {
                    into.push(error(
                        MISSING_TRANSITION_TARGET,
                        at(prefix, "/StartAt"),
                        missing_target_message("StartAt", declared),
                    ));
                }
            }
            None => into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(prefix, "/StartAt"),
                "a state machine definition must carry a string 'StartAt' member",
            )),
        }
    }
    let Some(states) = states else {
        if document.contains_key("States") {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(prefix, "/States"),
                "a state machine definition must carry an object 'States' member",
            ));
        }
        return;
    };
    if states.is_empty() {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(prefix, "/States"),
            "'States' must declare at least one state",
        ));
        return;
    }
    // Which variables each state of THIS machine can count on having been assigned
    // by the time it runs, which is what its own reads are judged against. Built
    // once per machine, before any state is judged, because it is a property of the
    // whole graph rather than of one state; only a walk that reports WARNINGS needs
    // it ([`POSSIBLY_UNDEFINED_VARIABLE`] is one).
    let scopes = if into.warnings_wanted() {
        Scopes::of(states, start_at, outer, into)
    } else {
        Scopes::none()
    };
    // Whether the machine declares a state that ENDS an execution, read before the
    // states are judged and reported after them: the diagnostic is about the machine,
    // so it follows the ones about its states.
    let terminal = states.values().any(ends_an_execution);
    for (name, state) in states {
        if !into.looking() {
            break;
        }
        diagnose_state(
            name,
            state,
            &Machine {
                states,
                language,
                prefix,
                depth,
            },
            &scopes,
            into,
        );
    }
    // "MISSING_END_STATE: The workflow does not have a terminal state." A state
    // machine is a graph that must be able to STOP, so at least one of the states it
    // declares ends an execution; one that declares none describes a workflow no run
    // of it can leave. It is read BEFORE the states are judged and reported AFTER
    // them, so it follows the diagnostics about them; whether there is room left to
    // report it is `push`'s own answer, as it is for every other diagnostic.
    if !terminal {
        into.push(error(
            MISSING_END_STATE,
            at(prefix, ""),
            MISSING_END_STATE_MESSAGE,
        ));
    }
}

/// What every state of ONE state machine is judged in: the state list its transition
/// targets resolve against, the query language it inherits from the machine, where
/// that machine was found and how deeply it is nested.
struct Machine<'a> {
    states: &'a Map<String, Value>,
    language: &'a str,
    prefix: &'a str,
    depth: usize,
}

/// Whether `state` ENDS an execution: a state of a type that terminates one
/// ([`TERMINAL_STATE_TYPES`]), or any state declaring `"End": true`.
///
/// This is what [`MISSING_END_STATE`] is read off, over every state a machine
/// DECLARES: what that diagnostic reports is that the workflow has no terminal state,
/// and a state machine that declares one has one — where this definition's
/// transitions lead is a different question, and no diagnostic this service reports is
/// about a state nothing reaches.
fn ends_an_execution(state: &Value) -> bool {
    let Some(state) = state.as_object() else {
        return false;
    };
    if state.get("End").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    state
        .get("Type")
        .and_then(Value::as_str)
        .is_some_and(|state_type| TERMINAL_STATE_TYPES.contains(&state_type))
}

/// [`diagnose_machine`] for ONE state of a state machine: the rules the state's own
/// members must satisfy, then the state machines nested inside it.
///
/// `scopes` is its machine's variable-scope analysis, which answers what every run
/// reaching this state has already assigned — what its own reads are judged against.
fn diagnose_state<'a>(
    name: &'a str,
    state: &Value,
    machine: &Machine<'_>,
    scopes: &'a Scopes<'a>,
    into: &mut Diagnosis,
) {
    let Machine {
        states,
        language: inherited,
        prefix,
        depth,
    } = *machine;
    // The name the state is DECLARED under, which is what its machine's analysis
    // answers for; the one quoted below is the clipped one.
    let declared = name;
    // A state name is a caller-chosen JSON object key and lands in every history
    // event of that state, so its LENGTH is bounded here, not only the number of
    // events: this is what keeps a history's bytes bounded. The name a message
    // quotes, and the one the location carries, is the clipped one, for the reason
    // [`echoed`] and [`nested_at`] record; the length checked and reported is the
    // DECLARED one.
    let declared_chars = name.chars().count();
    let shown = echoed(name);
    let name = shown.as_str();
    let here = nested_at(prefix, format_args!("/States/{name}"));
    if declared_chars > MAX_STATE_NAME_CHARS {
        into.push(error(
            INVALID_STATE_NAME,
            at(&here, ""),
            format!(
                "a state name may carry at most {MAX_STATE_NAME_CHARS} characters, and this \
                 definition declares one of {declared_chars}"
            ),
        ));
    }
    let Some(state) = state.as_object() else {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&here, ""),
            format!("state '{name}' must be a JSON object"),
        ));
        return;
    };
    let Some(state_type) = state.get("Type").and_then(Value::as_str) else {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&here, "/Type"),
            format!("state '{name}' must carry a string 'Type' member"),
        ));
        return;
    };
    // `Type` selects one of the state types the Amazon States Language DEFINES
    // ([`STATE_TYPES`]), so a state naming any other one describes no state at all:
    // nothing says what it reads, what it writes or where it goes next, which is why
    // the walk stops here rather than judging the state's other members under rules
    // that are keyed on its type.
    if !STATE_TYPES.contains(&state_type) {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&here, "/Type"),
            format!(
                "state '{name}' is of type '{}', which is not one of the state types the \
                 Amazon States Language defines ({})",
                echoed(state_type),
                STATE_TYPES.join(", ")
            ),
        ));
        return;
    }
    // A `Fail` state's `Error` and `Cause` are reported verbatim by
    // `DescribeExecution`, through shapes with their own maxima, so a
    // definition carrying a longer one could never be reported.
    for (member, cap) in [("Error", MAX_ERROR_BYTES), ("Cause", MAX_CAUSE_BYTES)] {
        if let Some(value) = state.get(member).and_then(Value::as_str) {
            if value.len() > cap {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&here, &format!("/{member}")),
                    format!(
                        "state '{name}' carries a '{member}' of {} bytes, more than the {cap} \
                         this service reports",
                        value.len()
                    ),
                ));
            }
        }
    }
    // Every member the state names a transition through — its own `Next`, the
    // `Default` of a `Choice`, each `Choice` rule's own `Next` and each `Catch`
    // catcher's `Next` — resolves by ONE rule: the name must be one the enclosing
    // `States` map declares. Each is reported at the member that names it.
    for transition in transitions(state) {
        if !states.contains_key(transition.target.as_str()) {
            into.push(error(
                MISSING_TRANSITION_TARGET,
                at(&here, &transition.path),
                missing_target_message(transition.member, &transition.target),
            ));
        }
    }
    match state.get("Next") {
        Some(next) => match next.as_str() {
            Some(_) => {
                if TERMINAL_STATE_TYPES.contains(&state_type) {
                    into.push(error(
                        SCHEMA_VALIDATION_FAILED,
                        at(&here, "/Next"),
                        format!(
                            "state '{name}' is a '{}' state, which ends an execution \
                             and must not carry 'Next'",
                            echoed(state_type)
                        ),
                    ));
                }
            }
            None => into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(&here, "/Next"),
                format!("state '{name}' has a non-string 'Next' member"),
            )),
        },
        None => {
            if CONTINUING_STATE_TYPES.contains(&state_type)
                && state.get("End").and_then(Value::as_bool) != Some(true)
            {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&here, ""),
                    format!(
                        "state '{name}' is a '{}' state, so it must carry either \
                         'Next' or \"End\": true",
                        echoed(state_type)
                    ),
                ));
            }
        }
    }
    // A state may be written in a query language of its own — but only in the
    // direction the service allows: a JSONPath state machine may hold a JSONata
    // state, a JSONata one may not hold a JSONPath state.
    let language = match declared_language(state.get("QueryLanguage")) {
        Ok(Some(declared)) => {
            if declared == JSONPATH && inherited == JSONATA {
                // The service blames the overriding STATE rather than the member it
                // declares the override in: the state is the thing a state machine of
                // this query language may not hold. Which state that is, is reported
                // by `location` alone — the sentence is about the member and is the
                // same for every state that carries it.
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&here, ""),
                    format!(
                        "'QueryLanguage' can not be '{JSONPATH}' if set to '{JSONATA}' \
                         for whole state machine"
                    ),
                ));
            }
            declared
        }
        Ok(None) => inherited,
        Err(why) => {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(&here, "/QueryLanguage"),
                why,
            ));
            inherited
        }
    };
    if language == JSONATA {
        diagnose_jsonata_state(name, state, state_type, &here, into);
    } else {
        diagnose_jsonpath_state(name, state, state_type, &here, into);
    }
    // A variable the state ASSIGNS is in scope for the state's own expressions,
    // because the language evaluates a state's assignments before the output they
    // feed; everything else it may read is what reaching this state already assigned.
    let own = assigned_variables(state);
    if into.warnings_wanted() {
        // POSSIBLY_UNDEFINED_VARIABLE: one diagnostic per FIELD that reads a
        // variable nothing reaching that field assigns, which is the granularity
        // `location` documents (`/States/<StateName>/<FieldName>`).
        for (field, read) in variable_reads(state) {
            if own.contains(&read) || scopes.holds(declared, &read) {
                continue;
            }
            into.push(warning(
                POSSIBLY_UNDEFINED_VARIABLE,
                at(&here, &field),
                possibly_undefined_variable_message(&read),
            ));
        }
    }
    // What the state machines nested in this state inherit: what reaching this state
    // had in scope, asked of its machine rather than copied into each of them.
    //
    // NOT this state's own `Assign`: only a `Map` and a `Parallel` declare a machine,
    // and their own assignments are evaluated when the work those machines DO has
    // completed, so a state inside one cannot count on them.
    let outer = Outer::State {
        machine: scopes,
        state: declared,
    };
    // A nested state machine's names reach the same history as this one's, so every
    // rule above holds for it too, and its states inherit this state's language — and
    // the variables this state has in scope, which the states it declares read.
    for member in ["ItemProcessor", "Iterator"] {
        if let Some(processor) = state.get(member) {
            diagnose_machine(
                processor,
                &nested_at(&here, format_args!("/{member}")),
                language,
                depth + 1,
                // A processor is the one position the language declares
                // `ProcessorConfig` at.
                true,
                outer,
                into,
            );
        }
    }
    if let Some(branches) = state.get("Branches").and_then(Value::as_array) {
        for (index, branch) in branches.iter().enumerate() {
            diagnose_machine(
                branch,
                &nested_at(&here, format_args!("/Branches/{index}")),
                language,
                depth + 1,
                false,
                outer,
                into,
            );
        }
    }
}

/// The most distinct variables one state machine's scope analysis follows through its
/// state graph ([`Scopes`]).
///
/// Following one variable costs one walk of the graph and a definition is
/// caller-sized, so the number followed is bounded like every other dimension of this
/// walk, and the bound is the one the reported list already carries
/// ([`DEFAULT_MAX_DIAGNOSTICS`]). A variable NO state assigns needs no walk at all and
/// is not counted against it: it is out of scope wherever it is read. A variable past
/// the bound is taken to be in scope, so the bound can only leave a warning unsaid,
/// never invent one.
///
/// It is also what fits one state's scope in a single [`ScopeMask`], which is what
/// keeps the analysis from holding the PRODUCT of the state count and the variable
/// count.
const MAX_SCOPED_VARIABLES: usize = DEFAULT_MAX_DIAGNOSTICS;

/// The most states the variable-scope analysis visits across ONE diagnosis.
///
/// Following one variable costs one walk of its machine's graph, so the analysis's
/// cost is the PRODUCT of the variables it follows and the states it walks, and a
/// definition chooses both — as it chooses how many machines it nests. The product is
/// therefore spent from ONE budget for the whole diagnosis
/// ([`Diagnosis::spend_scope_visits`]) rather than bounded per machine: the budget is
/// the diagnostics one call reports ([`DEFAULT_MAX_DIAGNOSTICS`]) times the
/// transitions one execution may make ([`MAX_STATE_TRANSITIONS`]), which is the
/// largest state graph this service walks at all. Past it no further variable is
/// followed, and a variable that was not followed is taken to be in scope, so a spent
/// budget can only leave a warning unsaid.
const MAX_SCOPE_VISITS: usize = DEFAULT_MAX_DIAGNOSTICS * MAX_STATE_TRANSITIONS;

/// Which of the variables one machine's analysis followed a state can count on: one
/// BIT each, at the index [`Scopes::assigned`] records for the variable.
///
/// A set of NAMES per state would cost the product of the state count and the variable
/// count, and a definition chooses both, so a state's scope is a mask: the analysis
/// holds one of these per state and nothing that grows with the names.
type ScopeMask = u128;

/// [`MAX_SCOPED_VARIABLES`] variables must fit the mask that carries them.
const _: () = assert!(MAX_SCOPED_VARIABLES <= ScopeMask::BITS as usize);

/// The member a state assigns variables in: "Assign".
const ASSIGN_MEMBER: &str = "Assign";

/// What was already in scope where a state machine is DECLARED, which its states
/// inherit.
///
/// A QUESTION asked up the chain of enclosing states, never a set copied into each
/// level: a copy per level would hold the caller's variable names once per nesting
/// level, and the nesting is the caller's too.
#[derive(Clone, Copy)]
enum Outer<'a> {
    /// The definition itself: a run has assigned nothing where it starts.
    Nothing,
    /// A machine one state declares: what reaching that state had in scope, which is
    /// its own machine's answer for it.
    State {
        machine: &'a Scopes<'a>,
        state: &'a str,
    },
}

impl Outer<'_> {
    /// Whether the enclosing scope holds `name`.
    fn holds(&self, name: &str) -> bool {
        match self {
            Outer::Nothing => false,
            Outer::State { machine, state } => machine.holds(state, name),
        }
    }
}

/// Which variables each state of ONE state machine can count on having been assigned
/// by the time it runs: those EVERY path from the machine's entry point to that state
/// assigns, together with the ones inherited from the state that declares the machine.
///
/// A variable is in a state's scope only when no run can reach that state without
/// having assigned it — which is what makes the ones outside it "possibly" undefined:
/// the read resolves on some runs and not on others. A state no run reaches has
/// nothing but the inherited variables in scope.
struct Scopes<'a> {
    /// Every variable some state of this machine assigns, and which bit of a state's
    /// [`ScopeMask`] answers for it. A variable the analysis did NOT follow — the
    /// enclosing scope already holds it, or the bound or the budget is spent — carries
    /// no bit and is taken to be in scope.
    assigned: BTreeMap<String, Option<usize>>,
    /// Per state, which of the followed variables every run reaching it has assigned.
    masks: BTreeMap<&'a str, ScopeMask>,
    /// What this machine is declared inside.
    outer: Outer<'a>,
}

impl<'a> Scopes<'a> {
    /// A machine whose variables are not judged at all: what a walk that reports no
    /// WARNING needs, since the lint this feeds is one.
    fn none() -> Self {
        Self {
            assigned: BTreeMap::new(),
            masks: BTreeMap::new(),
            outer: Outer::Nothing,
        }
    }

    /// Follow each variable `states` assigns through the transition graph from
    /// `start_at`, with `outer` already in scope.
    ///
    /// Each variable is followed with ONE walk of the graph, from the entry point
    /// along the transitions that do NOT assign it, and each walk is spent from the
    /// diagnosis's own [`MAX_SCOPE_VISITS`] budget, so the analysis holds one walk's
    /// states at a time and costs the same whatever the definition declares.
    fn of(
        states: &'a Map<String, Value>,
        start_at: Option<&str>,
        outer: Outer<'a>,
        into: &mut Diagnosis,
    ) -> Self {
        // Which assignment block runs on the way along each transition: a state's own
        // when its work completed, and a `Catch` catcher's when it did not. The blocks
        // are held ONCE and the transitions name them by index, so a state with many
        // transitions does not hold its block once per transition.
        let mut blocks: Vec<BTreeSet<String>> = Vec::new();
        let mut graph: BTreeMap<&str, Vec<ScopeEdge>> = BTreeMap::new();
        for (name, state) in states {
            let mut out = Vec::new();
            if let Some(state) = state.as_object() {
                let Assignments { own, caught } = assignments(state);
                let own_block = blocks.len();
                blocks.push(own);
                let caught_blocks: Vec<usize> = caught
                    .into_iter()
                    .map(|block| {
                        blocks.push(block);
                        blocks.len() - 1
                    })
                    .collect();
                for transition in transitions(state) {
                    if !states.contains_key(transition.target.as_str()) {
                        continue;
                    }
                    let assigns = transition
                        .caught
                        .and_then(|index| caught_blocks.get(index).copied())
                        .unwrap_or(own_block);
                    out.push(ScopeEdge {
                        target: transition.target,
                        assigns,
                    });
                }
            }
            graph.insert(name.as_str(), out);
        }
        // Each walk below visits each state at most once, so one walk costs at most
        // this many visits of the budget.
        let per_walk = states.len();
        let mut scopes = Self {
            assigned: BTreeMap::new(),
            masks: states.keys().map(|name| (name.as_str(), 0)).collect(),
            outer,
        };
        // Every variable this machine assigns somewhere. One set per MACHINE, so what
        // this holds is the machine's own text rather than that text once per state.
        let declared: BTreeSet<&String> = blocks.iter().flatten().collect();
        if !into.spend_scope_visits(per_walk) {
            // Nothing can be followed, so nothing this machine assigns is judged.
            scopes.assigned = declared.into_iter().map(|name| (name.clone(), None)).collect();
            return scopes;
        }
        // A machine whose entry point names no declared state has no run to reason
        // about, so no state of it reaches a read with anything assigned.
        let entry = start_at.filter(|start_at| states.contains_key(*start_at));
        let reachable = reached_from(entry, &graph, &blocks, None);
        let mut followed = 0_usize;
        for name in declared {
            // A variable the enclosing scope already holds is in scope at every state
            // of this machine whatever this machine does with it, and one the bound or
            // the budget cannot reach is taken to be, so neither is followed.
            if followed >= MAX_SCOPED_VARIABLES
                || scopes.outer.holds(name)
                || !into.spend_scope_visits(per_walk)
            {
                scopes.assigned.insert(name.clone(), None);
                continue;
            }
            let index = followed;
            followed += 1;
            // Where the variable is NOT yet assigned: every state a run can reach
            // without having taken a transition that assigns it.
            let unassigned = reached_from(entry, &graph, &blocks, Some(name));
            for (state, mask) in &mut scopes.masks {
                if reachable.contains(state) && !unassigned.contains(state) {
                    *mask |= 1 << index;
                }
            }
            scopes.assigned.insert(name.clone(), Some(index));
        }
        scopes
    }

    /// Whether `state` can count on `name` having been assigned by the time it runs.
    fn holds(&self, state: &str, name: &str) -> bool {
        match self.assigned.get(name) {
            // A variable this machine assigns and the analysis followed: in scope
            // where every run reaching the state has passed an assignment of it.
            Some(Some(index)) => {
                self.masks.get(state).copied().unwrap_or(0) & (1 << index) != 0
            }
            // One it did not follow, which is taken to be in scope.
            Some(None) => true,
            // One no state of this machine assigns: only the enclosing scope can hold
            // it.
            None => self.outer.holds(name),
        }
    }
}

/// One transition of a machine's graph as the scope analysis walks it: where it goes,
/// and which of the machine's assignment blocks runs on the way there.
struct ScopeEdge {
    target: String,
    /// Index into the machine's blocks.
    assigns: usize,
}

/// The states one run can reach from `entry`, following `graph` — past a transition
/// whose own block assigns `without`, that run has assigned it, so such a transition
/// is not followed.
///
/// `without` `None` makes this the states reachable at all. The walk visits each state
/// once, so it costs one pass over the graph.
fn reached_from<'a>(
    entry: Option<&'a str>,
    graph: &BTreeMap<&'a str, Vec<ScopeEdge>>,
    blocks: &[BTreeSet<String>],
    without: Option<&str>,
) -> BTreeSet<&'a str> {
    let mut reached = BTreeSet::new();
    let mut pending: Vec<&str> = entry.into_iter().collect();
    while let Some(state) = pending.pop() {
        let Some((state, edges)) = graph.get_key_value(state) else {
            continue;
        };
        if !reached.insert(*state) {
            continue;
        }
        for edge in edges {
            let assigned = without.is_some_and(|name| {
                blocks
                    .get(edge.assigns)
                    .is_some_and(|block| block.contains(name))
            });
            if !assigned {
                pending.push(edge.target.as_str());
            }
        }
    }
    reached
}

/// What one state ASSIGNS, by the path the assignment runs on.
///
/// A key written as a JSONPath reference field (`"<name>.$"`) assigns the variable
/// `<name>`, the suffix being how that language spells a value read from a path. A
/// state machine NESTED in this state assigns in a scope of its own, so its states'
/// blocks are not read here.
struct Assignments {
    /// The keys of the state's own `Assign` block, which the language evaluates when
    /// the state's work completed.
    own: BTreeSet<String>,
    /// The keys of each `Catch` catcher's own `Assign`, in the catchers' own order,
    /// which are evaluated INSTEAD when the work failed and that catcher took over.
    caught: Vec<BTreeSet<String>>,
}

/// [`Assignments`] of one state.
fn assignments(state: &Map<String, Value>) -> Assignments {
    let names = |block: Option<&Value>| -> BTreeSet<String> {
        block
            .and_then(Value::as_object)
            .map(|block| {
                block
                    .keys()
                    .map(|key| key.strip_suffix(".$").unwrap_or(key).to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    Assignments {
        own: names(state.get(ASSIGN_MEMBER)),
        caught: state
            .get(CATCH_MEMBER)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|catcher| names(catcher.get(ASSIGN_MEMBER)))
            .collect(),
    }
}

/// Every variable one state can assign, on whichever path it runs: what its OWN
/// fields may read, since the language evaluates a state's assignments — its own, or
/// its catcher's where that catcher took over — before the output they feed.
///
/// Which of them a state the transition LEADS TO can count on is a different question,
/// and the one [`Scopes`] answers per transition.
fn assigned_variables(state: &Map<String, Value>) -> BTreeSet<String> {
    let Assignments { mut own, caught } = assignments(state);
    for block in caught {
        own.extend(block);
    }
    own
}

/// Every variable one state's own fields READ: the path from the state to the field
/// that reads it, and the variable's name, each pair once and in a deterministic
/// order.
///
/// A read is what [`variable_references`] reports a state referencing a variable
/// through, so the lint below and `DescribeStateMachine`'s `variableReferences` cannot
/// disagree about what a read is: a `$name` term of a JSONata expression (which the
/// language delimits with `{%` and `%}`) or of a JSONPath reference field (a member
/// whose name ends in `.$`). A state machine NESTED in this state is walked as the
/// states it declares, so its reads are reported under their own names.
///
/// The walk is the one [`walk_fields`] does for every field this module reads, so the
/// path a field is reported at is the same one every other diagnostic about it
/// carries.
fn variable_reads(state: &Map<String, Value>) -> Vec<(String, String)> {
    let mut found: BTreeSet<(String, String)> = BTreeSet::new();
    let roots: Vec<(String, &Value, bool)> = state
        .iter()
        .filter(|(member, _)| !NESTED_MACHINE_MEMBERS.contains(&member.as_str()))
        .map(|(member, value)| {
            (
                nested_at("", format_args!("/{}", echoed(member))),
                value,
                member.ends_with(".$"),
            )
        })
        .collect();
    walk_fields(roots, |path, text, reference_field| {
        let mut names = BTreeSet::new();
        for expression in jsonata_expressions(text) {
            scan_expression(expression, &mut names);
        }
        if reference_field {
            scan_expression(text, &mut names);
        }
        for name in names {
            found.insert((path.to_string(), name));
        }
    });
    found.into_iter().collect()
}

/// The query language `declared` names, `None` when the member is absent, or why it
/// is not a query language the Amazon States Language defines.
fn declared_language(declared: Option<&Value>) -> Result<Option<&'static str>, String> {
    let Some(declared) = declared else {
        return Ok(None);
    };
    let Some(named) = declared.as_str() else {
        return Err("'QueryLanguage' must be a string".to_string());
    };
    QUERY_LANGUAGES
        .iter()
        .find(|language| **language == named)
        .copied()
        .map(Some)
        .ok_or_else(|| {
            format!(
                "'QueryLanguage' must be one of {}, and this definition names '{}'",
                QUERY_LANGUAGES.join(", "),
                echoed(named)
            )
        })
}

/// The members a state reads its data through in JSONPath, which a state written in
/// JSONata must not carry: that language reads its data through members of its own
/// (`Arguments`, `Output`, `Items`, a `Choice` rule's `Condition`), so each of these
/// is a field of the OTHER query language.
///
/// These are the JSONPath data-flow members the Amazon States Language declares, as
/// [`executable_members`] above lists them per state type: the `*Path` family, the
/// payload templates and `Pass`'s static `Result`.
const JSONPATH_ONLY_MEMBERS: &[&str] = &[
    "CausePath",
    "ErrorPath",
    "HeartbeatSecondsPath",
    "InputPath",
    "ItemsPath",
    "MaxConcurrencyPath",
    "OutputPath",
    "Parameters",
    "Result",
    "ResultPath",
    "ResultSelector",
    "SecondsPath",
    "TimeoutSecondsPath",
    "TimestampPath",
    "ToleratedFailureCountPath",
    "ToleratedFailurePercentagePath",
];

/// The members that declare a state machine of their own, walked as one rather than
/// scanned as this state's data.
const NESTED_MACHINE_MEMBERS: &[&str] = &["Branches", "ItemProcessor", "Iterator"];

/// The member a state hands a failure to, whose own `Assign` and `Output` are the
/// only ones the error output is in scope in.
const CATCH_MEMBER: &str = "Catch";

/// The reserved term a JSONata expression reads the error of a caught failure
/// through, which the language scopes to a `Catch`: `$states.errorOutput` holds "the
/// error output" of the state that failed, and only that `Catch`'s own `Assign` and
/// `Output` are evaluated with it in scope.
const ERROR_OUTPUT: &str = "$states.errorOutput";

/// The reserved term a JSONata expression reads the state's own outcome through,
/// which the language binds only where there IS one: `$states.result` holds what the
/// state's work produced, so it is in scope in a state whose work produces a result
/// ([`RESULT_STATE_TYPES`]) and on that state's success path — never in a `Catch`,
/// which runs because the work produced no result at all.
const RESULT: &str = "$states.result";

/// The state types whose work produces a result, which is what binds [`RESULT`]: a
/// `Task`'s own work, and the per-item and per-branch results a `Map` and a
/// `Parallel` gather. Every other type carries its input through, so there is no
/// result for an expression of it to read.
const RESULT_STATE_TYPES: &[&str] = &["Map", "Parallel", "Task"];

/// How the service states a field an expression reads that the expression's scope does
/// not bind: the field is not there to read, so it is reported as the field it named.
///
/// `field` is the whole field expression as the definition wrote it — the reserved term
/// together with the member accessors written after it ([`field_read`]) — because that,
/// and not the term it starts with, is what the caller has to change.
fn field_does_not_exist(field: &str) -> String {
    format!("Field '{field}' does not exist.")
}

/// The reserved terms of the state's own outcome that a `member` of a state of this
/// type must NOT read.
///
/// One rule, read off the member and the state's type: a term is reported wherever
/// the language does not bind it, so an expression that could never resolve is
/// refused at validation time rather than at run time.
fn out_of_scope_terms(state_type: &str, member: &str) -> Vec<&'static str> {
    let caught = member == CATCH_MEMBER;
    let mut terms = Vec::new();
    if !caught {
        terms.push(ERROR_OUTPUT);
    }
    if caught || !RESULT_STATE_TYPES.contains(&state_type) {
        terms.push(RESULT);
    }
    terms
}

/// The rules a state written in JSONata must satisfy: it carries none of the other
/// language's data-flow members, and each reserved term of its own outcome is read
/// only where the language binds it ([`out_of_scope_terms`]).
fn diagnose_jsonata_state(
    name: &str,
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    for member in state.keys() {
        // A `"<name>.$"` member is a JSONPath reference field, which is how that
        // language — and only that one — spells a member read from the state's data.
        if JSONPATH_ONLY_MEMBERS.contains(&member.as_str()) || member.ends_with(".$") {
            let member = echoed(member);
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(here, &format!("/{member}")),
                format!(
                    "state '{name}' is written in '{JSONATA}', so it must not carry the \
                     '{JSONPATH}' member '{member}'"
                ),
            ));
        }
    }
    if state_type == "Choice" {
        if let Some(rules) = state.get("Choices").and_then(Value::as_array) {
            for (index, rule) in rules.iter().enumerate() {
                diagnose_jsonata_rule(
                    name,
                    rule,
                    &nested_at(here, format_args!("/Choices/{index}")),
                    MAX_EXPRESSION_DEPTH,
                    into,
                );
            }
        }
    }
    for (member, value) in state {
        if NESTED_MACHINE_MEMBERS.contains(&member.as_str()) {
            continue;
        }
        let shown = echoed(member);
        for term in out_of_scope_terms(state_type, member) {
            // One diagnostic per FIELD that reads the term, which is the granularity
            // `location` documents (`/States/<StateName>/<FieldName>`): a member
            // carrying several such fields is several problems to fix, each at its own
            // place in the caller's document.
            for (field, read) in fields_reading(value, term) {
                into.push(error(
                    UNSUPPORTED_JSONATA_EXPRESSION,
                    at(here, &format!("/{shown}{field}")),
                    field_does_not_exist(&read),
                ));
            }
        }
    }
}

/// One `Choice` rule of a JSONata state, and every rule nested in its `And`, `Or`
/// and `Not`: each one tests a `Condition`, never a JSONPath `Variable`.
///
/// `depth` bounds the nesting this follows, as every other expression tree here is
/// bounded: the rule tree is caller-shaped.
fn diagnose_jsonata_rule(
    name: &str,
    rule: &Value,
    here: &str,
    depth: usize,
    into: &mut Diagnosis,
) {
    let Some(depth) = depth.checked_sub(1) else {
        return;
    };
    let Some(rule) = rule.as_object() else {
        return;
    };
    for (member, value) in rule {
        if member == "Variable" || member.ends_with(".$") {
            let shown = echoed(member);
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(here, &format!("/{shown}")),
                format!(
                    "a 'Choice' rule of state '{name}' is written in '{JSONATA}', so it must not \
                     carry the '{JSONPATH}' member '{shown}'"
                ),
            ));
        }
        match member.as_str() {
            "And" | "Or" => {
                if let Some(nested) = value.as_array() {
                    for (index, rule) in nested.iter().enumerate() {
                        diagnose_jsonata_rule(
                            name,
                            rule,
                            &nested_at(here, format_args!("/{}/{index}", echoed(member))),
                            depth,
                            into,
                        );
                    }
                }
            }
            "Not" => diagnose_jsonata_rule(name, value, &nested_at(here, "/Not"), depth, into),
            _ => {}
        }
    }
}

/// Every FIELD inside `value` whose JSONata expression reads `term`: the path from the
/// member that carries `value` down to that field — the empty path for a member that is
/// itself such an expression — paired with the field expression that read the term
/// there ([`field_read`]).
///
/// The list is sorted lexicographically, which is what makes the per-field list of one
/// member deterministic whatever shape the document has.
fn fields_reading(value: &Value, term: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    walk_fields(vec![(String::new(), value, false)], |path, text, _| {
        // One field is one problem to fix, so it is reported once: the field
        // expression reported for it is the FIRST its expressions read the term
        // through.
        if let Some(read) = jsonata_expressions(text)
            .iter()
            .find_map(|expression| field_read(expression, term))
        {
            found.push((path.to_string(), read));
        }
    });
    found.sort();
    found
}

/// Visit every string `roots` carries, with the path from its root down to it and
/// whether it is the value of a JSONPath reference field (a member whose name ends in
/// `.$`), which is what makes a string a reference rather than a literal.
///
/// The one walk of a caller-shaped document this module reads a state's fields with,
/// so the path a field is reported at cannot differ between what reads it. It is
/// ITERATIVE: the document is a caller's, so it carries its own stack rather than the
/// thread's, and each path is built through [`nested_at`], so it is bounded as it
/// grows.
fn walk_fields(roots: Vec<(String, &Value, bool)>, mut visit: impl FnMut(&str, &str, bool)) {
    let mut pending = roots;
    while let Some((path, value, reference_field)) = pending.pop() {
        match value {
            Value::String(text) => visit(&path, text, reference_field),
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    pending.push((
                        nested_at(&path, format_args!("/{index}")),
                        item,
                        reference_field,
                    ));
                }
            }
            Value::Object(members) => {
                for (member, child) in members {
                    pending.push((
                        nested_at(&path, format_args!("/{}", echoed(member))),
                        child,
                        member.ends_with(".$"),
                    ));
                }
            }
            _ => {}
        }
    }
}

/// The FIELD a JSONata `expression` reads `term` through, or `None` when it reads it
/// nowhere.
///
/// A field expression is the term together with the member accessors written after it:
/// the language reads a member of a value by a `.` followed by that member's name, so
/// the field runs from the term through every such step that follows it and ends at the
/// first character that can be part of neither. A term that is itself only the start of
/// a longer name is read to the end of that name, since what was written there is a
/// different field and not the term at all.
///
/// The result is [`echoed`]: the accessors are the caller's own text.
fn field_read(expression: &str, term: &str) -> Option<String> {
    let read = &expression[expression.find(term)?..];
    let bytes = read.as_bytes();
    let mut end = term.len();
    while bytes.get(end).is_some_and(|byte| is_member_byte(*byte)) {
        end += 1;
    }
    while bytes.get(end) == Some(&b'.') {
        let mut step = end + 1;
        let name = step;
        while bytes.get(step).is_some_and(|byte| is_member_byte(*byte)) {
            step += 1;
        }
        if step == name {
            break;
        }
        end = step;
    }
    Some(echoed(&read[..end]))
}

/// Whether `byte` can be part of a name written without quoting: what bounds the field
/// expression [`field_read`] reads, and the `$name` term [`scan_expression`] reads.
fn is_member_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The payload templates a JSONPath state renders: every member of one is a LITERAL
/// unless its name ends in `.$`, which is what makes its value a path.
const PAYLOAD_TEMPLATE_MEMBERS: &[&str] = &["ItemSelector", "Parameters", "ResultSelector"];

/// Members whose string value names something — a state, a type, a language — rather
/// than holding the state's data, so a `$` in one is part of that name.
const NAMING_MEMBERS: &[&str] = &[
    "Comment",
    "Default",
    "Next",
    "QueryLanguage",
    "StartAt",
    "Type",
];

/// The WARNINGS a state written in JSONPath can carry: each one is a value that
/// READS as a path or an intrinsic invocation where the language takes a literal, so
/// the definition is created and the value is used verbatim.
fn diagnose_jsonpath_state(
    name: &str,
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    // PASS_RESULT_IS_STATIC: "Attempt to use a path in the result of a pass state".
    if state_type == "Pass" {
        if let Some(result) = state.get("Result").and_then(Value::as_str) {
            if looks_like_a_path(result) {
                into.push(warning(
                    PASS_RESULT_IS_STATIC,
                    at(here, "/Result"),
                    format!(
                        "the 'Result' of state '{name}' holds '{}', which reads as a path, \
                         but a 'Result' is the state's literal result: 'Parameters' or \
                         'ResultPath' read a path",
                        echoed(result)
                    ),
                ));
            }
        }
    }
    // NO_DOLLAR: "No `.$` on a field that appears to be a JSONPath or Intrinsic
    // Function".
    for member in PAYLOAD_TEMPLATE_MEMBERS {
        let Some(template) = state.get(*member).and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in template {
            let Some(text) = value.as_str() else {
                continue;
            };
            if key.ends_with(".$") || !(looks_like_a_path(text) || looks_like_an_intrinsic(text)) {
                continue;
            }
            let key = echoed(key);
            let text = echoed(text);
            into.push(warning(
                NO_DOLLAR,
                at(here, &format!("/{member}/{key}")),
                format!(
                    "'{key}' of the '{member}' of state '{name}' holds '{text}', which reads as a \
                     path or an intrinsic function, but a member evaluated as one is named \
                     '{key}.$'"
                ),
            ));
        }
    }
    // NO_PATH: "Field value looks like a path, but field name does not end with
    // 'Path'".
    for (member, value) in state {
        let Some(text) = value.as_str() else {
            continue;
        };
        if NAMING_MEMBERS.contains(&member.as_str())
            || member.ends_with("Path")
            || member.ends_with(".$")
            || (state_type == "Pass" && member == "Result")
            || !looks_like_a_path(text)
        {
            continue;
        }
        let member = echoed(member);
        let text = echoed(text);
        into.push(warning(
            NO_PATH,
            at(here, &format!("/{member}")),
            format!(
                "the '{member}' of state '{name}' holds '{text}', which reads as a path, but \
                 '{member}' does not end in 'Path', so it is used as the literal it is"
            ),
        ));
    }
}

/// Whether `text` reads as a JSONPath: every path the language takes starts at the
/// state's data (`$`) or at the context object (`$$`).
fn looks_like_a_path(text: &str) -> bool {
    text.starts_with('$')
}

/// Whether `text` reads as an intrinsic-function invocation, which the language
/// spells `States.<Name>(<arguments>)`.
fn looks_like_an_intrinsic(text: &str) -> bool {
    text.starts_with("States.") && text.contains('(') && text.ends_with(')')
}

/// The name the ONE state `TestState` tests is declared under while it is judged.
///
/// A `definition` that carries the state alone gives it no name, and every diagnostic
/// the walk words names the state it was found in, so the state is judged under this
/// one.
const SINGLE_STATE_NAME: &str = "TheStateUnderTest";

/// One transition a state declares: where it goes, where in the state the member that
/// names it sits, that member's own NAME — which a diagnostic about an unresolved
/// target reports, since `Default` and `Next` are different fields to fix — and
/// whether it is taken because the state FAILED.
struct Transition {
    target: String,
    /// The path from the state to the member naming the target.
    path: String,
    member: &'static str,
    /// `Some(index)` for the `Next` of the `Catch` catcher at that index: a
    /// transition the state's own work never completed, so the state's own `Assign`
    /// did not run and that catcher's did.
    caught: Option<usize>,
}

/// Every transition `state` declares, in the order the members that name them are
/// judged: its own `Next`, the `Default` of a `Choice`, each `Choice` rule's own
/// `Next`, and each `Catch` catcher's `Next`.
///
/// ONE enumeration, read both by the diagnostic that resolves a target against the
/// enclosing `States` map and by the state graph the variable-scope analysis walks, so
/// the two cannot disagree about where a state can go.
///
/// Linear in the state's own size, which the modelled `Definition` (1 MiB) bounds, so
/// the list this builds is bounded by the request exactly as the state is.
fn transitions(state: &Map<String, Value>) -> Vec<Transition> {
    let mut transitions = Vec::new();
    for member in ["Next", "Default"] {
        if let Some(target) = state.get(member).and_then(Value::as_str) {
            transitions.push(Transition {
                target: target.to_string(),
                path: format!("/{member}"),
                member,
                caught: None,
            });
        }
    }
    for (member, caught) in [("Choices", false), (CATCH_MEMBER, true)] {
        let entries = state
            .get(member)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (index, entry) in entries.iter().enumerate() {
            if let Some(target) = entry.get("Next").and_then(Value::as_str) {
                transitions.push(Transition {
                    target: target.to_string(),
                    path: nested_at("", format_args!("/{member}/{index}/Next")),
                    member: "Next",
                    caught: caught.then_some(index),
                });
            }
        }
    }
    transitions
}

/// The states `state` can transition to, however it names them: what the lone state
/// of a [`validate_single_state`] has declared beside it.
fn transition_targets(state: &Value) -> Vec<String> {
    state
        .as_object()
        .map(|state| {
            transitions(state)
                .into_iter()
                .map(|transition| transition.target)
                .collect()
        })
        .unwrap_or_default()
}

/// Why `definition` is not the definition of a SINGLE state this service accepts, or
/// `Ok` when it is — what `TestState` takes when it names no `stateName`, since it
/// "accepts the definition of a single state".
///
/// The state is judged by the SAME walk every state of a state machine is, by
/// declaring it as the one state of a machine: each rule of the Amazon States
/// Language then applies to it unchanged, down to the machines nested inside it and
/// the query language it is written in. Two things about a LONE state are declared
/// around it rather than checked against it, because no definition encloses it:
///
/// * its transition targets name states no `States` map declares, so each one is
///   declared beside it as a `Succeed` state — `TestState` REPORTS the target it
///   names as `nextState` rather than resolving it;
/// * it need not say where it goes at all ("if you haven't defined a next state in
///   your definition [...] this field doesn't contain a value"), where a state
///   DECLARED in a machine must carry `Next` or `"End": true`, so a state naming no
///   transition is declared as the machine's terminal state.
///
/// Nothing else about the state is altered, and the `Err` is what the service reports
/// about THAT state: a required member the supplied object does not carry is named on
/// its own ([`missing_field_message`]), and every other diagnostic is stated without
/// the name and the JSON pointer the enclosing machine was declared under, neither of
/// which the request carries ([`about_the_single_state`]).
pub fn validate_single_state(definition: &str) -> Result<(), String> {
    let Ok(state) = serde_json::from_str::<Value>(definition) else {
        // A definition that is not JSON at all is refused in the same words whichever
        // form it was sent in, by the walk that words every other definition refusal.
        return match validate(definition) {
            Err(why) => Err(why),
            Ok(()) => Err(error(
                INVALID_JSON_DESCRIPTION,
                None,
                "the definition is not valid JSON",
            )
            .stated()),
        };
    };
    // The definition IS the state, so a member the language requires of a state is
    // missing from the object the REQUEST sent: it is reported as that member, and
    // nothing is said about a state machine the request does not carry.
    if let Some(member) = missing_required_member(&state) {
        return Err(missing_field_message(&member));
    }
    let mut judged = state.clone();
    if let Some(object) = judged.as_object_mut() {
        let continuing = object
            .get("Type")
            .and_then(Value::as_str)
            .is_some_and(|state_type| CONTINUING_STATE_TYPES.contains(&state_type));
        if continuing && !object.contains_key("Next") && !object.contains_key("End") {
            object.insert("End".to_string(), Value::Bool(true));
        }
    }
    let mut states = Map::new();
    states.insert(SINGLE_STATE_NAME.to_string(), judged);
    for target in transition_targets(&state) {
        states
            .entry(target)
            .or_insert_with(|| json!({ "Type": "Succeed" }));
    }
    let machine = json!({ "StartAt": SINGLE_STATE_NAME, "States": Value::Object(states) });
    let mut found = Diagnosis::until_first_error();
    diagnose_machine(
        &machine,
        "",
        JSONPATH,
        1,
        false,
        Outer::Nothing,
        &mut found,
    );
    match found.diagnostics.iter().find(|found| found.is_error()) {
        Some(found) => Err(about_the_single_state(found)),
        None => Ok(()),
    }
}

/// The members the Amazon States Language requires of a state, as alternatives: a
/// state carries the first member of each entry, or one of the members beside it.
///
/// Every state names what it does with `Type`, and a type whose work ONE member
/// declares requires that member too: a `Task` "requires a `Resource` field", a
/// `Choice` carries the `Choices` array its rules live in, a `Parallel` the `Branches`
/// it runs, and a `Map` the `ItemProcessor` its items go through (the language's
/// earlier `Iterator` names the same work). A `Pass`, `Succeed`, `Fail` or `Wait`
/// state declares its work through members none of which it must carry alone.
fn required_state_members(state_type: &str) -> &'static [&'static [&'static str]] {
    match state_type {
        "Task" => &[&["Resource"]],
        "Choice" => &[&["Choices"]],
        "Parallel" => &[&["Branches"]],
        "Map" => &[&["ItemProcessor", "Iterator"]],
        _ => &[],
    }
}

/// The first member `state` must carry and does not: `Type`, and then the member the
/// type it names requires ([`required_state_members`]).
///
/// `None` for a state that carries each of them, and for a `definition` that is not a
/// state object at all — which is not a member that is missing, and is reported as the
/// object it is instead.
fn missing_required_member(state: &Value) -> Option<String> {
    let state = state.as_object()?;
    if !state.contains_key("Type") {
        return Some("Type".to_string());
    }
    // A `Type` that is present but is not a string names no state type, so which
    // members the state owes is not decided here: that is a defect in the member it
    // carries rather than a member it is missing.
    let state_type = state.get("Type").and_then(Value::as_str)?;
    required_state_members(state_type)
        .iter()
        .find(|alternatives| {
            !alternatives
                .iter()
                .any(|member| state.contains_key(*member))
        })
        .and_then(|alternatives| alternatives.first().map(|member| (*member).to_string()))
}

/// What the service reports for a required member the state under test does not
/// carry: the member, named on its own.
///
/// The request sent ONE state object, so there is no state name and no path into an
/// enclosing definition to place the member in.
fn missing_field_message(member: &str) -> String {
    format!("Missing required field: '{}'", echoed(member))
}

/// What a diagnostic of the WRAPPED walk says about the state the request supplied.
///
/// The lone state is judged by declaring it as the one state of a state machine, so
/// each diagnostic names the state under that declared name and locates it inside that
/// machine. Neither is in the request, so the name is reported as the state under test
/// and the location is read back to the state's own member — `None` where it was the
/// state itself.
fn about_the_single_state(found: &Diagnostic) -> String {
    let declared = format!("state '{SINGLE_STATE_NAME}'");
    let message = found.message.replace(&declared, SINGLE_STATE_LABEL);
    let inside = format!("/States/{SINGLE_STATE_NAME}");
    let member = found
        .location
        .as_deref()
        .and_then(|location| location.strip_prefix(inside.as_str()))
        .filter(|member| !member.is_empty());
    match member {
        Some(member) => format!("{}: {message} at {member}", found.code),
        None => format!("{}: {message}", found.code),
    }
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
        while bytes.get(end).is_some_and(|byte| is_member_byte(*byte)) {
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
///
/// `tasks` is what a `Task` state's service integration is performed by: the walk
/// asks for the call and the operation running the walk makes it
/// ([`super::integration::interpret`]).
pub fn run(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    tasks: &dyn Tasks,
) -> Run {
    interpret(
        definition,
        input,
        start,
        context,
        Entry::Start,
        Duration::ZERO,
        tasks,
    )
    .0
}

/// Walk `definition` again from the step a redriven execution did not get past, and
/// report what happened.
///
/// "When you redrive an execution, it continues the failed execution from the
/// unsuccessful step and uses the same input. Step Functions preserves the results and
/// execution history of the successful steps, and doesn't rerun these steps when you
/// redrive an execution": the walk begins AT the state `from` names, with the raw
/// input that state was entered with, so no state the attempt already got past is run
/// a second time and the values those states produced — which `from`'s own input
/// carries — stand for the redriven attempt.
///
/// The state RUNS: it is the unsuccessful step, so a `Task` state there makes its
/// service-integration call again, or schedules its activity task again, rather than
/// being handed a result ([`resume`] is the walk that is handed one). `tasks` is the
/// same seam [`run`] takes, so that call is made exactly as the first attempt's was.
pub fn redrive(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    from: &RedriveStep,
    tasks: &dyn Tasks,
) -> Run {
    let state_input = serde_json::from_str(&from.state_input).unwrap_or(Value::Null);
    interpret(
        definition,
        input,
        start,
        context,
        Entry::At(from.state_name.clone(), state_input),
        Duration::ZERO,
        tasks,
    )
    .0
}

/// Walk `definition` on from the activity task `reply` answers, and report what
/// happened.
///
/// A worker's reply is the result of the state that scheduled its task, so the walk
/// begins AT that state — with the raw input the state was entered with the first
/// time, which `reply` carries and which its `ResultPath` combines the reply with —
/// applies the reply there instead of scheduling a task, and carries on from its
/// `Next`. The states before it already ran, under the call that scheduled the task,
/// so neither their work nor their events are repeated: what this reports begins at
/// the reply.
///
/// `input` is the EXECUTION's own input, which the context object (`$$`) reports; it
/// is not the document the walk starts from.
///
/// `elapsed` is how long the execution has ALREADY been running when the reply
/// arrives, which is what a definition-level `TimeoutSeconds` bounds: the span a task
/// spent outstanding is part of the execution's own run.
///
/// `tasks` is the same seam [`run`] takes: the states after the reply may themselves
/// be service integrations, and they are called exactly as the first attempt's were.
///
/// `None` when the definition declares no such state any more, so there is nothing to
/// continue from.
pub fn resume(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    reply: &ActivityReply,
    elapsed: Duration,
    tasks: &dyn Tasks,
) -> Option<Run> {
    let state_input: Value = serde_json::from_str(&reply.state_input).ok()?;
    let (run, applied) = interpret(
        definition,
        input,
        start,
        context,
        Entry::Reply(reply.clone(), state_input),
        elapsed,
        tasks,
    );
    applied.then_some(run)
}

/// Walk `definition` on from the run-a-job task whose job has ENDED, and report what
/// happened.
///
/// The job the state's integration started is its unit of work, so what that job
/// ended as is the state's own outcome: the walk begins AT that state — with the raw
/// input it was entered with the first time, which `reply` carries and which its
/// `ResultPath` combines the job's report with — reports the outcome there instead of
/// starting the job again, and carries on from its `Next` (or into its `Catch`, for a
/// job that ended without doing its work). The states before it already ran, under
/// the call that started the job, so neither their work nor their events are
/// repeated.
///
/// `input` is the EXECUTION's own input, which the context object (`$$`) reports; it
/// is not the document the walk starts from. `elapsed` is how long the execution had
/// already been running when the job ended, which a definition-level `TimeoutSeconds`
/// bounds: the span a job stayed unfinished is part of the execution's own run.
///
/// `None` when the definition declares no such state any more, so there is nothing to
/// continue from.
pub fn resume_job(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    reply: &JobReply,
    elapsed: Duration,
    tasks: &dyn Tasks,
) -> Option<Run> {
    let state_input: Value = serde_json::from_str(&reply.state_input).ok()?;
    let (run, applied) = interpret(
        definition,
        input,
        start,
        context,
        Entry::Job(reply.clone(), state_input),
        elapsed,
        tasks,
    );
    applied.then_some(run)
}

/// Where a walk of a definition begins, and what it does when it gets there.
enum Entry {    /// At the definition's `StartAt`, with the execution's own input: an attempt of
    /// its own.
    Start,
    /// At the state whose activity task a worker's reply answers, with the raw input
    /// that state was entered with: the reply is that state's RESULT, so the state
    /// does not run again.
    Reply(ActivityReply, Value),
    /// At the state whose run-a-job integration started the job that has now ended,
    /// with the raw input that state was entered with: what the job ended as is that
    /// state's outcome, so the job is not started again.
    Job(JobReply, Value),
    /// At the state a redriven execution did not get past, with the raw input that
    /// state was entered with: the state RUNS, because it is the step the attempt
    /// stopped at.
    At(String, Value),
}

/// One walk of `definition`, and whether the reply [`Entry::Reply`] carries was
/// applied in it.
///
/// `entry` is where the walk begins: at `StartAt`, at the state a reply answers, or at
/// the step a redrive continues from.
///
/// `elapsed` is the span the execution had already spent before this walk's first
/// state, which a definition-level `TimeoutSeconds` counts.
#[allow(clippy::too_many_arguments)]
fn interpret(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    entry: Entry,
    elapsed: Duration,
    tasks: &dyn Tasks,
) -> (Run, bool) {
    let document: Value = match serde_json::from_str(definition) {
        Ok(document) => document,
        Err(why) => {
            return (
                failed(
                    Vec::new(),
                    Duration::ZERO,
                    runtime(format!(
                        "the state machine definition is not valid JSON: {}",
                        crate::state::json_parse_diagnostic(definition, &why)
                    )),
                ),
                false,
            );
        }
    };
    let (Some(start_at), Some(states)) = (
        document.get("StartAt").and_then(Value::as_str),
        document.get("States").and_then(Value::as_object),
    ) else {
        return (
            failed(
                Vec::new(),
                Duration::ZERO,
                runtime(
                    "the state machine definition carries no 'StartAt' and 'States' pair"
                        .to_string(),
                ),
            ),
            false,
        );
    };
    if let Some(language) = document.get("QueryLanguage").and_then(Value::as_str) {
        if language != JSONPATH {
            return (
                failed(
                    Vec::new(),
                    Duration::ZERO,
                    runtime(format!(
                        "the definition is written in '{language}', and this emulator evaluates \
                         only '{JSONPATH}' definitions, so nothing was run for it"
                    )),
                ),
                false,
            );
        }
    }
    let input_value: Value = match serde_json::from_str(input) {
        Ok(value) => value,
        Err(why) => {
            return (
                failed(
                    Vec::new(),
                    Duration::ZERO,
                    runtime(format!(
                        "the execution input is not valid JSON: {}",
                        crate::state::json_parse_diagnostic(input, &why)
                    )),
                ),
                false,
            );
        }
    };

    // A walk that CONTINUES an execution begins at the state whose task the reply
    // answers, or at the step a redrive continues from, with the raw input that state
    // was entered with; one that is an attempt of its own begins at `StartAt` with the
    // execution's input. A state the definition no longer declares — the state machine
    // an execution started through an unqualified ARN has been updated since — is no
    // step to continue from, so such a walk is an attempt of its own.
    let (reply, job_reply, start_at, first_input) = match entry {
        Entry::Reply(reply, state_input) => {
            let state_name = reply.state_name.clone();
            (Some(reply), None, state_name, state_input)
        }
        Entry::Job(reply, state_input) => {
            let state_name = reply.state_name.clone();
            (None, Some(reply), state_name, state_input)
        }
        Entry::At(state_name, state_input) if states.contains_key(&state_name) => {
            (None, None, state_name, state_input)
        }
        Entry::At(..) | Entry::Start => (None, None, start_at.to_string(), input_value.clone()),
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
        tasks,
        waiting: None,
        parked: None,
        map_runs: Vec::new(),
        map_runs_entered: 0,
        reply,
        job_reply,
        resumed: None,
        // A child execution's ARN is minted in the state machine's OWN region, from
        // the state machine's own ARN, so it agrees with the parent execution's
        // however the request that started it was signed.
        machine_name: context.state_machine_name.clone(),
        region: super::arn_region(&context.state_machine_arn),
        child_counter: 0,
        express: context.express,
    };
    let branch = interpreter.walk(
        states,
        &start_at,
        first_input,
        Duration::ZERO,
        &Scope::default(),
    );
    let mut waiting = interpreter.waiting.take();
    let mut parked = interpreter.parked.take();
    let mut steps = interpreter.steps;
    // One execution's Map Runs are reported "in the order their states were entered",
    // which is the position each one carries. A nested Distributed Map inside another's
    // items finishes first, so the list is built in finish order and put in entry order
    // HERE, once nothing can prune it any more.
    let mut entered = interpreter.map_runs;
    entered.sort_by_key(|(entered, _)| *entered);
    let mut map_runs: Vec<StartedMapRun> =
        entered.into_iter().map(|(_, map_run)| map_run).collect();
    let mut end = branch.end;
    // A walk that applied a reply reports only what FOLLOWS it: the steps the replying
    // state writes before the reply is placed are the ones the call that scheduled the
    // task already recorded.
    let resumed = interpreter.resumed;
    if let Some(resumed) = resumed {
        steps.drain(..resumed.index.min(steps.len()));
        for step in &mut steps {
            step.after = step.after.saturating_sub(resumed.at);
        }
        map_runs.retain(|map_run| map_run.started_at >= resumed.at);
        for map_run in &mut map_runs {
            map_run.started_at = map_run.started_at.saturating_sub(resumed.at);
            map_run.stopped_at = map_run.stopped_at.saturating_sub(resumed.at);
            for item in &mut map_run.items {
                item.finished_at = item.finished_at.saturating_sub(resumed.at);
            }
            for child in &mut map_run.children {
                child.started_at = child.started_at.saturating_sub(resumed.at);
                child.finished_at = child.finished_at.saturating_sub(resumed.at);
            }
        }
        if let Some(waiting) = waiting.as_mut() {
            waiting.at = waiting.at.saturating_sub(resumed.at);
        }
        if let Some(parked) = parked.as_mut() {
            parked.at = parked.at.saturating_sub(resumed.at);
        }
        end = end.saturating_sub(resumed.at);
    }
    let applied = resumed.is_some();

    // A definition-level `TimeoutSeconds` bounds the whole execution: a run that got
    // past it ended TIMED_OUT at the timeout, whatever it went on to compute. The span
    // an execution had already spent before this walk — the time a task it was
    // suspended at stayed outstanding, say — counts towards it, so what is left of the
    // timeout for THIS walk is what its own offsets are judged against.
    if let Some(seconds) = document.get("TimeoutSeconds").and_then(Value::as_u64) {
        let timeout = Duration::from_secs(seconds);
        let remaining = timeout.saturating_sub(elapsed);
        if end > remaining {
            let kept = steps
                .into_iter()
                .filter(|step| step.after <= remaining)
                .collect();
            // A Map Run the walk only reached past the timeout never started, exactly
            // as the events past it never happened; one that had started stopped WHEN
            // THE EXECUTION DID, so its own stop cannot be later than the timeout.
            let kept_map_runs = map_runs
                .into_iter()
                .filter(|map_run| map_run.started_at <= remaining)
                .map(|mut map_run| {
                    map_run.stopped_at = map_run.stopped_at.min(remaining);
                    // An item whose iteration would only have finished past the
                    // timeout never finished: the execution ended first, so the Map
                    // Run never counted it.
                    map_run.items.retain(|item| item.finished_at <= remaining);
                    // And the child execution that item ran as never finished
                    // either, for the same reason.
                    map_run
                        .children
                        .retain(|child| child.finished_at <= remaining);
                    map_run
                })
                .collect();
            return (
                terminal(
                    kept,
                    remaining,
                    super::EVENT_EXECUTION_TIMED_OUT,
                    super::EXECUTION_STATUS_TIMED_OUT,
                    None,
                    Some(TIMEOUT_ERROR.to_string()),
                    Some(format!(
                        "the execution ran for longer than the {seconds} seconds its definition's \
                         'TimeoutSeconds' allows"
                    )),
                )
                .with_map_runs(kept_map_runs),
                applied,
            );
        }
    }

    // An execution that SUSPENDED reached no outcome: the walk stopped at an activity
    // task, and what happens next is a worker's reply, or it parked at the job a
    // run-a-job integration started, and what happens next is that job finishing. So
    // the run reports what it is waiting for and no terminal event is written. Two
    // checks come FIRST, so a run that cannot reach it is not reported as waiting for
    // it: the definition-level timeout above, which a run's own `Wait` states can
    // already have spent, and the branch's outcome, which is a FAILURE where the
    // document the task would carry outgrew what a state may produce.
    if branch.outcome.is_ok() && (waiting.is_some() || parked.is_some()) {
        return (
            Run {
                status: super::EXECUTION_STATUS_RUNNING,
                output: None,
                error: None,
                cause: None,
                steps,
                elapsed: end,
                waiting,
                parked,
                map_runs,
            },
            applied,
        );
    }

    let run = match branch.outcome {
        // A state's input and output is a JSON DOCUMENT: when the document the run
        // ended with is the one the caller wrote — no state transformed it — the
        // caller's own BYTES are what the execution reports, rather than a
        // re-encoding of a parsed copy. A document the definition built is reported
        // in the encoding the walk produced it in.
        Ok(value) if value == input_value => {
            succeeded(steps, end, input.to_string()).with_map_runs(map_runs)
        }
        Ok(value) => succeeded(steps, end, value.to_string()).with_map_runs(map_runs),
        Err(failure) => failed(steps, end, failure).with_map_runs(map_runs),
    };
    (run, applied)
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
        super::EVENT_EXECUTION_SUCCEEDED,
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
            super::EVENT_EXECUTION_FAILED,
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
        super::EVENT_EXECUTION_FAILED,
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
        details: super::EventDetails::default(),
    });
    Run {
        status,
        output,
        error,
        cause,
        steps,
        elapsed,
        // A run that reached a terminal event is waiting for nothing.
        waiting: None,
        parked: None,
        // Set by `run`, which is where the walk's Map Runs are in scope.
        map_runs: Vec::new(),
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
struct Interpreter<'tasks> {
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
    /// What a `Task` state's service integration is performed by, for the one walk
    /// this interpreter is: a nested `Map` iteration or `Parallel` branch runs in
    /// this same frame, so its tasks are made through the same seam.
    tasks: &'tasks dyn Tasks,
    /// The activity task a `Task` state scheduled, which SUSPENDED the walk: the
    /// execution waits at that state until a worker answers it. Set at most once,
    /// because the walk stops at the state that set it.
    waiting: Option<ScheduledActivity>,
    /// The job a `Task` state's run-a-job integration started, which PARKED the walk:
    /// the execution waits at that state until the job finishes. Set at most once,
    /// because the walk stops at the state that set it.
    parked: Option<ParkedJob>,
    /// The Map Run every Distributed Map state this walk entered started, each with
    /// the position of its state among the Map states this walk ENTERED
    /// ([`Interpreter::map_runs_entered`]).
    ///
    /// A run is recorded when its items are done, so the list is built in FINISH
    /// order and read in entry order ([`plan_run`]) — a nested Distributed Map inside
    /// another's items finishes first, and two of them may be entered at the same
    /// offset, so the position is carried rather than derived from the instants.
    /// Bounded by [`MAX_STATE_TRANSITIONS`], which every `Map` state spends from.
    map_runs: Vec<(usize, StartedMapRun)>,
    /// How many `Map` states this walk has ENTERED, which is what numbers the Map
    /// Runs above in entry order.
    map_runs_entered: usize,
    /// The reply a worker made to the activity task this walk is CONTINUING from,
    /// when it is one: the state the reply names produces that result instead of
    /// suspending. Taken when it is applied, so one reply answers one task.
    reply: Option<ActivityReply>,
    /// What the job this walk is CONTINUING from ended as, when it is one: the state
    /// that started that job reports that outcome instead of starting it again.
    /// Taken when it is applied, so one outcome answers one parked job.
    job_reply: Option<JobReply>,
    /// Where the reply was applied, once it has been: what the walk did BEFORE that
    /// point already happened under an earlier call, so [`interpret`] reports only
    /// what follows it.
    resumed: Option<Resumed>,
    /// The name of the state machine this walk runs, and the region its ARNs are
    /// minted in: what the ARN of a child execution a Distributed Map state starts is
    /// built from, together with that state's label.
    machine_name: String,
    region: String,
    /// How many child executions this walk has already named, mixed into the next
    /// one's drawn name so two mints in the same nanosecond differ — exactly as the
    /// store's own counter does for an execution name it mints.
    child_counter: u64,
    /// Whether this walk runs an EXPRESS workflow, which is what refuses a `Task`
    /// state asking for the run-a-job pattern ([`Context::express`]).
    express: bool,
}

/// Where a walk applied an [`ActivityReply`]: the step its own events begin at, and
/// the offset into the walk at which the replying state was entered.
#[derive(Debug, Clone, Copy)]
struct Resumed {
    index: usize,
    at: Duration,
}

/// How the items of ONE `Map` attempt turned out, counted as they are run.
///
/// A `Map` attempt can end at any item (a budget, a deadline, a document past what a
/// state may carry), so the counts are kept here rather than derived from the
/// attempt's result: however the attempt ended, what its items did is what this
/// holds.
#[derive(Debug, Default)]
struct MapTally {
    /// Each item that finished, with the offset it finished at, so what the Map Run
    /// reports can follow the items one by one rather than all at once.
    items: Vec<MapItemOutcome>,
    /// The child execution each item of a DISTRIBUTED attempt ran as, in the order
    /// the iterations were started; empty for an inline `Map`, which starts none.
    children: Vec<StartedChildExecution>,
}

impl Interpreter<'_> {
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
                // The state scheduled an activity task: the walk stops here, and
                // `run` reports the task rather than an outcome. Only a TOP-LEVEL
                // state suspends (see [`Interpreter::activity_task_state`]), so no
                // enclosing `Map` iteration or `Parallel` branch can be left holding
                // a half-finished walk.
                Next::Suspended => {
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
                self.push_entered("PassStateEntered", name, at, &raw);
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
                self.push_exited("PassStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: self.next_of(name, state).map_err(|failure| failure.at(at))?,
                })
            }
            "Wait" => {
                self.push_entered("WaitStateEntered", name, at, &raw);
                let waited = self
                    .wait_duration(state, &effective, &context, at)
                    .map_err(|why| runtime(format!("state '{name}': {why}")).at(at))?;
                let at = at + waited;
                let value = self
                    .output_value(state, effective, &context)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("WaitStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: self.next_of(name, state).map_err(|failure| failure.at(at))?,
                })
            }
            "Choice" => {
                self.push_entered("ChoiceStateEntered", name, at, &raw);
                let next = self
                    .choose(state, &effective, &context)
                    .map_err(|failure| failure.at(at))?;
                let value = self
                    .output_value(state, effective, &context)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("ChoiceStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: Next::Named(next),
                })
            }
            "Succeed" => {
                self.push_entered("SucceedStateEntered", name, at, &raw);
                let value = self
                    .output_value(state, effective, &context)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("SucceedStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: Next::EndBranch,
                })
            }
            "Fail" => {
                self.push_entered("FailStateEntered", name, at, &raw);
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
            "Task" => self.task_state(name, state, raw, effective, &context, at),
            // `Parallel` is the only type left: the list above is exhaustive.
            _ => self.parallel_state(name, state, raw, effective, &context, at),
        }
    }

    /// A `Task` state whose `Resource` names an ACTIVITY: the state SCHEDULES a task
    /// for that activity and the walk suspends, because what happens next is a
    /// worker's reply.
    ///
    /// "An activity is a worker — a program — that you host", which "polls Step
    /// Functions using the GetActivityTask API action and responds using SendTask*
    /// API actions": the task carries the state's effective input, and the state's
    /// own `TimeoutSeconds`/`HeartbeatSeconds` bound how long it may stay
    /// outstanding. A `Resource` naming any OTHER work is the service-integration
    /// call [`Interpreter::task_state`] makes, which is what hands this one the
    /// states it is for.
    ///
    /// A walk CONTINUING from a worker's reply to this state's task applies that reply
    /// here instead of suspending: the reply is the state's result, so the state's own
    /// result-side data flow places it and the walk goes on to its `Next`.
    fn activity_task_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: &Value,
        effective: Value,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        let resource = task_resource(state);
        if !super::names_an_activity(resource) {
            return Err(runtime(format!(
                "state '{name}' invokes '{}', a service integration this emulator does not make, \
                 so nothing was run for it",
                echoed(resource)
            ))
            .at(at));
        }
        // A nested state list is walked to its own end inside the enclosing state's
        // turn, so there is nowhere for one of its states to wait: a `Map` iteration
        // or a `Parallel` branch that schedules a task is not run, rather than run as
        // though the task had already answered.
        if self.depth > 0 {
            return Err(runtime(format!(
                "state '{name}' schedules an activity task inside a 'Map' iteration or a \
                 'Parallel' branch, which this emulator does not wait at, so nothing was run \
                 for it"
            ))
            .at(at));
        }
        // Both bounds are read from the state's effective INPUT, which is what their
        // `*Path` forms are reference paths into — not from the `Parameters` document
        // built below.
        let timeout_seconds = self
            .task_seconds(state, "TimeoutSeconds", &effective, context)
            .map_err(|failure| failure.at(at))?;
        let heartbeat_seconds = self
            .task_seconds(state, "HeartbeatSeconds", &effective, context)
            .map_err(|failure| failure.at(at))?;
        // "Parameters ... Used to create a collection of key-value pairs that will be
        // passed as input": the document the template builds IS what the worker is
        // handed, and without one the state's effective input is.
        let input = match state.get("Parameters") {
            Some(template) => {
                payload_template(template, &effective, context).map_err(|failure| failure.at(at))?
            }
            None => effective,
        };
        self.push_entered("TaskStateEntered", name, at, raw);
        self.push("ActivityScheduled", name, at);
        // A worker has already answered THIS state's task: the reply is that task's
        // result — "the JSON output of the task" — so the state's own result-side data
        // flow (`ResultSelector`, `ResultPath`, `OutputPath`) is applied to it and the
        // walk carries on to the state's `Next`, instead of suspending again. The
        // steps up to here are what the call that suspended the walk already recorded,
        // which is what [`Resumed::index`] marks.
        let replied = match &self.reply {
            Some(reply) if reply.state_name == name => self.reply.take(),
            _ => None,
        };
        if let Some(reply) = replied {
            self.resumed = Some(Resumed {
                index: self.steps.len(),
                at,
            });
            self.push("ActivitySucceeded", name, at);
            let value = self
                .place_result(state, raw, reply.result, context, true)
                .map_err(|failure| failure.at(at))?;
            self.push_exited("TaskStateExited", name, at, &value);
            let next = self.next_of(name, state).map_err(|failure| failure.at(at))?;
            return Ok(StateStep { value, at, next });
        }
        self.waiting = Some(ScheduledActivity {
            activity_arn: resource.to_string(),
            state_name: name.to_string(),
            input: input.clone(),
            raw_input: raw.clone(),
            at,
            timeout_seconds,
            heartbeat_seconds,
        });
        Ok(StateStep {
            value: input,
            at,
            next: Next::Suspended,
        })
    }

    /// A `Task` state's `TimeoutSeconds` or `HeartbeatSeconds`, from the literal
    /// member or from the `<member>Path` reference path into the state's input.
    ///
    /// Both are "a positive integer" of seconds, so a value that is not one is a
    /// defect in the definition rather than a bound to be guessed at.
    fn task_seconds(
        &self,
        state: &Map<String, Value>,
        member: &str,
        input: &Value,
        context: &Value,
    ) -> Result<Option<u64>, Failure> {
        let path_member = format!("{member}Path");
        let declared = match (state.get(member), state.get(path_member.as_str())) {
            (Some(declared), _) => Some(declared.clone()),
            (None, Some(Value::String(path))) => Some(expression(path, input, context)?),
            (None, Some(other)) => {
                return Err(runtime(format!(
                    "'{path_member}' must be a JSONPath string, not {other}"
                )))
            }
            (None, None) => return Ok(None),
        };
        match declared.as_ref().and_then(Value::as_u64) {
            Some(seconds) if seconds > 0 => Ok(Some(seconds)),
            _ => Err(runtime(format!(
                "'{member}' must be a positive whole number of seconds"
            ))),
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

    /// The context object (`$$`) the states of ONE child workflow execution read:
    /// this walk's own, with the `Execution` members of the CHILD.
    ///
    /// A Distributed Map's item runs as an execution in its own right, so the states
    /// inside it read that execution's identity — its ARN, its name, the input it was
    /// started with and the instant it started — rather than the parent's. The state
    /// machine stays the parent's: a child "runs the same state machine" the Map Run
    /// reports, which is what `ListMapRuns` names in `stateMachineArn`. It has been
    /// redriven no times, because it has just been started.
    fn child_context(&self, arn: &str, name: &str, input: &Value, at: Duration) -> Value {
        let mut context = self.base_context.clone();
        if let Some(execution) = context.get_mut("Execution").and_then(Value::as_object_mut) {
            execution.insert("Id".to_string(), json!(arn));
            execution.insert("Name".to_string(), json!(name));
            execution.insert("Input".to_string(), input.clone());
            execution.insert("StartTime".to_string(), json!(iso(super::after(self.start, at))));
            execution.insert("RedriveCount".to_string(), json!(0));
        }
        context
    }

    /// A fresh name for one child workflow execution.
    ///
    /// Step Functions names a child workflow execution it starts for an item, so the
    /// name is drawn here, as a version-4 UUID — the same form, from the same
    /// generator, as the name the store mints for a `StartExecution` that gave none.
    /// The counter is what makes two draws in the same nanosecond differ.
    fn fresh_child_name(&mut self) -> String {
        self.child_counter = self.child_counter.wrapping_add(1);
        let mut seed = super::now_nanos() ^ self.child_counter.rotate_left(29);
        let high = super::splitmix64(&mut seed);
        let low = super::splitmix64(&mut seed);
        super::uuid_v4(high, low)
    }

    fn push(&mut self, event_type: &'static str, state_name: &str, after: Duration) {
        self.push_detailed(
            event_type,
            state_name,
            after,
            super::EventDetails::default(),
        );
    }

    /// The event a state records when it is ENTERED, carrying the data that state was
    /// entered WITH: "the string that contains the JSON input data for the state",
    /// which is the raw input the state received, before its own `InputPath` selects
    /// from it.
    fn push_entered(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        input: &Value,
    ) {
        self.push_detailed(
            event_type,
            state_name,
            after,
            super::EventDetails {
                input: Some(input.to_string()),
                ..super::EventDetails::default()
            },
        );
    }

    /// The event a state records when it is LEFT, carrying the data that state EMITTED:
    /// "the JSON output data of the state", which is the document the state's own
    /// result-side data flow produced — `ResultSelector`, then `ResultPath`, then
    /// `OutputPath` — and is the next state's input.
    fn push_exited(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        output: &Value,
    ) {
        self.push_detailed(
            event_type,
            state_name,
            after,
            super::EventDetails {
                output: Some(output.to_string()),
                ..super::EventDetails::default()
            },
        );
    }

    /// One history event carrying what the walk knew about it.
    fn push_detailed(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        details: super::EventDetails,
    ) {
        self.steps.push(Step {
            event_type,
            state_name: Some(state_name.to_string()),
            after,
            terminal: false,
            details,
        });
    }

    /// The event one attempt of a `Task` state's work failed with: the language's own
    /// timeout event where the work ran out of the time the state allowed it, and the
    /// task's own failure event otherwise, both carrying the work that failed and the
    /// error and cause it failed with.
    fn push_task_failure(
        &mut self,
        state_name: &str,
        resource_type: &str,
        action: &str,
        failure: &Failure,
        after: Duration,
    ) {
        let event_type = if failure.name() == TIMEOUT_ERROR {
            super::EVENT_TASK_TIMED_OUT
        } else {
            super::EVENT_TASK_FAILED
        };
        let mut details = task_work_details(resource_type, action);
        details.error = failure.error.clone();
        details.cause = failure.cause.clone();
        self.push_detailed(event_type, state_name, after, details);
    }

    /// The event one attempt of a run-a-job task failed to START its job with: "the
    /// task submission failed", which is a different event from the failure of a task
    /// whose work ran — and the same event a request-response task records when ITS
    /// call failed, because for that pattern the call IS the work.
    fn push_submit_failure(
        &mut self,
        state_name: &str,
        resource_type: &str,
        action: &str,
        failure: &Failure,
        after: Duration,
        call: &super::integration::Call,
    ) {
        if call.job.is_none() || failure.name() == TIMEOUT_ERROR {
            self.push_task_failure(state_name, resource_type, action, failure, after);
            return;
        }
        let mut details = task_work_details(resource_type, action);
        details.error = failure.error.clone();
        details.cause = failure.cause.clone();
        self.push_detailed(super::EVENT_TASK_SUBMIT_FAILED, state_name, after, details);
    }

    /// What the job a run-a-job task started ENDED as, placed as that state's own
    /// outcome.
    ///
    /// The job is the task's unit of work, so a job that did its work makes the
    /// task succeed with the report the invoked service gave of it — placed by the
    /// state's own result-side data flow, exactly as a request-response call's
    /// response is — and a job that ended without doing it makes the task fail, which
    /// the state's `Catch` then answers for.
    #[allow(clippy::too_many_arguments)]
    fn finished_job(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: &Value,
        work: (&str, &str),
        outcome: Result<Value, Failure>,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        let (resource_type, action) = work;
        let outcome = outcome.and_then(|result| {
            measure_payload(&result, &format!("the 'Task' state '{name}'")).map(|_bytes| result)
        });
        match outcome {
            Ok(result) => {
                let mut details = task_work_details(resource_type, action);
                details.output = Some(result.to_string());
                self.push_detailed(super::EVENT_TASK_SUCCEEDED, name, at, details);
                let value = self
                    .place_result(state, raw, result, context, true)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("TaskStateExited", name, at, &value);
                let next = self.next_of(name, state).map_err(|failure| failure.at(at))?;
                Ok(StateStep { value, at, next })
            }
            Err(failure) => {
                self.push_task_failure(name, resource_type, action, &failure, at);
                self.catch_or_fail((name, "TaskStateExited"), state, raw, failure, context, at)
            }
        }
    }

    /// A `Task` state's failure, recorded on the state that produced it and then
    /// reported to the walk.
    /// The failure of a task is the TASK's, so the history attributes it to the work
    /// that failed — the error name and the cause, on the state that named it —
    /// whatever the walk then does with it: a caller reads what went wrong off the
    /// task rather than only off the execution the task ended.
    fn task_fault(
        &mut self,
        state_name: &str,
        resource_type: &str,
        action: &str,
        failure: Failure,
        after: Duration,
    ) -> Fault {
        self.push_task_failure(state_name, resource_type, action, &failure, after);
        failure.at(after)
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
        let result = select_result(state, result, context, with_selector)?;
        let placed = path_result(state, raw, result)?;
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
    // Task
    // -----------------------------------------------------------------------

    /// Run a `Task` state: do the work its `Resource` names, and place what came of
    /// it.
    ///
    /// A `Task` state "represents a single unit of work performed by a state
    /// machine", and the `Resource` says which work. An ACTIVITY belongs to a worker,
    /// so that one is scheduled and the walk suspends at it
    /// ([`Interpreter::activity_task_state`]). Every other `Resource` names a service
    /// INTEGRATION, whose unit of work is one API call: its request is the document
    /// the state's `Parameters` payload template builds over the state's effective
    /// input (the effective input itself when it declares none), and the operation's
    /// response document is the state's RESULT, which the result-side filters then
    /// place. A call that failed is the state's own failure, so the state's `Retry`
    /// and `Catch` apply to it exactly as they apply to a `Map`'s or a `Parallel`'s
    /// work — and what no call is made for is not faked, but reported as the failure
    /// it is.
    fn task_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: Value,
        effective: Value,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        // The activity task is decided FIRST, before this state's own history event:
        // that path records its own, and the work there is a worker's rather than a
        // call this emulator makes.
        if super::names_an_activity(task_resource(state)) {
            return self.activity_task_state(name, state, &raw, effective, context, at);
        }
        self.push_entered("TaskStateEntered", name, at, &raw);
        let resource = state.get("Resource").and_then(Value::as_str).ok_or_else(|| {
            runtime(format!(
                "the 'Task' state '{name}' carries no string 'Resource', so it names no work"
            ))
            .at(at)
        })?;
        // "The service name of the resource in a task state" and "the action of the
        // resource called by a task state": what every one of this state's own events
        // names the work by, read once from the `Resource` they all describe.
        let (resource_type, action) = super::integration::named_work(resource);
        let call = super::integration::read(resource).map_err(|why| {
            self.task_fault(
                name,
                &resource_type,
                &action,
                runtime(format!("state '{name}': {why}")),
                at,
            )
        })?;
        // "TimeoutSeconds […] If the task runs longer than the specified seconds,
        // this state fails with a States.Timeout error name": the span it bounds is
        // the call's own, so the member is what the WAIT for the answer is bounded
        // by (`integration::Tasks::perform`), not merely what the span is compared
        // with afterwards. A state that declares none leaves the emulator's own
        // ceiling as the only bound.
        //
        // It is read through the SAME reader an activity task's bounds are read
        // with, which holds the member to the "positive integer" of seconds the
        // language defines: a figure that is not one is a defect in the definition,
        // and reading it as a whole number of seconds is what makes converting it to
        // a `Duration` total for every value a caller can write.
        let within = self
            .task_seconds(state, "TimeoutSeconds", &effective, context)
            .map_err(|failure| self.task_fault(name, &resource_type, &action, failure, at))?
            .map(Duration::from_secs);
        let heartbeat = self
            .task_seconds(state, "HeartbeatSeconds", &effective, context)
            .map_err(|failure| self.task_fault(name, &resource_type, &action, failure, at))?;
        // The job a run-a-job integration started has ENDED: what it ended as is this
        // state's own outcome, so the state reports it instead of starting the job a
        // second time. The events up to here are the ones the call that parked the
        // walk already recorded, which is what [`Resumed::index`] marks.
        let ended = match &self.job_reply {
            Some(reply) if reply.state_name == name => self.job_reply.take(),
            _ => None,
        };
        if let Some(ended) = ended {
            self.resumed = Some(Resumed {
                index: self.steps.len(),
                at,
            });
            return self.finished_job(
                name,
                state,
                &raw,
                (&resource_type, &action),
                ended.outcome,
                context,
                at,
            );
        }
        // A nested state list is walked to its own end inside the enclosing state's
        // turn, so there is nowhere for one of its states to park: a `Map` iteration
        // or a `Parallel` branch that would start a job is not run, rather than run
        // as though the job had already finished.
        if call.job.is_some() && self.depth > 0 {
            return Err(self.task_fault(
                name,
                &resource_type,
                &action,
                runtime(format!(
                    "state '{name}' starts a run-a-job integration inside a 'Map' iteration or a \
                     'Parallel' branch, which this emulator does not park at, so nothing was run \
                     for it"
                )),
                at,
            ));
        }
        // The run-a-job pattern is served only for the STANDARD workflow type, so an
        // EXPRESS execution fails AT this state and the integration's call is NOT
        // made: no job is started for a task the workflow type cannot wait at.
        if call.job.is_some() && self.express {
            return Err(self.task_fault(
                name,
                &resource_type,
                &action,
                express_run_a_job_refusal(name),
                at,
            ));
        }
        let (outcome, end) = self.with_retries(state, at, &mut |me, at| {
            // This attempt's own request reads `$$.State.RetryCount`, so the payload
            // template is rendered with the context of the attempt it is on.
            let attempt_context = with_retry_count(context, me.retry_count);
            let request = match state.get("Parameters") {
                Some(template) => match payload_template(template, &effective, &attempt_context) {
                    Ok(request) => request,
                    Err(failure) => {
                        me.push_task_failure(name, &resource_type, &action, &failure, at);
                        return (Err(failure), at);
                    }
                },
                None => effective.clone(),
            };
            // Each attempt hands the work over and starts it again, so each writes
            // its own pair of events, carrying the document THAT attempt built and
            // the bounds the state declared for it.
            me.push_detailed(
                super::EVENT_TASK_SCHEDULED,
                name,
                at,
                super::EventDetails {
                    resource_type: Some(resource_type.clone()),
                    resource: Some(action.clone()),
                    input: Some(request.to_string()),
                    timeout_seconds: within.map(|within| within.as_secs()),
                    heartbeat_seconds: heartbeat,
                    ..super::EventDetails::default()
                },
            );
            // The call is made AS the state machine's execution role, so what that
            // role may do decides whether it is made AT ALL: a call the policies in
            // force on the role do not allow is the invoked service's own
            // access-denied refusal and this task's own failure, reported before the
            // request goes anywhere — so nothing is started for it and no submission
            // is claimed.
            if let Some(refused) = me.tasks.authorize(&call, &request) {
                let failure = Failure {
                    error: Some(refused.error),
                    cause: Some(refused.cause),
                };
                me.push_task_failure(name, &resource_type, &action, &failure, at);
                return (Err(failure), at);
            }
            me.push_detailed(
                super::EVENT_TASK_STARTED,
                name,
                at,
                task_work_details(&resource_type, &action),
            );
            // The interpretation's own deadline is consulted HERE as well as at each
            // transition: the walk blocks inside a call rather than transitioning, so
            // a run already past its deadline makes no further call (and a `Retry`
            // cannot keep making them).
            if let Some(failure) = me.out_of_time() {
                me.push_submit_failure(name, &resource_type, &action, &failure, at, &call);
                return (Err(failure), at);
            }
            let answered = me.tasks.perform(&call, &request, within);
            // The identifier of the job a run-a-job call STARTED is read from the
            // answer as the invoked model spells it, before that answer becomes the
            // task's result in the integration's own convention
            // ([`super::integration::task_result`]).
            let started = answered.as_ref().ok().and_then(|answer| {
                let job = call.job?;
                answer
                    .get(job.job_id)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
            let attempt = match answered {
                // The result is a document a state carries, so it is measured
                // against the same ceiling every state's document is measured
                // against.
                Ok(answer) => {
                    let result = super::integration::task_result(&answer);
                    match measure_payload(&result, &format!("the 'Task' state '{name}'")) {
                        Ok(_bytes) => Ok(result),
                        Err(failure) => Err(failure),
                    }
                }
                Err(Failed { error, cause }) => Err(Failure {
                    error: Some(error),
                    cause: Some(cause),
                }),
            };
            match (&attempt, call.job) {
                // A run-a-job integration's call STARTED a job: the task is not
                // finished, so what the call answered is recorded as the task's
                // SUBMISSION ("the response from a resource when a task has
                // started") and the walk parks at the job until it ends.
                (Ok(result), Some(job)) => {
                    let Some(job_id) = started else {
                        let failure = runtime(format!(
                            "state '{name}': the '{}' call of {} answered no '{}', so the job it \
                             started cannot be reported and nothing was run for it",
                            call.operation, call.api.sdk_id, job.job_id
                        ));
                        me.push_submit_failure(name, &resource_type, &action, &failure, at, &call);
                        return (Err(failure), at);
                    };
                    let mut details = task_work_details(&resource_type, &action);
                    details.output = Some(result.to_string());
                    me.push_detailed(super::EVENT_TASK_SUBMITTED, name, at, details);
                    me.parked = Some(ParkedJob {
                        state_name: name.to_string(),
                        raw_input: raw.clone(),
                        job,
                        api: call.api,
                        parsed_output: call.parsed_output,
                        job_id: job_id.to_string(),
                        // The job is reported and stopped through calls of its own
                        // service, which for some integrations name it by a member of
                        // the request that STARTED it beside the identifier that call
                        // answered.
                        naming: job.naming(&request),
                        submitted: result.clone(),
                        at,
                        timeout_seconds: within.map(|within| within.as_secs()),
                    });
                }
                (Ok(result), None) => {
                    let mut details = task_work_details(&resource_type, &action);
                    details.output = Some(result.to_string());
                    me.push_detailed(super::EVENT_TASK_SUCCEEDED, name, at, details);
                }
                (Err(failure), _) => {
                    me.push_submit_failure(name, &resource_type, &action, failure, at, &call)
                }
            }
            (attempt, at)
        });
        match outcome {
            // The walk PARKED at the job this state's call started: the state has no
            // result yet, so the walk stops here and the execution waits for the job
            // ([`ParkedJob`]). The document the parked record carries is the one the
            // starting call answered, which is what the submission event reports.
            Ok(submitted) if self.parked.is_some() => Ok(StateStep {
                value: submitted,
                at: end,
                next: Next::Suspended,
            }),
            Ok(result) => {
                let value = self
                    .place_result(state, &raw, result, context, true)
                    .map_err(|failure| failure.at(end))?;
                self.push_exited("TaskStateExited", name, end, &value);
                Ok(StateStep {
                    value,
                    at: end,
                    next: self
                        .next_of(name, state)
                        .map_err(|failure| failure.at(end))?,
                })
            }
            Err(failure) => self.catch_or_fail(
                (name, "TaskStateExited"),
                state,
                &raw,
                failure,
                context,
                end,
            ),
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
        self.push_entered("MapStateEntered", name, at, &raw);
        // Which Map state of this walk this one is, in ENTRY order: the Map Run it may
        // start is recorded once its items are done, and the list it goes in is read
        // in this order rather than that one.
        let entered = self.map_runs_entered;
        self.map_runs_entered = self.map_runs_entered.saturating_add(1);
        let items = self
            .map_items(name, state, &effective, context)
            .map_err(|failure| failure.at(at))?;
        let (processor, distributed) = map_processor(name, state).map_err(|failure| failure.at(at))?;
        let (processor_states, processor_start) =
            states_of(processor, &format!("the 'ItemProcessor' of state '{name}'"))
                .map_err(|failure| failure.at(at))?;
        // "A user-defined or an auto-generated string that identifies a Map state":
        // the field the Map Run's own ARN carries, and the field the ARN of every
        // child execution it starts carries too, so one reading serves both.
        let label = state
            .get("Label")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string();
        let plan = MapPlan {
            items: &items,
            name,
            label: &label,
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

        // The tally belongs to the LAST attempt: a retried `Map` state resets it, so
        // what is reported is what the attempt the state ended on did.
        let mut tally = MapTally::default();
        let (outcome, end) = self.with_retries(state, at, &mut |me, at| {
            // Every attempt at this state iterates the items again, so every attempt
            // writes its own `MapStateStarted` — carrying "the size of the array for
            // Map state iterations" — and the event of the outcome those iterations
            // reached.
            me.push_detailed(
                super::EVENT_MAP_STATE_STARTED,
                name,
                at,
                super::EventDetails {
                    length: Some(i32::try_from(plan.items.len()).unwrap_or(i32::MAX)),
                    ..super::EventDetails::default()
                },
            );
            // This attempt's own members read `$$.State.RetryCount`, so they are
            // evaluated with the context of the attempt they are on.
            let context = with_retry_count(context, me.retry_count);
            let (outcome, end) = me.run_map_items(&plan, &effective, &context, at, &mut tally);
            let event = if outcome.is_ok() {
                super::EVENT_MAP_STATE_SUCCEEDED
            } else {
                super::EVENT_MAP_STATE_FAILED
            };
            me.push(event, name, end);
            (outcome, end)
        });
        // A Distributed Map "starts a Map Run", a resource of its own; an INLINE Map
        // starts none, so only the distributed mode records one.
        if distributed {
            // Held in the order the iterations FINISHED, which is the order the Map
            // Run counts them in: items of one batch run at once, so the one that
            // finished first is not always the one that started first.
            let mut finished_items = std::mem::take(&mut tally.items);
            finished_items.sort_by_key(|item| item.finished_at);
            // Where this state's Map Run belongs in the walk's list is decided by the
            // position its state holds among the Map states ENTERED, never by a
            // position in the list captured before the items ran: the same list is
            // pruned WHILE they run, by the failure of a nested `Parallel` or `Map`
            // ([`Interpreter::abort_units`]), so a position taken at entry no longer
            // names the same place.
            self.map_runs.push((
                entered,
                StartedMapRun {
                    label: label.clone(),
                    status: if outcome.is_ok() {
                        super::MAP_RUN_STATUS_SUCCEEDED
                    } else {
                        super::MAP_RUN_STATUS_FAILED
                    },
                    started_at: at,
                    stopped_at: end,
                    max_concurrency: super::reported_max_concurrency(
                        i64::try_from(plan.concurrency).unwrap_or(i64::MAX),
                    ),
                    tolerated_failure_count: i64::try_from(plan.tolerated_count)
                        .unwrap_or(i64::MAX),
                    tolerated_failure_percentage: plan.tolerated_percentage as f32,
                    total_items: i64::try_from(plan.items.len()).unwrap_or(i64::MAX),
                    items: finished_items,
                    // One child execution per item of the attempt the state ended
                    // on, which is what `ListExecutions` reports for this Map Run.
                    children: std::mem::take(&mut tally.children),
                },
            ));
        }
        match outcome {
            Ok(results) => {
                let value = self
                    .place_result(state, &raw, results, context, true)
                    .map_err(|failure| failure.at(end))?;
                self.push_exited("MapStateExited", name, end, &value);
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
    ///
    /// `tally` is counted up as the items are run, so it holds what this attempt's
    /// items did however the attempt ended.
    fn run_map_items(
        &mut self,
        plan: &MapPlan<'_>,
        effective: &Value,
        context: &Value,
        at: Duration,
        tally: &mut MapTally,
    ) -> Attempt {
        *tally = MapTally::default();
        let batch = if plan.concurrency == 0 {
            plan.items.len().max(1)
        } else {
            plan.concurrency
        };
        let mut results: Vec<Value> = Vec::new();
        let mut results_bytes = 0usize;
        let mut failures: u64 = 0;
        // The iteration that failed FIRST, with the offset it failed at: the failure
        // an intolerant state reports, and the instant its other iterations stopped.
        let mut first_failure: Option<(Failure, Duration)> = None;
        // Where each iteration's own events begin in this walk's list, so an iteration
        // the state's failure aborted can be cut back to the instant it failed.
        let mut units: Vec<Unit> = Vec::new();
        // "ToleratedFailureCount" and "ToleratedFailurePercentage" are what lets a
        // state go on iterating past a failed item; a state that declares neither
        // tolerates none, so the first failure is the state's own and the iterations
        // that had not finished stop there.
        let tolerant = plan.distributed || plan.tolerance_declared;
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
                // A DISTRIBUTED Map "starts a child workflow execution for each
                // item", so this item's iteration runs as an execution of its own:
                // its ARN is minted HERE, before the iteration, because the item's
                // states read it through `$$.Execution.Id` — and the context they
                // read is the CHILD's, not the parent's. An inline Map starts no
                // execution, so its iteration keeps the parent's context.
                let child = plan.distributed.then(|| {
                    let name = self.fresh_child_name();
                    let arn = super::execution_arn(
                        &self.region,
                        &format!("{}/{}", self.machine_name, plan.label),
                        &name,
                    );
                    (name, arn)
                });
                let enclosing_context = child.as_ref().map(|(name, arn)| {
                    let child_context =
                        self.child_context(arn, name, &item_input, batch_start);
                    std::mem::replace(&mut self.base_context, child_context)
                });
                // The child's own `input` is the document this iteration was handed,
                // kept before the walk takes it: "the string that contains the JSON
                // input data" of that execution.
                let child_input = child.as_ref().map(|_| item_input.to_string());
                // One iteration per item: it is STARTED here, carrying the index of
                // the item it was handed, and records its own outcome below — which is
                // how a caller observes the fan-out, and how many items ran at once.
                let from = self.steps.len();
                self.push_detailed(
                    super::EVENT_MAP_ITERATION_STARTED,
                    plan.name,
                    batch_start,
                    iteration_details(index),
                );
                units.push(Unit {
                    from,
                    map_runs_from: self.map_runs.len(),
                    aborted: Some(Step {
                        event_type: super::EVENT_MAP_ITERATION_ABORTED,
                        state_name: Some(plan.name.to_string()),
                        after: Duration::ZERO,
                        terminal: false,
                        details: iteration_details(index),
                    }),
                });
                let branch = self.nested_walk(
                    plan.states,
                    plan.start_at,
                    item_input,
                    batch_start,
                    &scope,
                );
                if let Some(enclosing) = enclosing_context {
                    self.base_context = enclosing;
                }
                batch_end = batch_end.max(branch.end);
                if let Some((name, arn)) = child {
                    // The child execution this item ran as, as it ended: an item that
                    // produced a result is an execution that SUCCEEDED with that
                    // result as its output, and one whose iteration failed is an
                    // execution that FAILED with that failure's own error and cause.
                    let (status, output, error, cause) = match &branch.outcome {
                        Ok(value) => (
                            super::EXECUTION_STATUS_SUCCEEDED,
                            Some(value.to_string()),
                            None,
                            None,
                        ),
                        Err(failure) => (
                            super::EXECUTION_STATUS_FAILED,
                            None,
                            failure.error.clone(),
                            failure.cause.clone(),
                        ),
                    };
                    tally.children.push(StartedChildExecution {
                        name,
                        arn,
                        input: child_input.unwrap_or_default(),
                        output,
                        error,
                        cause,
                        status,
                        started_at: batch_start,
                        finished_at: branch.end,
                        // One item per child: see [`StartedChildExecution::item_count`].
                        item_count: 1,
                    });
                }
                let outcome_event = match &branch.outcome {
                    Ok(_) => super::EVENT_MAP_ITERATION_SUCCEEDED,
                    Err(_) => super::EVENT_MAP_ITERATION_FAILED,
                };
                self.push_detailed(
                    outcome_event,
                    plan.name,
                    branch.end,
                    iteration_details(index),
                );
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
                        tally.items.push(MapItemOutcome {
                            finished_at: branch.end,
                            failed: false,
                        });
                    }
                    Err(failure) => {
                        failures += 1;
                        tally.items.push(MapItemOutcome {
                            finished_at: branch.end,
                            failed: true,
                        });
                        if first_failure
                            .as_ref()
                            .is_none_or(|(_, when)| branch.end < *when)
                        {
                            first_failure = Some((failure, branch.end));
                        }
                    }
                }
                index += 1;
            }
            at = batch_end;
            // A state that tolerates no failure has already failed, so the items it
            // had not started yet are never started.
            if failures > 0 && !tolerant {
                break;
            }
        }

        if failures == 0 {
            return (Ok(Value::Array(results)), at);
        }
        if super::within_tolerated_failures(
            i64::try_from(failures).unwrap_or(i64::MAX),
            i64::try_from(plan.items.len()).unwrap_or(i64::MAX),
            i64::try_from(plan.tolerated_count).unwrap_or(i64::MAX),
            plan.tolerated_percentage,
        ) {
            return (Ok(Value::Array(results)), at);
        }
        // A Distributed Map reports its own tolerated-failure error; an inline Map
        // has no threshold to exceed, so the iteration's own failure is the state's.
        if tolerant {
            return (
                Err(named(
                    TOLERATED_FAILURE_ERROR,
                    format!(
                        "{failures} of {} items failed, more than this 'Map' state tolerates",
                        plan.items.len()
                    ),
                )),
                at,
            );
        }
        let Some((failure, when)) = first_failure else {
            return (Err(runtime("a 'Map' iteration failed".to_string())), at);
        };
        self.abort_units(when, &units);
        (Err(failure), when)
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
        self.push_entered("ParallelStateEntered", name, at, &raw);
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
            // Every attempt at this state starts its branches, so every attempt
            // writes its own `ParallelStateStarted` and the event of the outcome
            // those branches reached.
            me.push(super::EVENT_PARALLEL_STATE_STARTED, name, at);
            // "Parameters ... The effective input each branch receives." It is
            // re-evaluated per attempt, because this attempt's own members read
            // `$$.State.RetryCount`.
            let context = with_retry_count(context, me.retry_count);
            let branch_input = match state.get("Parameters") {
                Some(template) => match payload_template(template, &effective, &context) {
                    Ok(value) => value,
                    Err(failure) => {
                        me.push(super::EVENT_PARALLEL_STATE_FAILED, name, at);
                        return (Err(failure), at);
                    }
                },
                None => effective.clone(),
            };
            let (outcome, end) = me.run_branches(&plan, &branch_input, at);
            let event = if outcome.is_ok() {
                super::EVENT_PARALLEL_STATE_SUCCEEDED
            } else {
                super::EVENT_PARALLEL_STATE_FAILED
            };
            me.push(event, name, end);
            (outcome, end)
        });
        match outcome {
            Ok(results) => {
                let value = self
                    .place_result(state, &raw, results, context, true)
                    .map_err(|failure| failure.at(end))?;
                self.push_exited("ParallelStateExited", name, end, &value);
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
    ///
    /// "If any branch fails ... the entire Parallel state is considered to have
    /// failed", and it failed at the instant that branch did: the branches that were
    /// still running are STOPPED there ([`Interpreter::abort_units`]), so no state of
    /// a surviving branch is entered after the state the Parallel failed with.
    fn run_branches(&mut self, plan: &ParallelPlan<'_>, input: &Value, at: Duration) -> Attempt {
        let mut results = Vec::with_capacity(plan.branches.len());
        let mut results_bytes = 0usize;
        let mut end = at;
        // Where each branch's own events begin in this walk's list, so a branch the
        // state's failure aborted can be cut back to the instant it failed.
        let mut units: Vec<Unit> = Vec::with_capacity(plan.branches.len());
        let mut failed: Option<(Failure, Duration)> = None;
        for (states, start_at) in &plan.branches {
            let from = self.steps.len();
            let map_runs_from = self.map_runs.len();
            let branch = self.nested_walk(states, start_at, input.clone(), at, &Scope::default());
            units.push(Unit {
                from,
                map_runs_from,
                aborted: None,
            });
            match branch.outcome {
                Ok(value) => {
                    match measure_payload(&value, "a 'Parallel' branch") {
                        Ok(bytes) => results_bytes = results_bytes.saturating_add(bytes),
                        Err(failure) => return (Err(failure), end.max(branch.end)),
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
                            end.max(branch.end),
                        );
                    }
                    end = end.max(branch.end);
                    results.push(value);
                }
                // The state failed with the error of the branch that failed FIRST,
                // which is the one whose failure stopped the others.
                Err(failure) => {
                    if failed.as_ref().is_none_or(|(_, when)| branch.end < *when) {
                        failed = Some((failure, branch.end));
                    }
                }
            }
        }
        let Some((failure, when)) = failed else {
            return (Ok(Value::Array(results)), end);
        };
        self.abort_units(when, &units);
        (Err(failure), when)
    }

    /// Stop the branches or iterations of a state that has FAILED, at the instant
    /// `when` it failed.
    ///
    /// A state whose concurrent work has already failed does not go on running the
    /// rest of it: each unit is cut back to the events that were due at or before that
    /// instant, the state it was in when the failure reached it records that type's own
    /// aborted event, and the unit itself records the one its kind has. `units` is
    /// where each unit's events begin in this walk's list, in the order they were run,
    /// so they are cut from the last one back and no earlier unit's position moves.
    fn abort_units(&mut self, when: Duration, units: &[Unit]) {
        for (position, unit) in units.iter().enumerate().rev() {
            // Every bound is clamped to the list as it stands: a later unit has
            // already been cut back by the time this one is, so the span this reads
            // is the one the list still holds.
            let until = units
                .get(position + 1)
                .map_or(self.steps.len(), |next| next.from)
                .min(self.steps.len());
            let from = unit.from.min(until);
            let Some(cut) = (from..until).find(|index| {
                self.steps
                    .get(*index)
                    .is_some_and(|step| step.after > when)
            }) else {
                continue;
            };
            // The states this unit was still inside when the failure reached it: each
            // one entered and not yet left, innermost last.
            let mut open: Vec<(&'static str, Option<String>)> = Vec::new();
            for step in self.steps.get(from..cut).unwrap_or_default() {
                if let Some(entered) = step.event_type.strip_suffix(STATE_ENTERED) {
                    open.push((entered, step.state_name.clone()));
                } else if let Some(left) = step.event_type.strip_suffix(STATE_EXITED) {
                    if let Some(index) = open.iter().rposition(|(name, _)| *name == left) {
                        open.remove(index);
                    }
                }
            }
            let mut stopped: Vec<Step> = Vec::new();
            for (entered, state_name) in open.iter().rev() {
                if let Some(event_type) = aborted_event(entered) {
                    stopped.push(Step {
                        event_type,
                        state_name: state_name.clone(),
                        after: when,
                        terminal: false,
                        details: super::EventDetails::default(),
                    });
                }
            }
            if let Some(aborted) = &unit.aborted {
                let mut aborted = aborted.clone();
                aborted.after = when;
                stopped.push(aborted);
            }
            self.steps.splice(cut..until, stopped);
        }
        // A Map Run one of THESE units only started after the state had already failed
        // never started: the state it belongs to was not entered. The list also holds
        // the Map Runs of work that finished before these units began — an earlier
        // `Parallel` branch's, the walk's own — whose own instants this failure says
        // nothing about, so only what these units recorded is judged.
        let recorded_from = units
            .first()
            .map_or(self.map_runs.len(), |unit| unit.map_runs_from)
            .min(self.map_runs.len());
        let mut recorded = self.map_runs.split_off(recorded_from);
        recorded.retain(|(_, map_run)| map_run.started_at <= when);
        self.map_runs.append(&mut recorded);
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
                at += Duration::from_secs_f64(retry_delay_seconds(retrier, taken));
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
            self.push_exited(left, name, at, &value);
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

/// The members every event of a `Task` state's own work reports: "the service name of
/// the resource in a task state" and "the action of the resource called by a task
/// state", as [`super::integration::named_work`] read them off the `Resource`.
fn task_work_details(resource_type: &str, action: &str) -> super::EventDetails {
    super::EventDetails {
        resource_type: Some(resource_type.to_string()),
        resource: Some(action.to_string()),
        ..super::EventDetails::default()
    }
}

/// The member one `Map` iteration's own events report beside the state's name: "the
/// index of the array belonging to the Map state iteration".
fn iteration_details(index: usize) -> super::EventDetails {
    super::EventDetails {
        index: Some(i32::try_from(index).unwrap_or(i32::MAX)),
        ..super::EventDetails::default()
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

/// How long a retrier waits before the attempt after `taken` ones:
/// "IntervalSeconds" scaled by "BackoffRate" once per attempt already taken, capped
/// by `MaxDelaySeconds` where the retrier declares one.
///
/// The product is of two numbers the definition chose, so it can overflow to infinity
/// (or to NaN, when a zero interval meets an infinite backoff). A wait no longer than
/// the longest one this service waits for is what such a retrier gets: the figure
/// this returns is always finite and in range, which is what makes converting it to a
/// `Duration` total. An attempt count past `i32::MAX` — which `MaxAttempts` and the
/// step budget both make unreachable — scales to that same maximum rather than to the
/// bare interval.
fn retry_delay_seconds(retrier: &Map<String, Value>, taken: u64) -> f64 {
    let interval =
        finite_seconds(retrier, "IntervalSeconds").unwrap_or(DEFAULT_RETRY_INTERVAL_SECONDS);
    let backoff = finite_seconds(retrier, "BackoffRate").unwrap_or(DEFAULT_RETRY_BACKOFF_RATE);
    let mut delay = interval * backoff.powi(i32::try_from(taken).unwrap_or(i32::MAX));
    if let Some(cap) = finite_seconds(retrier, "MaxDelaySeconds") {
        delay = delay.min(cap);
    }
    if delay.is_finite() {
        delay.clamp(0.0, MAX_WAIT_SECONDS as f64)
    } else {
        MAX_WAIT_SECONDS as f64
    }
}

/// Which retrier of the `Task` state `state_name` of `definition` retries an activity
/// task that failed with `error`, and how long after that failure its next attempt is
/// due.
///
/// `taken` is how many attempts each retrier of that state has already spent on the
/// task, in the order the state declares them. The answer is the FIRST retrier whose
/// `ErrorEquals` names the error, exactly as a retried walk's is, so one rule decides
/// every retry this service makes.
///
/// `None` when the definition declares no such state, when the state declares no
/// `Retry`, when no retrier names the error, or when the one that does has spent its
/// `MaxAttempts`: the failure is then the state's own.
pub fn activity_retry(
    definition: &str,
    state_name: &str,
    error: &str,
    taken: &[u64],
) -> Option<(usize, Duration)> {
    let document = serde_json::from_str::<Value>(definition).ok()?;
    let state = document
        .get("States")
        .and_then(Value::as_object)?
        .get(state_name)
        .and_then(Value::as_object)?;
    let retriers = state.get("Retry").and_then(Value::as_array)?;
    for (index, retrier) in retriers.iter().enumerate() {
        let Some(retrier) = retrier.as_object() else {
            continue;
        };
        if !errors_match(retrier, error) {
            continue;
        }
        let max_attempts = retrier
            .get("MaxAttempts")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_RETRY_MAX_ATTEMPTS);
        let spent = taken.get(index).copied().unwrap_or(0);
        if spent >= max_attempts {
            return None;
        }
        return Some((
            index,
            Duration::from_secs_f64(retry_delay_seconds(retrier, spent)),
        ));
    }
    None
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

/// `ResultSelector` applied to a state's raw result, when the state declares one and
/// its type has one (`with_selector`).
fn select_result(
    state: &Map<String, Value>,
    result: Value,
    context: &Value,
    with_selector: bool,
) -> Result<Value, Failure> {
    match state.get("ResultSelector").filter(|_| with_selector) {
        Some(template) => payload_template(template, &result, context),
        None => Ok(result),
    }
}

/// `ResultPath` applied to a state's result: where in the state's own RAW input the
/// result is placed.
fn path_result(
    state: &Map<String, Value>,
    raw: &Value,
    result: Value,
) -> Result<Value, Failure> {
    match state.get("ResultPath") {
        // "ResultPath (Optional) ... The default value is $, which matches the
        // entire input."
        None => Ok(result),
        // "If the ResultPath is null, that means that the state's own raw input is
        // the state's output."
        Some(Value::Null) => Ok(raw.clone()),
        Some(Value::String(path)) => set_at_reference_path(raw, path, result),
        Some(other) => Err(runtime(format!(
            "'ResultPath' must be a reference-path string or null, not {other}"
        ))),
    }
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
                        let built = template_expression(key, text, input, context)?;
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

/// Evaluate the value of a `"<name>.$"` member of a payload template, saying which
/// member a path that selected nothing was written for.
///
/// A template member names the field it builds, so that is what the service reports a
/// path it could not resolve against: "The JSONPath '<path>' specified for the field
/// '<field>' could not be found in the input '<document>'". The document is the one the
/// path was evaluated against — the context object for a path that begins with `$$`,
/// the template's own input for every other — quoted under the one clipping every
/// caller value in a diagnostic gets.
fn template_expression(
    field: &str,
    text: &str,
    input: &Value,
    context: &Value,
) -> Result<Value, Failure> {
    if text.starts_with("States.") {
        return intrinsic(text, input, context);
    }
    select_optional(text, input, context)?.ok_or_else(|| {
        let searched = if is_context_path(text) { context } else { input };
        runtime(format!(
            "The JSONPath '{}' specified for the field '{}' could not be found in the input '{}'",
            echoed(text),
            echoed(field),
            echoed(&searched.to_string())
        ))
    })
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
    // "paths that begin with $$ ... are evaluated against the Context Object", with
    // the first `$` naming that object and the rest being the path into it.
    if is_context_path(path) {
        return select_path(&path[1..], context);
    }
    match path.strip_prefix('$') {
        Some(_) => select_path(path, input),
        None => Err(runtime(format!(
            "'{path}' is not a JSONPath: a path begins with '$'"
        ))),
    }
}

/// Whether `path` is evaluated against the context object rather than against the
/// state's data: "paths that begin with $$ ... are evaluated against the Context
/// Object."
fn is_context_path(path: &str) -> bool {
    path.starts_with("$$")
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
/// (`..`), a wildcard, a union or a filter would walk the whole document, or multiply
/// the node list by its own length, and is refused here.
///
/// That guard is STRUCTURAL, not textual: the engine is handed
/// [`canonical_path`]'s rendering of the segments the validator produced, never the
/// caller's own text, so no syntax the validator does not produce can reach the
/// engine however the caller's text would otherwise have parsed. The node count is
/// still checked afterwards, as defence in depth.
fn select_path(expression: &str, root: &Value) -> Result<Option<Value>, Failure> {
    if expression.len() > MAX_PATH_CHARS {
        return Err(runtime(format!(
            "a path of {} bytes is longer than the {MAX_PATH_CHARS} this service evaluates",
            expression.len()
        )));
    }
    let segments = reference_segments(expression).map_err(|why| {
        runtime(format!(
            "'{expression}' is not a reference path this service evaluates: {why}"
        ))
    })?;
    let canonical = canonical_path(&segments);
    let parsed = JsonPath::parse(&canonical).map_err(|error| {
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
///
/// A segment names exactly ONE place, which is what makes a reference path's cost its
/// segment count: the wildcard `*` and a comma-separated union are therefore refused
/// here in both the `.name` and the `['name']` form, since either one would select a
/// list of nodes whose length the path text does not bound.
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
            if name.contains(SELECTOR_CHARS) {
                return Err(format!(
                    "'{path}' carries the member name '{name}', and a reference path names one \
                     member rather than a wildcard or a list of them"
                ));
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
                // A subscript still carrying a quote or a comma inside the quotes it
                // was stripped of is a UNION of names, which selects as many nodes as
                // it lists; `*` is the wildcard. Neither names one place, so neither
                // is a reference path, whatever the two outer quotes suggest.
                Some(name) if name.contains(SELECTOR_CHARS) => {
                    return Err(format!(
                        "'{path}' carries the subscript '{inside}', and a reference path names one \
                         member rather than a wildcard or a list of them"
                    ));
                }
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

/// The characters a member name of a reference path may not carry, because each one
/// turns a single-node selector into one that selects a list: the wildcard `*`, the
/// union separator `,` and the two quotes a union's members are written in.
const SELECTOR_CHARS: &[char] = &['*', ',', '\'', '"'];

/// The canonical reference path for `segments`: `$['name'][0]`, with every member
/// name quoted and every quote and backslash inside it escaped.
///
/// This is the ONLY expression handed to the JSONPath engine. The caller's path text
/// is validated into segments and then WRITTEN OUT again from them, so the engine
/// evaluates a path this module produced rather than one a caller wrote: a text that
/// would otherwise have parsed as a union, a wildcard, a slice or a filter cannot
/// reach it, and the node list it builds is bounded by the segment count the
/// validator counted.
fn canonical_path(segments: &[Segment]) -> String {
    let mut path = String::from("$");
    for segment in segments {
        match segment {
            Segment::Key(key) => {
                path.push_str("['");
                for character in key.chars() {
                    if character == '\\' || character == '\'' {
                        path.push('\\');
                    }
                    path.push(character);
                }
                path.push_str("']");
            }
            Segment::Index(index) => {
                path.push('[');
                path.push_str(&index.to_string());
                path.push(']');
            }
        }
    }
    path
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
/// past either is reported with [`RUNTIME_ERROR`] rather than parsed.
fn invoke_intrinsic(
    text: &str,
    input: &Value,
    context: &Value,
    depth: usize,
) -> Result<Value, Failure> {
    let text = text.trim();
    if depth > MAX_EXPRESSION_DEPTH {
        return Err(runtime(format!(
            "an intrinsic-function invocation nests more than the {MAX_EXPRESSION_DEPTH} \
             levels this service evaluates"
        )));
    }
    if text.len() > MAX_EXPRESSION_CHARS {
        return Err(runtime(format!(
            "an intrinsic-function invocation of {} bytes is longer than the \
             {MAX_EXPRESSION_CHARS} this service evaluates",
            text.len()
        )));
    }
    let open = text
        .find('(')
        .ok_or_else(|| runtime(format!("'{text}' is not an intrinsic-function invocation")))?;
    if !text.ends_with(')') {
        return Err(runtime(format!(
            "the intrinsic invocation '{text}' is not closed"
        )));
    }
    let name = text[..open].trim();
    let arguments = split_arguments(&text[open + 1..text.len() - 1])
        .map_err(|why| runtime(format!("'{text}': {why}")))?;
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
            return Err(runtime(format!(
                "'{name}' was given at least {spent} bytes of arguments, more than the \
                 {MAX_PAYLOAD_BYTES} a state's output may carry"
            )));
        }
        values.push(value);
    }
    let result =
        apply_intrinsic(name, &values).map_err(|why| runtime(format!("'{text}': {why}")))?;
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
        runtime(format!(
            "'{text}' is not a path, a quoted string or a JSON literal: {error}"
        ))
    })
}

/// Apply one intrinsic function to its evaluated arguments.
///
/// The set below is every documented intrinsic except `States.Hash`, which is
/// DECLINED: the algorithms it names include MD5 and SHA-1, and this emulator does
/// not take a dependency on broken hash functions to evaluate a state's payload
/// template. Like any other name this closed allow-list does not carry, it is
/// reported with [`RUNTIME_ERROR`] rather than approximated.
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
// One state, tested by itself (`TestState`)
// ---------------------------------------------------------------------------

/// How a failure of the ONE state `TestState` tests names it: the state the request
/// carries alone has no name of its own, so what a failure it raised says about it
/// names it as the state under test rather than inventing a name for it.
pub const SINGLE_STATE_LABEL: &str = "the state under test";

/// `TestExecutionStatus`' `SUCCEEDED`: the state's work produced an output
/// (`smithy_query(query="shape",shape="TestExecutionStatus")` -> `CAUGHT_ERROR |
/// FAILED | RETRIABLE | SUCCEEDED`).
pub const TEST_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// `TestExecutionStatus`' `FAILED`: the state failed, and neither a `Retry` nor a
/// `Catch` of its own answers for the error it failed with.
pub const TEST_STATUS_FAILED: &str = "FAILED";

/// `TestExecutionStatus`' `RETRIABLE`: the state failed with an error one of its
/// `Retry` retriers matches and still has attempts left for, so the state machine
/// would re-run it.
pub const TEST_STATUS_RETRIABLE: &str = "RETRIABLE";

/// `TestExecutionStatus`' `CAUGHT_ERROR`: the state failed and one of its `Catch`
/// catchers took the failure over, so the execution continues at that catcher's
/// `Next` with the error output.
pub const TEST_STATUS_CAUGHT_ERROR: &str = "CAUGHT_ERROR";

/// The state types whose `Parameters` member is a payload template over the state's
/// effective INPUT (see [`executable_members`]); the other types declare no such
/// member, so no `afterParameters` stage exists for them.
const TEMPLATED_STATE_TYPES: &[&str] = &["Map", "Parallel", "Pass", "Task"];

/// The state types whose `ResultSelector` member is a payload template over the
/// state's raw RESULT. `Pass` is absent: the Amazon States Language gives it a
/// `Result`, not a `ResultSelector`.
const SELECTOR_STATE_TYPES: &[&str] = &["Map", "Parallel", "Task"];

/// The state types whose own work is an INVOCATION — a service integration, an
/// iteration over items, a set of branches — rather than a function of the state's
/// data alone.
///
/// "TestState only supports the following when a mock is specified: Activity tasks,
/// .sync or .waitForTaskToken service integration patterns, Parallel, or Map states":
/// a state of one of these types is tested through its mocked outcome, and the
/// operation refuses to test a `Map` or a `Parallel` without one.
pub const INVOKING_STATE_TYPES: &[&str] = &["Map", "Parallel", "Task"];

/// A mocked outcome for the state under test: "A JSON object that contains a mocked
/// `result` or `errorOutput`", which stands in for the invocation the state would
/// otherwise make. Nothing is invoked for a mocked state; what the mock names IS the
/// state's result or failure, and the state's own data flow is applied to it.
#[derive(Debug, Clone)]
pub enum Mock {
    /// `mock.result`: "A JSON string containing the mocked result of the state
    /// invocation."
    Result(Value),
    /// `mock.errorOutput`: "the mocked response is returned as a JSON object that
    /// contains an `error` and `cause` field."
    Error(Failure),
}

/// What `TestState` reports about one state's data processing: the document the state
/// carried after each filter of the Amazon States Language's data flow.
///
/// A stage a state has no member for is left unreported rather than repeated, and the
/// result-side stages exist only for a state that produces a result of its own.
#[derive(Debug, Clone, Default)]
pub struct Inspection {
    /// "The raw state input", as the caller wrote it.
    pub input: Option<String>,
    /// "The input after Step Functions applies the InputPath filter."
    pub after_input_path: Option<String>,
    /// "The effective input after Step Functions applies the Parameters filter."
    pub after_parameters: Option<String>,
    /// "The state's raw result."
    pub result: Option<String>,
    /// "The effective result after Step Functions applies the ResultSelector filter."
    pub after_result_selector: Option<String>,
    /// "The effective result combined with the raw state input after Step Functions
    /// applies the ResultPath filter."
    pub after_result_path: Option<String>,
}

/// What one state, tested by itself, produced.
#[derive(Debug, Clone)]
pub struct Trial {
    /// One of [`TEST_STATUS_SUCCEEDED`], [`TEST_STATUS_FAILED`],
    /// [`TEST_STATUS_RETRIABLE`] and [`TEST_STATUS_CAUGHT_ERROR`].
    pub status: &'static str,
    pub output: Option<String>,
    /// Where the state transitions to. "If you haven't defined a next state in your
    /// definition or if the execution of the state fails, this field doesn't contain
    /// a value."
    pub next_state: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub inspection: Inspection,
    /// The position in `Retry` of the retrier that matched a [`TEST_STATUS_RETRIABLE`]
    /// failure, and how long that retrier waits before the attempt it owes.
    pub retry_index: Option<i32>,
    pub retry_backoff_seconds: Option<i32>,
    /// The position in `Catch` of the catcher that took a
    /// [`TEST_STATUS_CAUGHT_ERROR`] failure over.
    pub catch_index: Option<i32>,
}

impl Trial {
    /// The trial of a state that produced `output`.
    fn succeeded(output: Option<String>, next_state: Option<String>, inspection: Inspection) -> Self {
        Self {
            status: TEST_STATUS_SUCCEEDED,
            output,
            next_state,
            error: None,
            cause: None,
            inspection,
            retry_index: None,
            retry_backoff_seconds: None,
            catch_index: None,
        }
    }

    /// The trial of a state that failed with nothing of its own to answer for it.
    ///
    /// `error` and `cause` leave through the modelled `SensitiveError` (0..256) and
    /// `SensitiveCause` (0..32768) shapes, so they are bounded exactly as an
    /// execution's are: a name past its maximum is reported as ASL's own data-limit
    /// error, since a name carries meaning and cannot be shortened, while an
    /// over-long cause keeps the error name and is clipped to what the service
    /// reports.
    fn failed(inspection: Inspection, failure: Failure) -> Self {
        let Failure { error, cause } = failure;
        if error
            .as_ref()
            .is_some_and(|error| error.len() > MAX_ERROR_BYTES)
        {
            return Self {
                status: TEST_STATUS_FAILED,
                output: None,
                next_state: None,
                error: Some(DATA_LIMIT_ERROR.to_string()),
                cause: Some(format!(
                    "the state's error name is longer than the {MAX_ERROR_BYTES} bytes this \
                     service reports it through"
                )),
                inspection,
                retry_index: None,
                retry_backoff_seconds: None,
                catch_index: None,
            };
        }
        Self {
            status: TEST_STATUS_FAILED,
            output: None,
            next_state: None,
            error,
            cause: cause.map(clip_cause),
            inspection,
            retry_index: None,
            retry_backoff_seconds: None,
            catch_index: None,
        }
    }
}

/// One `TestState` call: the state under test, and everything the request fixed
/// about it.
pub struct StateTest<'a> {
    /// The state's name, which `$$.State.Name` reports. Empty for a `definition`
    /// that is the state alone, which carries no name — only a `stateName` naming a
    /// state of a full definition gives it one.
    pub name: &'a str,
    /// The state's own definition: the object the Amazon States Language declares it
    /// as.
    pub state: &'a Map<String, Value>,
    /// "A string that contains the JSON input data for the state", as the caller
    /// wrote it.
    pub input: &'a str,
    /// When the state is being tested, which `$$.State.EnteredTime` reports.
    pub start: SystemTime,
    /// What the context object reports about this test, for the members the request
    /// fixes.
    pub context: &'a Context,
    /// "A JSON string representing a valid Context object for the state under test":
    /// the whole `$$` document, which replaces the one built from `context`.
    pub context_override: Option<&'a Value>,
    pub mock: Option<&'a Mock>,
    /// `stateConfiguration.retrierRetryCount`: "The number of retry attempts that
    /// have occurred for the state's Retry that applies to the mocked error", which
    /// is what decides whether the state's failure is still RETRIABLE.
    pub retries_taken: u64,
}

/// Run the ONE state `test` names over its input and report what it produced.
///
/// This is `TestState`'s walk: a single state, with no state machine around it, so
/// nothing is transitioned to and nothing is persisted. The state's data flow, its
/// `Choice` rules, its `Wait` members and its `Fail` members are evaluated by the
/// same code an execution's walk uses; an invocation this process cannot make is
/// answered by the request's `mock`, and without one the state fails with ASL's own
/// [`RUNTIME_ERROR`] rather than with a fabricated result.
pub fn test_state(test: &StateTest<'_>) -> Trial {
    let inspection = Inspection {
        input: Some(test.input.to_string()),
        ..Inspection::default()
    };
    let raw: Value = match serde_json::from_str(test.input) {
        Ok(value) => value,
        Err(why) => {
            return Trial::failed(
                inspection,
                runtime(format!(
                    "the state input is not valid JSON: {}",
                    crate::state::json_parse_diagnostic(test.input, &why)
                )),
            );
        }
    };
    // The input is a caller's document too, so it is measured against the ceiling
    // every document a state produces is measured against: an input larger than a
    // state's output may carry is answered with ASL's own data-limit error rather
    // than evaluated, which is also what bounds what one path or one `StringMatches`
    // rule can be asked to walk.
    if let Err(failure) = measure_payload(&raw, "the state's input") {
        return Trial::failed(inspection, failure);
    }
    let base_context = match test.context_override {
        Some(context) => context.clone(),
        None => test.context.value(&raw, test.start),
    };
    let mut interpreter = Interpreter {
        budget: MAX_STATE_TRANSITIONS,
        depth: 0,
        deadline: super::after(super::now(), MAX_INTERPRET_DURATION),
        // The state is tested as the attempt the request says it is on, which is what
        // it reads through `$$.State.RetryCount`.
        retry_count: test.retries_taken,
        steps: Vec::new(),
        start: test.start,
        base_context,
        // `TestState` tests ONE state, with no execution around it: a state whose
        // work is an invocation is tested through its `mock`, so nothing is called
        // for this walk and there is nowhere for it to wait.
        tasks: &super::integration::Unserved,
        waiting: None,
        parked: None,
        // `TestState` starts nothing: it reports what one state computes, and the
        // Map Runs a Distributed Map starts — and the child executions its items run
        // as — belong to an execution.
        map_runs: Vec::new(),
        map_runs_entered: 0,
        // `TestState` continues no execution: a state whose work is an invocation is
        // answered by the request's own `mock`, not by a worker's reply.
        reply: None,
        job_reply: None,
        resumed: None,
        machine_name: test.context.state_machine_name.clone(),
        region: super::arn_region(&test.context.state_machine_arn),
        child_counter: 0,
        // The state under test belongs to no state machine, so no workflow type
        // refuses it: the context the request supplies says nothing of one.
        express: test.context.express,
    };
    interpreter.trial(test, &raw, inspection)
}

impl Interpreter<'_> {
    /// [`test_state`]'s walk of the one state under test.
    fn trial(&mut self, test: &StateTest<'_>, raw: &Value, mut inspection: Inspection) -> Trial {
        let state = test.state;
        let state_type = state.get("Type").and_then(Value::as_str).unwrap_or_default();
        let context = self.context_for(&Scope::default(), test.name, Duration::ZERO);

        if let Some(language) = state.get("QueryLanguage").and_then(Value::as_str) {
            if language != JSONPATH {
                return Trial::failed(
                    inspection,
                    runtime(format!(
                        "{SINGLE_STATE_LABEL} is written in '{language}', and this emulator \
                         evaluates only '{JSONPATH}' states, so nothing was run for it"
                    )),
                );
            }
        }
        // A member this emulator does not interpret changes what the state does, so
        // the state is not run for it — the rule the walk applies at every state. A
        // MOCKED state is exempt: the members that describe the invocation the mock
        // stands in for are the mock's business, and nothing is invoked for them; a
        // type this emulator does not run at all is reported by `produce` as the
        // invocation it is, not as a member.
        if test.mock.is_none() && EXECUTED_STATE_TYPES.contains(&state_type) {
            if let Some(member) = unexecutable_member(state_type, state) {
                return Trial::failed(
                    inspection,
                    runtime(format!(
                        "{SINGLE_STATE_LABEL} carries the member '{member}', which this emulator \
                         does not interpret, so nothing was run for it"
                    )),
                );
            }
        }
        // The input side: `InputPath`, then the `Parameters` payload template.
        let effective = match self.input_value(state, raw, &context) {
            Ok(value) => value,
            Err(failure) => return self.classify(test, raw, &context, failure, inspection),
        };
        inspection.after_input_path = Some(effective.to_string());
        let templated = TEMPLATED_STATE_TYPES.contains(&state_type);
        let after_parameters = match state.get("Parameters").filter(|_| templated) {
            Some(template) => match payload_template(template, &effective, &context) {
                Ok(value) => value,
                Err(failure) => return self.classify(test, raw, &context, failure, inspection),
            },
            None => effective.clone(),
        };
        if templated {
            inspection.after_parameters = Some(after_parameters.to_string());
        }

        // A `Choice` state's transition is what the state DOES, and the walk decides
        // it before the state's output is built, so a rule that cannot be evaluated
        // is what such a state fails with.
        let chosen = if state_type == "Choice" {
            match self.choose(state, &effective, &context) {
                Ok(next) => Some(next),
                Err(failure) => return self.classify(test, raw, &context, failure, inspection),
            }
        } else {
            None
        };

        let produced = self.produce(state, state_type, &after_parameters, &effective, &context, test.mock);
        let placed = match produced {
            Ok(Some(result)) => {
                self.place_recorded(state, state_type, raw, result, &context, &mut inspection)
            }
            Ok(None) => self.output_value(state, effective.clone(), &context),
            Err(failure) => Err(failure),
        };
        let output = match placed {
            Ok(output) => output,
            Err(failure) => return self.classify(test, raw, &context, failure, inspection),
        };
        // The document the state produced is measured here, at the state that
        // produced it, against what a state's output may carry — as the walk measures
        // every state's.
        if let Err(failure) = measure_payload(&output, SINGLE_STATE_LABEL) {
            return self.classify(test, raw, &context, failure, inspection);
        }

        let next_state = match chosen {
            Some(next) => Some(next),
            None if TERMINAL_STATE_TYPES.contains(&state_type) => None,
            None if state.get("End").and_then(Value::as_bool) == Some(true) => None,
            // A single state's `Next` names a state no definition around it declares,
            // so it is REPORTED rather than resolved; a state that names none has no
            // next state at all.
            None => state.get("Next").and_then(Value::as_str).map(str::to_string),
        };
        Trial::succeeded(
            Some(output_text(&output, raw, test.input)),
            next_state,
            inspection,
        )
    }

    /// The RAW result the state's own work produced: the document the result-side
    /// filters place, `None` for a state that produces no result of its own (its
    /// effective input is what `OutputPath` reads), or the failure it ended in.
    fn produce(
        &mut self,
        state: &Map<String, Value>,
        state_type: &str,
        after_parameters: &Value,
        effective: &Value,
        context: &Value,
        mock: Option<&Mock>,
    ) -> Result<Option<Value>, Failure> {
        // The mock IS the outcome of the invocation it stands in for, so nothing is
        // invoked for it and the state's result is the one the request named.
        match mock {
            Some(Mock::Result(result)) => return Ok(Some(result.clone())),
            Some(Mock::Error(failure)) => return Err(failure.clone()),
            None => {}
        }
        match state_type {
            // "Parameters ... Used to create a collection of key-value pairs that
            // will be passed as input": it transforms the state's effective INPUT,
            // while `Result` is the state's RESULT — the rule the walk applies.
            "Pass" => Ok(Some(
                state
                    .get("Result")
                    .cloned()
                    .unwrap_or_else(|| after_parameters.clone()),
            )),
            // A `Wait` state produces no result of its own. Its members are still
            // evaluated, so a malformed one is reported as the failure it is, but
            // nothing sleeps for the span it declares.
            "Wait" => {
                self.wait_duration(state, effective, context, Duration::ZERO)
                    .map_err(|why| runtime(format!("{SINGLE_STATE_LABEL}: {why}")))?;
                Ok(None)
            }
            "Choice" | "Succeed" => Ok(None),
            // A `Fail` state ends with the error and cause it declares, either of
            // which ASL may instead read from the state's input through a `*Path`.
            "Fail" => {
                let error = self.fail_member(state, "Error", "ErrorPath", effective, context)?;
                let cause = self.fail_member(state, "Cause", "CausePath", effective, context)?;
                Err(Failure { error, cause })
            }
            // Every remaining type's work is an invocation this process does not
            // make: a service integration, an iteration over items, a set of
            // branches. Without a mock to stand in for it, nothing ran — which is
            // what the state fails with, rather than a synthesised result.
            other => Err(runtime(format!(
                "a '{other}' state's own work is an invocation this emulator does not make, and \
                 the request names no mock to stand in for it, so nothing was run for it"
            ))),
        }
    }

    /// The result-side filters over a state's raw result — `ResultSelector`,
    /// `ResultPath` against the state's own raw input, then `OutputPath` — recording
    /// what the state carried after each one.
    fn place_recorded(
        &self,
        state: &Map<String, Value>,
        state_type: &str,
        raw: &Value,
        result: Value,
        context: &Value,
        inspection: &mut Inspection,
    ) -> Result<Value, Failure> {
        inspection.result = Some(result.to_string());
        let with_selector = SELECTOR_STATE_TYPES.contains(&state_type);
        let selected = select_result(state, result, context, with_selector)?;
        if with_selector {
            inspection.after_result_selector = Some(selected.to_string());
        }
        let placed = path_result(state, raw, selected)?;
        inspection.after_result_path = Some(placed.to_string());
        self.output_value(state, placed, context)
    }

    /// What `TestState` reports for a state whose work failed: the attempt one of its
    /// `Retry` retriers still owes (RETRIABLE), the catcher that takes the failure
    /// over (CAUGHT_ERROR), or the failure itself (FAILED).
    ///
    /// The two are the state's OWN error handling, read exactly as the walk reads it:
    /// the first retrier whose `ErrorEquals` names the error decides, and `Catch` is
    /// consulted only once no retrier has an attempt left.
    fn classify(
        &self,
        test: &StateTest<'_>,
        raw: &Value,
        context: &Value,
        failure: Failure,
        inspection: Inspection,
    ) -> Trial {
        let state = test.state;
        let name = failure.name().to_string();
        for (index, retrier) in retry_entries(state, "Retry").iter().enumerate() {
            let Some(retrier) = retrier.as_object() else {
                continue;
            };
            if !errors_match(retrier, &name) {
                continue;
            }
            let max_attempts = retrier
                .get("MaxAttempts")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_RETRY_MAX_ATTEMPTS);
            if test.retries_taken >= max_attempts {
                break;
            }
            let mut trial = Trial::failed(inspection, failure);
            trial.status = TEST_STATUS_RETRIABLE;
            trial.retry_index = Some(i32::try_from(index).unwrap_or(i32::MAX));
            trial.retry_backoff_seconds = Some(
                retry_delay_seconds(retrier, test.retries_taken).round() as i32,
            );
            return trial;
        }
        for (index, catcher) in retry_entries(state, "Catch").iter().enumerate() {
            let Some(catcher) = catcher.as_object() else {
                continue;
            };
            if !errors_match(catcher, &name) {
                continue;
            }
            let Some(next) = catcher.get("Next").and_then(Value::as_str) else {
                return Trial::failed(
                    inspection,
                    runtime("a 'Catch' catcher must carry a string 'Next'".to_string()),
                );
            };
            // "When a state reports an error and it matches a catcher, the state's
            // output is the error output", placed through the catcher's own
            // `ResultPath`.
            let output = match self.place_result(catcher, raw, failure.payload(), context, true) {
                Ok(output) => output,
                Err(failure) => return Trial::failed(inspection, failure),
            };
            let mut trial = Trial::failed(inspection, failure);
            trial.status = TEST_STATUS_CAUGHT_ERROR;
            trial.output = Some(output.to_string());
            trial.next_state = Some(next.to_string());
            trial.catch_index = Some(i32::try_from(index).unwrap_or(i32::MAX));
            return trial;
        }
        Trial::failed(inspection, failure)
    }
}

/// A state's `Retry` or `Catch` entries, or none when it declares no such array.
fn retry_entries<'a>(state: &'a Map<String, Value>, member: &str) -> &'a [Value] {
    state
        .get(member)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// A state's output as the response carries it: the caller's OWN bytes when the
/// document the state put out is the one the caller wrote, and the encoding the walk
/// produced otherwise — the rule an execution's output follows.
fn output_text(output: &Value, raw: &Value, input: &str) -> String {
    if output == raw {
        input.to_string()
    } else {
        output.to_string()
    }
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

// SPDX-License-Identifier: CC0-1.0

//! @op InvokeCodeInterpreter
//!
//! `POST /code-interpreters/{codeInterpreterIdentifier}/tools/invoke -> 200`. Invokes
//! one tool on a live code interpreter session and answers with the tool's result as a
//! `vnd.amazon.eventstream` body: a single `result` event carrying the
//! `CodeInterpreterResult` the tool produced.
//!
//! This operation is hand-authored rather than generated: its output member `stream`
//! targets a `@streaming` union, which the op-stub generator declines
//! (`src/ops/SKIPPED_STUBS.md`) and whose generated `ErasedOperation` shim refuses the
//! request. The framing and the routing seam live in `service_common::event_stream`
//! and `src/service.rs`; this file holds the behaviour, and the shapes are in
//! `src/ops/types/invoke_code_interpreter_types.rs`.
//!
//! # What each tool does here
//!
//! The session named by `x-amzn-code-interpreter-session-id` owns a WORKSPACE and an
//! execution CONTEXT in [`crate::state`], and every tool below computes its answer
//! from them:
//!
//! * `writeFiles`, `readFiles`, `listFiles`, `removeFiles` — the session's workspace.
//!   A path is a workspace-relative key ([`crate::state::normalize_session_path`]), so
//!   no tool argument names anything outside the session's own state, and a write is
//!   bounded by the workspace ceilings the store declares.
//! * `executeCode` with `language: javascript` — the submitted script RUNS, in the
//!   QuickJS engine embedded in this process (`rquickjs`), and what it wrote to
//!   `console` is the `stdout` / `stderr` the result reports. A run is evaluated on top
//!   of the scripts this session has already run to completion, which is what makes
//!   `clearContext` mean something: with it set, the context is dropped first.
//! * `executeCode` with any other `language`, and the command tools (`executeCommand`,
//!   `startCommandExecution`, `getTask`, `stopTask`) — this emulator embeds no engine
//!   for them (there is no in-process Python runtime, no TypeScript transpiler and no
//!   command shell), so the result says the work was NOT run, with `isError` set.
//!   Nothing is synthesised to look as though it ran: no output, no exit status and no
//!   duration is reported for work no engine performed.
//!
//! Which members the `result` carries follows from the tool, not from the call:
//! `structuredContent` reports a RUN (its members are `stdout`, `stderr`, `exitCode`,
//! `executionTime`, `taskId`, `taskStatus`), so a workspace tool has nothing to put in
//! it; and `isError` says that "the result represents an error", so a tool that did
//! what was asked reports nothing there. [`ToolOutcome`] is where that lives.
//!
//! # How a run is bounded and contained
//!
//! The engine is an execution substrate, so every limit lives in this host code, which
//! a script cannot reach:
//!
//! * wall clock — the engine's interrupt handler stops the script once
//!   [`RUN_WALL_CLOCK`] has passed, which is what bounds a CPU-bound loop a timeout
//!   alone could not preempt;
//! * memory and stack — [`RUN_MEMORY_BYTES`] and [`RUN_STACK_BYTES`] are set on the
//!   engine before anything is evaluated, so an allocation or a recursion past them
//!   ends that run and nothing else;
//! * output — each captured stream stops at [`MAX_CAPTURED_STREAM_BYTES`], so a small
//!   request cannot make a large response, and a `readFiles` result stops at
//!   [`MAX_READ_RESULT_BYTES`] over at most [`MAX_TOOL_PATHS`] files, so neither can a
//!   tool call that names many large files;
//! * script size — a submitted script past [`MAX_SCRIPT_BYTES`] is not compiled;
//! * concurrency — at most [`MAX_CONCURRENT_RUNS`] scripts run at once, so the memory
//!   and CPU the engine can hold is bounded however many calls arrive, and a call that
//!   waits [`PERMIT_WAIT`] for its turn is answered with the modelled throttle;
//! * the run happens on a blocking thread (`tokio::task::spawn_blocking`), never on an
//!   async worker, and with NO state guard of this service held, so one caller's script
//!   cannot stall another caller's request.
//!
//! Containment: the engine is built with default features only — no module loader and
//! no native module loading — so a script can import nothing, and plain QuickJS
//! exposes no filesystem, process, environment or socket API at all. The only host
//! functions a script can call are the two that collect its console output. The
//! session workspace is reachable through the file tools, never from inside a script,
//! and it lives in this process's memory: no host path is opened for it.
//!
//! ## Input
//!
//! * `codeInterpreterIdentifier` (String, `@httpLabel`) **required**
//! * `sessionId` (CodeInterpreterSessionId, pattern `^[0-9a-zA-Z]{1,40}$`,
//!   `@httpHeader("x-amzn-code-interpreter-session-id")`) — the session to run in; a
//!   call that names none is served in a session this call starts, which the response
//!   names back in the same header.
//! * `name` (ToolName) **required** — `executeCode` | `executeCommand` | `readFiles` |
//!   `listFiles` | `removeFiles` | `writeFiles` | `startCommandExecution` | `getTask` |
//!   `stopTask`.
//! * `arguments` (ToolArguments) — the invoked tool's parameters.
//! * `traceId`, `traceParent` (`@httpHeader`) — distributed tracing; part of no answer.
//!
//! ## Output
//!
//! * `stream` (CodeInterpreterStreamOutput, `@httpPayload`, `@streaming`) — the body:
//!   one `result` event holding `content` (required), `structuredContent` and
//!   `isError`.
//! * `sessionId` (`@httpHeader("x-amzn-code-interpreter-session-id")`) — the session
//!   the tool ran in.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — the session named is not one this
//!   emulator holds for that code interpreter.
//! * `ConflictException` (HTTP 409) — the session is `TERMINATED` (stopped, or past its
//!   time-to-live), which the model documents as a session a tool call cannot use.
//! * `ValidationException` (HTTP 400) — the invoked tool was given no argument it
//!   cannot work without, or one that addresses nothing in the workspace.
//! * `AccessDeniedException` (403), `InternalServerException` (500),
//!   `ServiceQuotaExceededException` (402), `ThrottlingException` (429).

#[allow(unused_imports)]
use super::types::*;
use aws_smithy_runtime_api::http::{Request, Response};
use rquickjs::function::Func;
use rquickjs::{CatchResultExt, Context, Ctx, Runtime, Value};
use service_common::event_stream::{
    deserialize_request, error_response, event_stream_response, frame_event,
};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the
/// model declares for it.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentcore#InvokeCodeInterpreter")]
#[derive(Debug)]
pub enum InvokeCodeInterpreterErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// How long one script may run before the engine's interrupt handler stops it.
///
/// An EMULATOR bound, not an AWS one: the script runs in this process, so a loop with
/// no exit would otherwise hold a thread for ever. Long enough for any computation a
/// test or an agent performs in one tool call.
const RUN_WALL_CLOCK: Duration = Duration::from_secs(2);

/// The heap one run may use, set on the engine before anything is evaluated.
const RUN_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// The JavaScript stack one run may use. Well inside the stack of the blocking thread
/// the run happens on, so deep recursion in a script ends that run with the engine's
/// own stack-overflow exception rather than touching the thread's own guard page.
const RUN_STACK_BYTES: usize = 512 * 1024;

/// The most bytes of one output stream a run reports back. Past it the stream keeps
/// what it already captured and the rest is dropped, so a one-line script cannot make
/// an unbounded response.
const MAX_CAPTURED_STREAM_BYTES: usize = 64 * 1024;

/// The longest script this engine compiles. `MaxLenString` admits 100 MB of `code`,
/// which no in-process engine should be handed; this is the EMULATOR's own bound.
const MAX_SCRIPT_BYTES: usize = 1024 * 1024;

/// The most files one tool call may address. `StringList` and `InputContentBlockList`
/// carry no `@length`, so this is the EMULATOR's own bound on the COUNT side of a tool
/// call: it is what keeps the work and the response of one call proportional to the
/// call rather than to the product of a caller-chosen count and a caller-chosen file
/// size.
const MAX_TOOL_PATHS: usize = 64;

/// The most bytes one `readFiles` result carries across every block in it. The cap on
/// the PRODUCT the count bound above cannot give on its own, since one workspace file
/// may hold [`crate::state::MAX_SESSION_FILE_BYTES`]: a read whose result would pass
/// this is refused rather than materialised.
const MAX_READ_RESULT_BYTES: usize = 4 * 1024 * 1024;

/// How many scripts this service runs at once. Together with [`RUN_MEMORY_BYTES`] it
/// bounds what the engine can hold however many calls arrive at once, and it keeps the
/// blocking pool from filling with CPU-bound runs.
const MAX_CONCURRENT_RUNS: usize = 4;

/// How long a call waits for one of those run slots before it is answered with the
/// modelled `ThrottlingException` instead of queueing behind other callers' scripts.
const PERMIT_WAIT: Duration = Duration::from_secs(5);

/// The `ProgrammingLanguage` value this emulator has an engine for (source:
/// smithy_query query="shape" shape="ProgrammingLanguage", values python | javascript |
/// typescript).
const LANGUAGE_JAVASCRIPT: &str = "javascript";

/// The `LanguageRuntime` values that run JavaScript, which is what the embedded engine
/// serves (source: smithy_query query="shape" shape="LanguageRuntime", values nodejs |
/// deno | python). A request naming any other runtime — including one this list does
/// not know — is answered as work nothing ran, rather than run as JavaScript on a
/// guess.
const JAVASCRIPT_RUNTIMES: [&str; 2] = ["nodejs", "deno"];

/// `ContentBlockType`'s values, as a tool result's blocks use them (source:
/// smithy_query query="shape" shape="ContentBlockType", values text | image | resource
/// | resource_link).
const BLOCK_TEXT: &str = "text";
/// See [`BLOCK_TEXT`].
const BLOCK_RESOURCE: &str = "resource";
/// See [`BLOCK_TEXT`].
const BLOCK_RESOURCE_LINK: &str = "resource_link";

/// `ResourceContentType`'s values (source: smithy_query query="shape"
/// shape="ResourceContentType", values text | blob).
const RESOURCE_TEXT: &str = "text";
/// See [`RESOURCE_TEXT`].
const RESOURCE_BLOB: &str = "blob";

/// The media type reported for a workspace entry that is a directory rather than a
/// file, which is how a `listFiles` block tells the two apart.
const DIRECTORY_MEDIA_TYPE: &str = "inode/directory";

/// This operation's own modelled 400 `ValidationException`, for a tool argument the
/// invoked tool cannot work without or cannot work with.
///
/// `reason` is `ValidationExceptionReason`'s `FieldValidationFailed` (source:
/// smithy_query query="shape" shape="ValidationExceptionReason", values CannotParse |
/// FieldValidationFailed | IdempotentParameterMismatchException | EventInOtherSession |
/// ResourceConflict), which is the member-level failure every case here is.
fn validation(message: impl Into<String>) -> InvokeCodeInterpreterErrorModeled {
    InvokeCodeInterpreterErrorModeled::ValidationException(ValidationException {
        message: message.into(),
        reason: "FieldValidationFailed".to_string(),
        field_list: None,
    })
}

fn internal(message: impl Into<String>) -> InvokeCodeInterpreterErrorModeled {
    InvokeCodeInterpreterErrorModeled::InternalServerException(InternalServerException {
        message: Some(message.into()),
    })
}

fn throttling(message: impl Into<String>) -> InvokeCodeInterpreterErrorModeled {
    InvokeCodeInterpreterErrorModeled::ThrottlingException(ThrottlingException {
        message: Some(message.into()),
    })
}

/// The `InvokeCodeInterpreter` operation.
///
/// Owns the shared engine (the constructor seam). The generated routing table holds
/// one of these and `service.rs` holds another wrapped in an `EventStreamRoute`; both
/// reach the same state, because the state lives in the shared engine, not on the
/// operation.
pub struct InvokeCodeInterpreter<Eng> {
    engine: Arc<Eng>,
    /// The run slots [`MAX_CONCURRENT_RUNS`] declares, held by the service rather than
    /// by one request, so the bound is over every caller of this endpoint.
    runs: Arc<::tokio::sync::Semaphore>,
}

impl<Eng> InvokeCodeInterpreter<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            engine,
            runs: Arc::new(::tokio::sync::Semaphore::new(MAX_CONCURRENT_RUNS)),
        }
    }
}

impl<Eng: Send + Sync + 'static> InvokeCodeInterpreter<Eng> {
    /// The whole request: resolve the session, invoke the named tool on it, and frame
    /// the tool's result as the operation's one `result` event.
    async fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, InvokeCodeInterpreterErrorModeled> {
        let input: InvokeCodeInterpreterInput =
            deserialize_request(protocol, req, &InvokeCodeInterpreterInput::SCHEMA)
                .map_err(validation)?;

        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(|| internal("the shared engine does not carry this service's state"))?;

        // A code interpreter is created through the CONTROL plane (apart from the one the
        // service provides to every account), so this call resolves the interpreter before
        // it reaches any session of it: a tool cannot run in a sandbox that does not
        // exist, and a call that names no session would otherwise START one on it. The
        // inventory is read and released before this service's own store is taken.
        let held = crate::state::holds_code_interpreter(
            &*self.engine,
            &input.code_interpreter_identifier,
        )
        .ok_or_else(|| internal("the shared engine does not carry the AgentCore resource inventory"))?;
        if !held {
            return Err(
                InvokeCodeInterpreterErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::resource_not_found_message(
                            crate::state::NotFoundResource::CodeInterpreter,
                            &input.code_interpreter_identifier,
                        )),
                    },
                ),
            );
        }

        let now = SystemTime::now();
        // The session this tool runs in, resolved (or started) under the guard and
        // then addressed by id, so nothing below holds the guard while a script runs.
        let session_id = {
            let mut store = state.lock();
            match input.session_id.as_deref() {
                Some(named) => {
                    let session = store
                        .code_interpreter_session(&input.code_interpreter_identifier, named)
                        .ok_or_else(|| {
                            InvokeCodeInterpreterErrorModeled::ResourceNotFoundException(
                                ResourceNotFoundException {
                                    message: Some(format!(
                                        "session {named} not found for code interpreter {}",
                                        input.code_interpreter_identifier
                                    )),
                                },
                            )
                        })?;
                    // The model documents that a session which "has expired or been
                    // stopped" cannot serve a tool call: that is a request conflicting
                    // with the current state of the resource.
                    let status = session.status_as_of(now);
                    if status == crate::state::CODE_INTERPRETER_SESSION_TERMINATED {
                        return Err(InvokeCodeInterpreterErrorModeled::ConflictException(
                            ConflictException {
                                message: Some(crate::state::session_already_terminated(named)),
                            },
                        ));
                    }
                    named.to_string()
                }
                // `sessionId` is optional on the request and bound on the RESPONSE, so
                // a call that names no session is served in one this call starts, with
                // the modelled default time-to-live, and the response names it back.
                None => {
                    let seq = store.next_seq();
                    let started = crate::state::CodeInterpreterSessionRecord::started(
                        seq,
                        input.code_interpreter_identifier.clone(),
                        None,
                        crate::state::DEFAULT_CODE_INTERPRETER_SESSION_TIMEOUT_SECONDS,
                        None,
                        None,
                        now,
                    );
                    let session_id = started.session_id.clone();
                    store.insert_code_interpreter_session(started, None);
                    session_id
                }
            }
        };

        let arguments = input.arguments.clone().unwrap_or_default();
        // `ToolName` is a closed enum the request walk already checked the value
        // against, so the final arm is unreachable for a served request — it is here
        // because a match over a string has to be total.
        let outcome = match input.name.as_str() {
            "executeCode" => {
                self.execute_code(
                    &state,
                    &input.code_interpreter_identifier,
                    &session_id,
                    &arguments,
                )
                .await
            }
            "writeFiles" => self.write_files_in_session(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                &arguments,
                SystemTime::now(),
            )?,
            "readFiles" => self.with_session(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                |session| read_files(session, &arguments),
            )?,
            "listFiles" => self.with_session(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                |session| list_files(session, &arguments),
            )?,
            "removeFiles" => self.with_session_mut(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                |session| remove_files(session, &arguments),
            )?,
            "executeCommand" | "startCommandExecution" | "getTask" | "stopTask" => {
                Ok(no_command_runtime(&input.name))
            }
            other => return Err(validation(format!("{other} is not a code interpreter tool."))),
        };
        let result = outcome?.into_result();

        let frames = frame_event(
            protocol,
            // The `CodeInterpreterStreamOutput` member this event is, which the frame
            // carries as its `:event-type` (source: smithy_query query="shape"
            // shape="CodeInterpreterStreamOutput").
            "result",
            false,
            &result,
            &CodeInterpreterResult::SCHEMA,
        )
        .map_err(|error| internal(format!("the result event could not be framed: {error}")))?;

        event_stream_response(
            protocol,
            "InvokeCodeInterpreter",
            &InvokeCodeInterpreterHeaders {
                session_id: Some(session_id),
            },
            &InvokeCodeInterpreterHeaders::SCHEMA,
            frames,
        )
        .map_err(|error| internal(format!("the response could not be serialized: {error}")))
    }

    /// Read the live session under the guard, hand it to `tool`, and release the guard
    /// with the tool's answer. The session is re-resolved here because it is addressed
    /// by id: a stop that landed in between is a 404 for this call, not a panic.
    fn with_session<T>(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        tool: impl FnOnce(&crate::state::CodeInterpreterSessionRecord) -> T,
    ) -> Result<T, InvokeCodeInterpreterErrorModeled> {
        let store = state.lock();
        let session = store
            .code_interpreter_session(code_interpreter_identifier, session_id)
            .ok_or_else(|| session_gone(code_interpreter_identifier, session_id))?;
        Ok(tool(session))
    }

    /// [`Self::with_session`] for a tool that CHANGES the session — one of the file
    /// tools that writes the workspace.
    fn with_session_mut<T>(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        tool: impl FnOnce(&mut crate::state::CodeInterpreterSessionRecord) -> T,
    ) -> Result<T, InvokeCodeInterpreterErrorModeled> {
        let mut store = state.lock();
        let session = store
            .code_interpreter_session_mut(code_interpreter_identifier, session_id)
            .ok_or_else(|| session_gone(code_interpreter_identifier, session_id))?;
        Ok(tool(session))
    }

    /// [`Self::with_session_mut`] for the ONE tool that GROWS the store: the bytes every
    /// other session holds are read under the same guard and handed to the write, so a
    /// workspace write holds the whole-store ceiling as well as this session's own.
    fn write_files_in_session(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        arguments: &ToolArguments,
        now: SystemTime,
    ) -> Result<Result<ToolOutcome, InvokeCodeInterpreterErrorModeled>, InvokeCodeInterpreterErrorModeled>
    {
        let mut store = state.lock();
        let elsewhere = store.session_bytes_elsewhere(code_interpreter_identifier, session_id);
        let session = store
            .code_interpreter_session_mut(code_interpreter_identifier, session_id)
            .ok_or_else(|| session_gone(code_interpreter_identifier, session_id))?;
        Ok(write_files(session, arguments, now, elsewhere))
    }

    /// `executeCode`: run the submitted script in the engine this emulator embeds for
    /// the language it names, on top of the session's execution context.
    async fn execute_code(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        arguments: &ToolArguments,
    ) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
        let Some(script) = arguments.code.clone() else {
            return Err(validation(
                "The executeCode tool requires the code argument.",
            ));
        };
        let Some(language) = arguments.language.clone() else {
            return Err(validation(
                "The executeCode tool requires the language argument.",
            ));
        };
        // `language` and `runtime` both name what the code is to run under, and only
        // JavaScript has an engine here.
        let runs_javascript = language == LANGUAGE_JAVASCRIPT
            && arguments
                .runtime
                .as_deref()
                .is_none_or(|runtime| JAVASCRIPT_RUNTIMES.contains(&runtime));
        if !runs_javascript {
            return Ok(ToolOutcome::NothingRan(format!(
                "This code interpreter session has no {language} engine, so the code was not \
                 run. The engine it embeds runs {LANGUAGE_JAVASCRIPT}."
            )));
        }
        if script.len() > MAX_SCRIPT_BYTES {
            return Ok(ToolOutcome::NothingRan(format!(
                "The submitted script is {} bytes, past the {MAX_SCRIPT_BYTES} this session \
                 compiles, so it was not run.",
                script.len()
            )));
        }

        // The context this run is evaluated on top of, copied out under the guard, and
        // dropped first where the call asked for a clear context. The guard is released
        // before anything is evaluated.
        let context_scripts = {
            let mut store = state.lock();
            let session = store
                .code_interpreter_session_mut(code_interpreter_identifier, session_id)
                .ok_or_else(|| session_gone(code_interpreter_identifier, session_id))?;
            if arguments.clear_context == Some(true) {
                session.context_scripts.clear();
            }
            session.context_scripts.clone()
        };

        let submitted = script.clone();
        // One of the service's run slots, waited for only as long as [`PERMIT_WAIT`]:
        // past that the caller is throttled rather than left queueing behind other
        // callers' scripts. Held until this run ends, however it ends.
        let permit = ::tokio::time::timeout(PERMIT_WAIT, Arc::clone(&self.runs).acquire_owned())
            .await
            .map_err(|_| {
                throttling(
                    "Too many code executions are running in this code interpreter. Retry the \
                     request.",
                )
            })?
            .map_err(|error| internal(format!("the run slots are gone: {error}")))?;
        // Off the async worker: the engine is synchronous and CPU-bound, and the
        // runtime has to stay responsive while a script runs.
        let run = ::tokio::task::spawn_blocking(move || run_script(&context_scripts, &submitted))
            .await
            .map_err(|error| internal(format!("the script run could not be joined: {error}")))?;
        drop(permit);

        if run.exit_code == 0 {
            // A script that ran to completion becomes part of what the next run of this
            // session sees. One that ended on an exception defined nothing a later run
            // can rely on, so it is not kept.
            let mut store = state.lock();
            let elsewhere =
                store.session_bytes_elsewhere(code_interpreter_identifier, session_id);
            if let Some(session) =
                store.code_interpreter_session_mut(code_interpreter_identifier, session_id)
            {
                session.keep_context_script(script, elsewhere);
            }
        }

        let text = if run.stdout.is_empty() && !run.stderr.is_empty() {
            run.stderr.clone()
        } else {
            run.stdout.clone()
        };
        Ok(ToolOutcome::Ran {
            content: vec![text_block(text)],
            stdout: run.stdout,
            stderr: run.stderr,
            exit_code: run.exit_code,
            elapsed: run.elapsed,
        })
    }
}

/// What one tool invocation did, before it is worded as a `CodeInterpreterResult`.
///
/// Which members the result carries follows from the KIND of tool, because both of the
/// optional ones are only meaningful for some of them:
///
/// * `structuredContent` reports a RUN — `stdout`, `stderr`, `exitCode`,
///   `executionTime`, `taskId`, `taskStatus` are its only members — so it belongs to a
///   tool that runs code or a command, and a tool that reads or writes the workspace
///   has nothing to put in it;
/// * `isError` is documented as indicating that "the result represents an error", so a
///   tool that did what was asked reports nothing there.
enum ToolOutcome {
    /// A workspace tool that did what was asked: the blocks a client reads, and
    /// nothing else.
    Content(Vec<ContentBlock>),
    /// A tool that ran and could not do what was asked — a path the workspace does not
    /// hold, or a workspace with no room left.
    Failed(String),
    /// A script this emulator RAN: the blocks a client reads, plus the report of the
    /// run itself (`0` for a script that ran to completion, `1` for one that did not)
    /// and how long THAT run took — not how long the call around it took.
    Ran {
        content: Vec<ContentBlock>,
        stdout: String,
        stderr: String,
        exit_code: i32,
        elapsed: Duration,
    },
    /// Work this emulator embeds no engine for, so nothing performed it.
    NothingRan(String),
}

impl ToolOutcome {
    /// The `result` event's own shape: the blocks, the structured report where the tool
    /// made one, and `isError` where the result is an error.
    fn into_result(self) -> CodeInterpreterResult {
        match self {
            Self::Content(content) => CodeInterpreterResult {
                content,
                structured_content: None,
                is_error: None,
            },
            Self::Failed(reason) => CodeInterpreterResult {
                content: vec![text_block(reason)],
                structured_content: None,
                is_error: Some(true),
            },
            Self::Ran {
                content,
                stdout,
                stderr,
                exit_code,
                elapsed,
            } => CodeInterpreterResult {
                content,
                structured_content: Some(ToolResultStructuredContent {
                    task_id: None,
                    task_status: None,
                    stdout: Some(stdout),
                    stderr: Some(stderr),
                    exit_code: Some(exit_code),
                    // Seconds, as a `Double`: the engine's own time on this run.
                    execution_time: Some(elapsed.as_secs_f64()),
                }),
                is_error: Some(exit_code != 0),
            },
            // No output, status or duration: nothing ran to produce any.
            Self::NothingRan(reason) => CodeInterpreterResult {
                content: vec![text_block(reason.clone())],
                structured_content: Some(ToolResultStructuredContent {
                    stderr: Some(reason),
                    ..Default::default()
                }),
                is_error: Some(true),
            },
        }
    }
}

/// The 404 for a session that is no longer held — a stop that landed between this
/// call's own steps.
fn session_gone(
    code_interpreter_identifier: &str,
    session_id: &str,
) -> InvokeCodeInterpreterErrorModeled {
    InvokeCodeInterpreterErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(format!(
            "session {session_id} not found for code interpreter {code_interpreter_identifier}"
        )),
    })
}

/// One `text` content block.
fn text_block(text: impl Into<String>) -> ContentBlock {
    ContentBlock {
        r#type: BLOCK_TEXT.to_string(),
        text: Some(text.into()),
        ..Default::default()
    }
}

/// The result for work this emulator embeds no engine for.
///
/// The answer for every tool of the command plane. A command runs in a shell inside the
/// session's sandbox, and this emulator embeds none — so no command is run, no task is
/// ever started, and therefore no task id addresses one either.
fn no_command_runtime(tool: &str) -> ToolOutcome {
    ToolOutcome::NothingRan(format!(
        "This code interpreter session has no command runtime, so the {tool} tool ran nothing."
    ))
}

/// `writeFiles`: put every block's bytes in the session's workspace.
///
/// All or nothing: the paths are normalized and the bytes taken first, a block that
/// names a path an earlier block in the same call named is that path's last write, and
/// the WHOLE batch is then checked against the workspace ceilings before anything is
/// stored — so a call the workspace has no room for leaves it exactly as it was.
fn write_files(
    session: &mut crate::state::CodeInterpreterSessionRecord,
    arguments: &ToolArguments,
    now: SystemTime,
    bytes_elsewhere: usize,
) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
    let blocks = arguments.content.as_deref().unwrap_or_default();
    if blocks.is_empty() {
        return Err(validation(
            "The writeFiles tool requires the content argument to carry at least one file.",
        ));
    }
    if blocks.len() > MAX_TOOL_PATHS {
        return Err(too_many_paths("writeFiles", blocks.len()));
    }
    let mut pending: Vec<(String, Vec<u8>)> = Vec::with_capacity(blocks.len());
    for block in blocks {
        let key = workspace_key(&block.path)?;
        let bytes = match (block.text.as_ref(), block.blob.as_ref()) {
            (Some(text), None) => text.as_bytes().to_vec(),
            (None, Some(blob)) => blob.clone(),
            (None, None) => Vec::new(),
            (Some(_), Some(_)) => {
                return Err(validation(format!(
                    "The writeFiles content block for {key} carries both text and blob; it must \
                     carry one."
                )))
            }
        };
        // One entry per path, so the room this batch needs is the room its last write
        // of each path needs.
        match pending.iter_mut().find(|(held, _)| *held == key) {
            Some(held) => held.1 = bytes,
            None => pending.push((key, bytes)),
        }
    }

    // The ceilings are checked over the whole batch, where the store owns the
    // arithmetic, so no part of a batch that does not fit is stored.
    let sizes: Vec<(String, usize)> = pending
        .iter()
        .map(|(key, bytes)| (key.clone(), bytes.len()))
        .collect();
    if let Err(full) = session.room_for(&sizes, bytes_elsewhere) {
        return Ok(ToolOutcome::Failed(match full {
            crate::state::WorkspaceFull::Files(limit) => format!(
                "The session workspace holds at most {limit} files, so none of the {} files this \
                 call names was written.",
                pending.len()
            ),
            crate::state::WorkspaceFull::Bytes(limit) => format!(
                "The session workspace holds at most {limit} bytes, so none of the {} files this \
                 call names was written.",
                pending.len()
            ),
            crate::state::WorkspaceFull::StoreBytes(limit) => format!(
                "The code interpreter sessions of this emulator together hold at most {limit} \
                 bytes, so none of the {} files this call names was written. Stop a session you \
                 no longer need, or remove files from it.",
                pending.len()
            ),
        }));
    }

    let mut written = String::new();
    for (key, bytes) in pending {
        let size = bytes.len();
        // The batch already has room, so this cannot report the workspace full; the
        // store checks each write anyway, and its answer is reported rather than
        // dropped.
        if let Err(full) = session.write_file(&key, bytes, now, bytes_elsewhere) {
            return Ok(ToolOutcome::Failed(match full {
                crate::state::WorkspaceFull::Files(limit) => format!(
                    "The session workspace holds at most {limit} files, so {key} was not written."
                ),
                crate::state::WorkspaceFull::Bytes(limit) => format!(
                    "The session workspace holds at most {limit} bytes, so {key} was not written."
                ),
                crate::state::WorkspaceFull::StoreBytes(limit) => format!(
                    "The code interpreter sessions of this emulator together hold at most {limit} \
                     bytes, so {key} was not written."
                ),
            }));
        }
        written.push_str(&format!("Wrote {key} ({size} bytes)\n"));
    }
    Ok(ToolOutcome::Content(vec![text_block(written)]))
}

/// `readFiles`: the contents of every path named, as one `resource` block each.
fn read_files(
    session: &crate::state::CodeInterpreterSessionRecord,
    arguments: &ToolArguments,
) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
    let keys = workspace_keys(arguments, "readFiles")?;
    let missing: Vec<&String> = keys
        .iter()
        .filter(|key| !session.files.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        return Ok(not_in_workspace(&missing));
    }
    let mut content = Vec::with_capacity(keys.len());
    // What this result has already materialised, so the response stays proportional to
    // the call rather than to the count times the size a caller chose.
    let mut emitted = 0usize;
    for key in &keys {
        let Some(file) = session.files.get(key) else {
            continue;
        };
        emitted = emitted.saturating_add(file.bytes.len());
        if emitted > MAX_READ_RESULT_BYTES {
            return Ok(ToolOutcome::Failed(format!(
                "The files named hold more than the {MAX_READ_RESULT_BYTES} bytes one readFiles \
                 result carries, so nothing was read. Read fewer files per call."
            )));
        }
        let media_type = media_type_of(key);
        // Text where the stored bytes are text, bytes where they are not: the file's
        // own content decides, not its name.
        let resource = match String::from_utf8(file.bytes.clone()) {
            Ok(text) => ResourceContent {
                r#type: RESOURCE_TEXT.to_string(),
                uri: Some(workspace_uri(key)),
                mime_type: Some(media_type),
                text: Some(text),
                blob: None,
            },
            Err(_) => ResourceContent {
                r#type: RESOURCE_BLOB.to_string(),
                uri: Some(workspace_uri(key)),
                mime_type: Some(media_type),
                text: None,
                blob: Some(file.bytes.clone()),
            },
        };
        content.push(ContentBlock {
            r#type: BLOCK_RESOURCE.to_string(),
            resource: Some(resource),
            ..Default::default()
        });
    }
    Ok(ToolOutcome::Content(content))
}

/// `listFiles`: every workspace entry directly under the directory named, as one
/// `resource_link` block each.
fn list_files(
    session: &crate::state::CodeInterpreterSessionRecord,
    arguments: &ToolArguments,
) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
    let directory = crate::state::normalize_session_directory(
        arguments.directory_path.as_deref().unwrap_or_default(),
    )
    .map_err(|rejection| {
        validation(path_rejection_message(
            arguments.directory_path.as_deref().unwrap_or_default(),
            rejection,
        ))
    })?;
    let content: Vec<ContentBlock> = session
        .list_directory(&directory)
        .into_iter()
        .map(|(key, size)| ContentBlock {
            r#type: BLOCK_RESOURCE_LINK.to_string(),
            uri: Some(workspace_uri(&key)),
            name: Some(key.rsplit('/').next().unwrap_or(&key).to_string()),
            mime_type: Some(match size {
                Some(_) => media_type_of(&key),
                None => DIRECTORY_MEDIA_TYPE.to_string(),
            }),
            size: size.and_then(|size| i64::try_from(size).ok()),
            ..Default::default()
        })
        .collect();
    Ok(ToolOutcome::Content(content))
}

/// `removeFiles`: drop every path named from the session's workspace.
fn remove_files(
    session: &mut crate::state::CodeInterpreterSessionRecord,
    arguments: &ToolArguments,
) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
    let keys = workspace_keys(arguments, "removeFiles")?;
    let missing: Vec<&String> = keys
        .iter()
        .filter(|key| !session.files.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        return Ok(not_in_workspace(&missing));
    }
    let mut removed = String::new();
    for key in &keys {
        session.files.remove(key);
        removed.push_str(&format!("Removed {key}\n"));
    }
    Ok(ToolOutcome::Content(vec![text_block(removed)]))
}

/// The result for a tool whose paths address nothing in the workspace: the tool ran and
/// could not do what was asked.
fn not_in_workspace(missing: &[&String]) -> ToolOutcome {
    let listed = missing
        .iter()
        .map(|key| key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    ToolOutcome::Failed(format!(
        "The session workspace holds no such file: {listed}"
    ))
}

/// The workspace key one tool `path` addresses, or the 400 that says why it addresses
/// none.
fn workspace_key(path: &str) -> Result<String, InvokeCodeInterpreterErrorModeled> {
    crate::state::normalize_session_path(path)
        .map_err(|rejection| validation(path_rejection_message(path, rejection)))
}

/// The workspace keys a tool's `paths` (or its single `path`) addresses, each named
/// once: repeating a path addresses the same file, and de-duplicating is what keeps one
/// repeated name from multiplying the work and the response.
fn workspace_keys(
    arguments: &ToolArguments,
    tool: &str,
) -> Result<Vec<String>, InvokeCodeInterpreterErrorModeled> {
    let named: Vec<String> = match (arguments.paths.as_deref(), arguments.path.as_deref()) {
        (Some(paths), _) if !paths.is_empty() => paths.to_vec(),
        (_, Some(path)) => vec![path.to_string()],
        _ => {
            return Err(validation(format!(
                "The {tool} tool requires the paths argument."
            )))
        }
    };
    if named.len() > MAX_TOOL_PATHS {
        return Err(too_many_paths(tool, named.len()));
    }
    let mut keys: Vec<String> = Vec::with_capacity(named.len());
    for path in &named {
        let key = workspace_key(path)?;
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    Ok(keys)
}

/// The 400 for a tool call that addresses more files than one call may.
fn too_many_paths(tool: &str, named: usize) -> InvokeCodeInterpreterErrorModeled {
    validation(format!(
        "The {tool} tool was given {named} paths; at most {MAX_TOOL_PATHS} are accepted in one \
         call."
    ))
}

/// Why a path addresses nothing, worded for the caller that sent it.
fn path_rejection_message(path: &str, rejection: crate::state::PathRejection) -> String {
    match rejection {
        crate::state::PathRejection::Empty => {
            "A tool path must name a file in the session workspace.".to_string()
        }
        crate::state::PathRejection::Absolute => format!(
            "The tool path {path} is absolute; a path addresses the session workspace and is \
             relative to it."
        ),
        crate::state::PathRejection::Escapes => format!(
            "The tool path {path} leaves the session workspace; a path cannot contain a parent \
             segment."
        ),
        crate::state::PathRejection::TooLong => format!(
            "The tool path {path} is longer than the {} bytes or {} segments a session workspace \
             path has.",
            crate::state::MAX_SESSION_PATH_BYTES,
            crate::state::MAX_SESSION_PATH_SEGMENTS
        ),
    }
}

/// The URI a workspace entry is named by: the workspace is the session's own, so the
/// key IS the whole path.
fn workspace_uri(key: &str) -> String {
    format!("file:///{key}")
}

/// The media type a workspace entry's name implies, which is the tree's ONE reading of
/// what a file extension names (`service_common::web_media_type`), so this plane cannot
/// answer an extension differently from the ones that serve deployed files.
fn media_type_of(key: &str) -> String {
    ::service_common::web_media_type::media_type_of_path(key).to_string()
}

impl<Eng: Send + Sync + 'static> service_common::event_stream::EventStreamOperation
    for InvokeCodeInterpreter<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let response = match self.respond(protocol, req).await {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused invocation is diagnosable.
            // No request payload and no credential reaches it.
            tracing::debug!(
                operation = "InvokeCodeInterpreter",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}

// =============================================================================
// The JavaScript engine: one run, bounded and contained
// =============================================================================

/// What one script run produced.
struct ScriptRun {
    stdout: String,
    stderr: String,
    /// `0` when the script ran to completion, `1` when it ended on an uncaught
    /// exception or hit one of this module's bounds — the status a script host reports
    /// for each.
    exit_code: i32,
    /// How long the engine spent on this run, which is what the result reports as its
    /// `executionTime`: measured around the evaluation only, so no permit wait or lock
    /// acquisition is counted as the script's own time.
    elapsed: Duration,
}

/// What a run wrote to its output streams, as the host collected it.
#[derive(Debug, Default)]
struct Capture {
    stdout: String,
    stderr: String,
    /// Output is collected only while the script the CALLER submitted runs: the
    /// context scripts before it already reported their output to the calls that ran
    /// them, and reporting it twice would be output no caller's script produced.
    recording: bool,
}

impl Capture {
    /// Append `text` to one stream, stopping at [`MAX_CAPTURED_STREAM_BYTES`].
    fn write(&mut self, to_stderr: bool, text: &str) {
        if !self.recording {
            return;
        }
        let stream = if to_stderr {
            &mut self.stderr
        } else {
            &mut self.stdout
        };
        let room = MAX_CAPTURED_STREAM_BYTES.saturating_sub(stream.len());
        if room == 0 {
            return;
        }
        // On a character boundary, so a capped stream is still a string.
        let mut end = text.len().min(room);
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        stream.push_str(&text[..end]);
    }
}

/// The guard over the capture, recovering a poisoned lock rather than panicking with
/// it: the capture is this run's own, so whatever a panic left in it is still the only
/// output there is.
fn captured(capture: &Arc<Mutex<Capture>>) -> ::std::sync::MutexGuard<'_, Capture> {
    capture.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The `console` a script writes to. Defined in JavaScript over the two host functions
/// installed beside it, so the formatting a caller sees is the engine's own.
const CONSOLE_PRELUDE: &str = r#"
globalThis.console = (function () {
    var text = function (value) {
        if (typeof value === "string") { return value; }
        if (value instanceof Error) { return value.stack ? String(value.stack) : String(value); }
        try {
            var json = JSON.stringify(value);
            return json === undefined ? String(value) : json;
        } catch (error) {
            return String(value);
        }
    };
    var line = function (args) {
        return Array.prototype.map.call(args, text).join(" ") + "\n";
    };
    var out = function () { __hostStdout(line(arguments)); };
    var err = function () { __hostStderr(line(arguments)); };
    return { log: out, info: out, debug: out, dir: out, warn: err, error: err, trace: err };
})();
"#;

/// Run `script` in a fresh engine, on top of `context_scripts`.
///
/// Called on a blocking thread with no state guard held. Every bound this module
/// declares is set on the engine before anything is evaluated, so a script cannot
/// outrun them.
fn run_script(context_scripts: &[String], script: &str) -> ScriptRun {
    let started = Instant::now();
    let capture = Arc::new(Mutex::new(Capture::default()));
    let outcome = evaluate(&capture, context_scripts, script);
    let elapsed = started.elapsed();
    let captured = captured(&capture);
    let mut stdout = captured.stdout.clone();
    let mut stderr = captured.stderr.clone();
    drop(captured);
    match outcome {
        Ok(()) => ScriptRun {
            stdout,
            stderr,
            exit_code: 0,
            elapsed,
        },
        Err(failure) => {
            // The engine stops a script that outran the wall clock by raising inside
            // it; say so as what it is rather than passing the engine's interrupt on.
            let reason = if elapsed >= RUN_WALL_CLOCK {
                format!(
                    "The script was stopped after {} seconds.",
                    RUN_WALL_CLOCK.as_secs()
                )
            } else {
                failure
            };
            if !stderr.is_empty() && !stderr.ends_with('\n') {
                stderr.push('\n');
            }
            stderr.push_str(&reason);
            stderr.push('\n');
            if !stdout.is_empty() && !stdout.ends_with('\n') {
                stdout.push('\n');
            }
            ScriptRun {
                stdout,
                stderr,
                exit_code: 1,
                elapsed,
            }
        }
    }
}

/// Build the engine, install the console, replay the session's context and evaluate
/// the submitted script. `Err` carries the engine's own account of what went wrong.
fn evaluate(
    capture: &Arc<Mutex<Capture>>,
    context_scripts: &[String],
    script: &str,
) -> Result<(), String> {
    let runtime = Runtime::new().map_err(|error| format!("the engine did not start: {error}"))?;
    runtime.set_memory_limit(RUN_MEMORY_BYTES);
    runtime.set_max_stack_size(RUN_STACK_BYTES);
    let deadline = Instant::now() + RUN_WALL_CLOCK;
    // The only thing that can stop a script with no exit: a timeout cannot preempt a
    // loop running on this thread, but the engine asks this handler as it works.
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context =
        Context::full(&runtime).map_err(|error| format!("the engine did not start: {error}"))?;

    context.with(|ctx| {
        install_console(&ctx, capture)?;
        // The context this session already built, re-evaluated with its output not
        // collected, so the variables and functions earlier runs defined are in scope.
        for earlier in context_scripts {
            run_one(&ctx, earlier)?;
        }
        captured(capture).recording = true;
        run_one(&ctx, script)
    })
}

/// Evaluate one script in `ctx`, turning an uncaught exception into the text a caller
/// sees on `stderr`.
fn run_one(ctx: &Ctx<'_>, script: &str) -> Result<(), String> {
    ctx.eval::<Value<'_>, _>(script)
        .catch(ctx)
        .map(|_| ())
        .map_err(|caught| caught.to_string())
}

/// Install the two host functions the console writes through, then the console itself.
///
/// These are the ONLY host functions a script can reach: there is no module loader, and
/// plain QuickJS exposes no filesystem, process, environment or network API.
fn install_console(ctx: &Ctx<'_>, capture: &Arc<Mutex<Capture>>) -> Result<(), String> {
    let to_stdout = Arc::clone(capture);
    ctx.globals()
        .set(
            "__hostStdout",
            Func::from(move |text: String| captured(&to_stdout).write(false, &text)),
        )
        .map_err(|error| format!("the console could not be installed: {error}"))?;
    let to_stderr = Arc::clone(capture);
    ctx.globals()
        .set(
            "__hostStderr",
            Func::from(move |text: String| captured(&to_stderr).write(true, &text)),
        )
        .map_err(|error| format!("the console could not be installed: {error}"))?;
    run_one(ctx, CONSOLE_PRELUDE)
}

// SPDX-License-Identifier: CC0-1.0

//! RUNNING a function: what an invocation of a stored deployment package comes to.
//!
//! An invocation's answer is the handler's: "For synchronous invocation, details about the
//! function response, including errors, are included in the response body and headers"
//! (`Invoke`). So this module runs the handler — the package's modules are compiled and
//! executed in this process by the engine for the function's own language ([`python`],
//! [`node`]), with the event the caller sent and the context the runtime reports — and
//! reports back exactly what the run produced: the value the handler returned, serialized
//! by that engine; the error document and `Unhandled` signal of an exception it raised; and
//! the log the run wrote. Nothing here is synthesised: a payload, a log line, an error type
//! or a duration is the one the run produced, and a package no engine here can run is
//! reported as the modelled failure for it rather than as a result.
//!
//! # Which packages run
//!
//! A `Zip` package whose `Runtime` names a language this emulator embeds an engine for
//! ([`engine_for`]) — the Python runtimes and the node ones — out of the bytes the store
//! holds for the version the invocation resolved. Every other package — a container image,
//! a runtime this emulator embeds no engine for — is [`Unrunnable::NoEngine`], which the
//! calling operation answers as the modelled `InvalidRuntimeException`: no response payload
//! is returned that no function produced.
//!
//! # What bounds a run
//!
//! Two ceilings, for two different things:
//!
//! * the RUN's own budget, which the engine spends and the guest cannot reach
//!   ([`Budget`]): the function's own `Timeout`, clamped to the range the model
//!   allows for it, plus the step, memory, log and response bounds;
//! * how long the CALLER waits, which is this module's ([`HOST_WAIT_CEILING`]) and is
//!   never the guest's to set: a run still going when the caller's wait is spent is
//!   ABANDONED — its slot goes back to the pool at once, it counts against
//!   [`MAX_ABANDONED_RUNS`] for as long as its thread lasts, and past that cap an
//!   invocation is refused with the modelled throttle rather than adding another thread.
//!
//! At most [`MAX_CONCURRENT_RUNS`] handlers run at once, so the memory and CPU the
//! engines can hold is bounded however many invocations arrive; a call that waits
//! [`PERMIT_WAIT`] for a slot is answered with the modelled throttle. Every run happens on
//! a thread of its own with no state guard of this service held, so one caller's handler
//! cannot stall another caller's request.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

/// The modules of a .zip deployment package, read out of the bytes the store holds.
pub mod package;

/// What one run may spend, and what it came to: the same for every engine.
pub mod outcome;
pub use outcome::{Budget, Completion, Finished, Stop};

/// The Python interpreter one invocation runs in, and everything that bounds and contains
/// it.
pub mod python;

/// The JavaScript engine one invocation runs in, and everything that bounds and contains
/// it.
pub mod node;

/// The runtime the handler runs under, in the language the handler is written in.
mod bootstrap;

/// The AWS SDK call seam a running handler reaches the rest of this emulator through.
pub mod aws_api;
pub use aws_api::AwsAccess;

/// Binding one of those calls to HTTP for a service whose protocol is `restJson1`.
pub mod rest_json;

/// The AWS SDK modules a Python handler imports, which that seam is reached through.
mod aws_sdk_guest;

/// Delivering one invocation's log to CloudWatch Logs, which is where a client reads it.
pub mod log_delivery;
pub use log_delivery::{log_target, LogEvent, LogGrants, LogTarget};

/// The AWS SDK modules a node handler imports, reached through that same seam.
mod node_sdk_guest;

/// How many LAYER archives one run reads.
///
/// AWS's own quota is five layers per function, so a configuration naming more than that
/// has more layers than a function may have. The cap is applied in HOST code before an
/// archive is read, so the number of archives one invocation holds and inflates does not
/// grow with the list a caller wrote — the same ARN repeated is still one archive per entry.
pub const MAX_RUN_LAYER_ARCHIVES: usize = 5;

/// The archives of the layers one function's configuration holds, in the order it names
/// them, at most [`MAX_RUN_LAYER_ARCHIVES`] of them.
///
/// A layer entry this emulator holds no archive bytes for contributes nothing: the layer
/// root simply does not hold its files, which is what a handler importing from it sees. The
/// bytes are SHARED rather than copied — a published layer version is immutable — so an
/// invocation holds one reference per entry and not one copy.
#[must_use]
pub fn layer_archives(layers: &[super::FunctionLayer]) -> Vec<Arc<Vec<u8>>> {
    layers
        .iter()
        .take(MAX_RUN_LAYER_ARCHIVES)
        .filter_map(|layer| layer.zip_bytes.clone())
        .collect()
}

/// The `posix` module a run's Python is built on: this emulator's own, with no host call
/// in it.
mod host_posix;

/// Whether the key a function's environment variables are encrypted with can decrypt them
/// — the check every invocation of such a function begins with.
pub mod environment_key;
pub use environment_key::{durable_data_key_fault, environment_key_fault, EnvironmentKeyFault};

/// How many handlers run at once.
pub const MAX_CONCURRENT_RUNS: usize = 4;

/// How long an invocation waits for one of those slots before it is throttled.
pub const PERMIT_WAIT: Duration = Duration::from_secs(5);

/// The longest a CALLER waits for a run, whatever the function's own timeout says.
///
/// This is the HOST's ceiling, not the function's: a `Timeout` of up to 900 seconds would
/// otherwise hold a request open for a quarter of an hour. The run's own deadline is the
/// function's clamped `Timeout` ([`run_budget`]); this is only how long the answer is
/// waited for.
pub const HOST_WAIT_CEILING: Duration = Duration::from_secs(30);

/// How long after its budget is spent a run is still waited for before it is abandoned.
pub const ABANDON_GRACE: Duration = Duration::from_secs(2);

/// How many abandoned runs may be outstanding before an invocation is throttled instead of
/// starting another.
pub const MAX_ABANDONED_RUNS: usize = 8;

/// The stack one run's thread is given. Well above the interpreter's clamped recursion
/// limit, so deep guest recursion ends that run with the interpreter's own
/// `RecursionError` rather than touching the thread's guard page.
pub(crate) const RUN_STACK_BYTES: usize = 16 * 1024 * 1024;

/// The smallest `Timeout` the model allows (`smithy_query(query="shape",
/// shape="Timeout")`: `@range(min: 1)`), which a stored value is clamped up to before it
/// bounds a run.
const MIN_TIMEOUT_SECONDS: i64 = 1;

/// The largest `Timeout` the model allows for a function (AWS publishes 900 seconds, which
/// is the maximum `CreateFunction` accepts), which a stored value is clamped down to.
const MAX_TIMEOUT_SECONDS: i64 = 900;

/// The smallest `MemorySize` the model allows (`smithy_query(query="shape",
/// shape="MemorySize")`: `@range(min: 128, max: 32768)`), which a stored value is clamped
/// up to before it bounds a run.
const MIN_MEMORY_SIZE_MB: i64 = 128;

/// The largest `MemorySize` that same range allows, which a stored value is clamped down
/// to.
const MAX_MEMORY_SIZE_MB: i64 = 32_768;

/// The bytes of one MB of a function's `MemorySize`, which the value is stated in.
const BYTES_PER_MB: u64 = 1024 * 1024;

/// The REQUEST ID one invocation is identified by, in the log it writes and in the error
/// document it answers with: a fresh UUID per invocation, which is what `aws_request_id`
/// is on the real runtime.
#[must_use]
pub fn mint_request_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// How many traced events one run may execute. Large enough for any handler a test or a
/// client runs, small enough that a loop with no exit ends inside the caller's wait.
const MAX_RUN_STEPS: u64 = 20_000_000;

/// How much memory one run may add to this process.
const MAX_RUN_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

/// How deep one run may recurse.
///
/// The interpreter's own default is far LOWER than Python's (256 against CPython's 1000),
/// and the standard library's own import chain needs more than that, so this is the limit
/// a run is given rather than a reduction: it is set before any guest code runs and the
/// call that would raise it is taken away, so it is the ceiling for the whole run.
const MAX_RUN_RECURSION: usize = 2_000;

/// How much of a run's output is kept. The model reports "The last 4 KB of the execution
/// log" on `LogResult`, so what is kept is a multiple of that rather than unbounded.
const MAX_RUN_LOG_BYTES: usize = 256 * 1024;

/// How large a handler's response may be, which is the quota Lambda names in its own
/// refusal for a response past it ([`RESPONSE_SIZE_TOO_LARGE`]) — a response of exactly
/// this many bytes is delivered, and one byte more is not.
const MAX_RESPONSE_BYTES: usize = 6_291_556;

/// The bytes of a run's log the model reports on `LogResult`: "The last 4 KB of the
/// execution log, which is base64-encoded."
pub const LOG_TAIL_BYTES: usize = 4 * 1024;

/// The `FunctionError` an invocation whose handler raised reports: "If present, indicates
/// that an error occurred during function execution." AWS reports the error of a handler
/// that raised as `Unhandled`.
pub const FUNCTION_ERROR_UNHANDLED: &str = "Unhandled";

/// The `Runtime` values this emulator runs with the Python interpreter.
///
/// Matched by prefix because the identifiers are versioned (`python3.12`,
/// `python3.13`, …) and the interpreter runs the language rather than one of its point
/// releases (source for the value set: `smithy_query(query="shape", shape="Runtime")`).
const PYTHON_RUNTIME_PREFIX: &str = "python";

/// The `Runtime` values this emulator runs with the JavaScript engine, by the same rule:
/// every `nodejs…` identifier of that same value set names the one language.
const NODE_RUNTIME_PREFIX: &str = "nodejs";

/// Which engine runs a function's handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// The Python interpreter ([`python`]).
    Python,
    /// The JavaScript engine ([`node`]).
    Node,
}

/// The engine that runs a package for `runtime`, or `None` when this emulator embeds none
/// for it.
#[must_use]
pub fn engine_for(runtime: Option<&str>) -> Option<Engine> {
    let runtime = runtime?;
    if runtime.starts_with(PYTHON_RUNTIME_PREFIX) {
        return Some(Engine::Python);
    }
    if runtime.starts_with(NODE_RUNTIME_PREFIX) {
        return Some(Engine::Node);
    }
    None
}

/// Whether `runtime` names a runtime this emulator can run a package for.
#[must_use]
pub fn runs_handler(runtime: Option<&str>) -> bool {
    engine_for(runtime).is_some()
}

/// Everything one invocation of a stored function needs to run.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The function's name.
    pub function_name: String,
    /// The version the invocation resolved to — what `ExecutedVersion` reports.
    pub executed_version: String,
    /// The ARN the invocation addressed, as `context.invoked_function_arn`.
    pub invoked_function_arn: String,
    /// The Region the invocation was addressed to.
    pub region: String,
    /// The `Runtime` of the resolved configuration.
    pub runtime: Option<String>,
    /// The `Handler` of the resolved configuration.
    pub handler: Option<String>,
    /// The function's own environment variables.
    pub variables: BTreeMap<String, String>,
    /// The function's `MemorySize`, in MB.
    pub memory_size: i32,
    /// The ARCHITECTURE the function is configured for (`Architectures`), which is what
    /// the runtime reports as its own.
    pub architecture: String,
    /// The function's `Timeout`, in seconds.
    pub timeout: i32,
    /// The deployment package bytes of the resolved version, when this emulator holds
    /// them.
    pub zip_bytes: Option<Vec<u8>>,
    /// The archives of the LAYERS the resolved configuration holds, in the order the
    /// configuration names them — the content a managed runtime mounts under the layer
    /// root, where a later layer's entry wins over an earlier one's. Shared rather than
    /// copied: a published layer version is immutable.
    pub layer_zips: Vec<Arc<Vec<u8>>>,
    /// The event payload the caller sent.
    pub payload: Vec<u8>,
    /// The `ClientContext` the caller sent, already decoded from base64 to its JSON text.
    pub client_context: Option<String>,
    /// The `TenantId` the caller sent.
    pub tenant_id: Option<String>,
    /// This invocation's request id.
    pub request_id: String,
    /// What the run reports as `AWS_LAMBDA_INITIALIZATION_TYPE`: which kind of execution
    /// environment is serving this invocation, as the admission that let it in decided
    /// ([`super::execution_slot`]).
    pub initialization_type: &'static str,
    /// How the handler reaches the rest of this emulator through the AWS SDK this runtime
    /// supplies it ([`aws_api`]), or `None` where there is no seam to dispatch a call on.
    pub aws: Option<AwsAccess>,
    /// Where this invocation's log is delivered and what the function's execution role may
    /// do there ([`log_delivery`]), or `None` for a run whose caller resolved no target —
    /// in which case the run's log is still its answer's log tail and simply reaches no log
    /// group.
    pub logs: Option<LogTarget>,
}

/// Why an invocation ran nothing at all.
///
/// Each of these is a condition the CALLING operation answers as one of its own modelled
/// errors: nothing here is reported as a result of a run that did not happen.
#[derive(Debug, Clone)]
pub enum Unrunnable {
    /// This emulator embeds no engine for the function's runtime or package type.
    NoEngine {
        /// What the function's code is, as the configuration names it.
        runtime: String,
    },
    /// The function's configuration names no handler to run.
    NoHandler,
    /// This emulator holds no bytes for the resolved version's package.
    PackageNotHeld,
    /// The bytes it holds are not a readable archive.
    PackageUnreadable(String),
    /// Too many handlers are already running, or too many runs have been abandoned.
    Overloaded(String),
    /// The run could not be started or driven at all. Never a guest failure.
    Internal(String),
}

/// What one invocation came to.
#[derive(Debug, Clone)]
pub struct Invoked {
    /// The response payload: what the handler returned, or the error document for the
    /// failure it came to.
    pub payload: Vec<u8>,
    /// The `FunctionError` to report, when the run failed.
    pub function_error: Option<String>,
    /// The whole log of the run.
    pub log: String,
    /// That same log as the events CloudWatch Logs holds it as, one per line
    /// ([`log_delivery`]).
    pub log_events: Vec<LogEvent>,
    /// How long the run took.
    pub duration: Duration,
    /// The WRITES a response-streaming handler made, in order — empty for a handler that
    /// returned a value instead. A streaming caller is delivered one chunk per entry, so
    /// the chunk boundaries it sees are the handler's own.
    pub chunks: Vec<Vec<u8>>,
}

impl Invoked {
    /// The `LogResult` for this run: "The last 4 KB of the execution log, which is
    /// base64-encoded."
    #[must_use]
    pub fn log_tail(&self) -> String {
        let bytes = self.log.as_bytes();
        let from = bytes.len().saturating_sub(LOG_TAIL_BYTES);
        // Start the tail on a character boundary, so what is reported is still text.
        let mut start = from;
        while start < bytes.len() && !self.log.is_char_boundary(start) {
            start += 1;
        }
        ::base64::Engine::encode(
            &::base64::engine::general_purpose::STANDARD,
            &bytes[start..],
        )
    }
}

/// Run `invocation`'s handler and report what it came to.
///
/// The run happens on a thread of its own, holding one of this module's run slots and no
/// state guard of this service, and the caller's wait for it is bounded by
/// [`HOST_WAIT_CEILING`] however long the function's own timeout is.
pub async fn run_invocation(invocation: Invocation) -> Result<Invoked, Unrunnable> {
    let Some(engine) = engine_for(invocation.runtime.as_deref()) else {
        return Err(Unrunnable::NoEngine {
            runtime: invocation
                .runtime
                .clone()
                .unwrap_or_else(|| "an image package".to_string()),
        });
    };
    let Some(handler) = invocation.handler.clone() else {
        return Err(Unrunnable::NoHandler);
    };
    // Each engine imports the sources of its own language: the entries a package holds for
    // the other one are not modules this run could load. One budget is spent across every
    // mount this invocation reads ([`package::ReadBudget`]).
    let mut read_budget = package::ReadBudget::default();
    let read = match engine {
        Engine::Python => package::python_modules(invocation.zip_bytes.as_deref()),
        Engine::Node => package::javascript_modules_within(
            invocation.zip_bytes.as_deref(),
            &mut read_budget,
        ),
    };
    let mut modules = match read {
        Ok(modules) => modules,
        Err(package::PackageFault::NotHeld) => return Err(Unrunnable::PackageNotHeld),
        Err(package::PackageFault::Unreadable(why)) => {
            return Err(Unrunnable::PackageUnreadable(why))
        }
    };
    // The layers' own entries, under the prefix the runtime keys the layer root by, read
    // out of the SAME budget the deployment package was: what one invocation holds is
    // bounded across its mounts rather than per archive. A layer this emulator holds no
    // readable archive for mounts nothing, exactly as a deployment package it holds no
    // bytes for imports nothing.
    if engine == Engine::Node {
        for archive in &invocation.layer_zips {
            if let Ok(entries) =
                package::javascript_modules_within(Some(archive.as_slice()), &mut read_budget)
            {
                modules.extend(entries.into_iter().map(|entry| package::PackageModule {
                    path: format!("{}{}", node::LAYER_ID_PREFIX, entry.path),
                    source: entry.source,
                }));
            }
        }
    }
    // The modules the runtime itself supplies on the search path, which is where the AWS
    // SDK a managed node runtime ships is found.
    if engine == Engine::Node {
        modules.extend(node_sdk_guest::runtime_modules());
    }

    if abandoned_runs() >= MAX_ABANDONED_RUNS {
        return Err(Unrunnable::Overloaded(format!(
            "{MAX_ABANDONED_RUNS} invocations of this emulator are still running past their \
             budget. Retry the request."
        )));
    }
    // One of the run slots, waited for only as long as [`PERMIT_WAIT`]: past that the
    // caller is throttled rather than queueing behind other callers' handlers. Held until
    // this invocation is answered, including when its run is abandoned — which is what
    // puts the slot back at once.
    let permit = ::tokio::time::timeout(PERMIT_WAIT, Arc::clone(run_slots()).acquire_owned())
        .await
        .map_err(|_| {
            Unrunnable::Overloaded(
                "Too many invocations are running in this emulator. Retry the request."
                    .to_string(),
            )
        })?
        .map_err(|error| Unrunnable::Internal(format!("the run slots are gone: {error}")))?;

    let budget = run_budget(invocation.timeout);
    let heap_bytes = run_heap_bytes(invocation.memory_size);
    let started = SystemTime::now();
    let request_id = invocation.request_id.clone();
    let reported = invocation.clone();
    let (sender, receiver) = ::tokio::sync::oneshot::channel();
    let lifecycle = Arc::new(AtomicU8::new(RUN_RUNNING));
    let thread_lifecycle = Arc::clone(&lifecycle);
    let environment = runtime_environment(&invocation, started);
    let spawned = ::std::thread::Builder::new()
        .name("lambda-invocation".to_string())
        .stack_size(RUN_STACK_BYTES)
        .spawn(move || {
            // The lifecycle is marked by a GUARD, so the mark and the release of an
            // abandoned run's count happen on every exit of this thread — including an
            // unwind out of the interpreter, which would otherwise leave the count
            // claimed for a thread that is no longer there.
            let _mark = ThreadLifecycle {
                lifecycle: thread_lifecycle,
            };
            let event = event_text(&invocation.payload);
            let finished = match engine {
                Engine::Python => python::run(&python::Run {
                    modules: &modules,
                    handler: &handler,
                    environment: &environment,
                    event: event.as_deref(),
                    request_id: &invocation.request_id,
                    invoked_function_arn: &invocation.invoked_function_arn,
                    client_context: invocation.client_context.as_deref(),
                    tenant_id: invocation.tenant_id.as_deref(),
                    budget,
                    aws: invocation.aws.as_ref(),
                }),
                Engine::Node => node::run(&node::Run {
                    modules: &modules,
                    handler: &handler,
                    runtime: invocation.runtime.as_deref().unwrap_or_default(),
                    architecture: &invocation.architecture,
                    environment: &environment,
                    event: event.as_deref(),
                    request_id: &invocation.request_id,
                    invoked_function_arn: &invocation.invoked_function_arn,
                    client_context: invocation.client_context.as_deref(),
                    tenant_id: invocation.tenant_id.as_deref(),
                    budget,
                    heap_bytes,
                    aws: invocation.aws.as_ref(),
                }),
            };
            let _ = sender.send(finished);
        });
    if let Err(error) = spawned {
        return Err(Unrunnable::Internal(format!(
            "the invocation thread could not be started: {error}"
        )));
    }

    let wait = budget.wall_clock.saturating_add(ABANDON_GRACE).min(
        HOST_WAIT_CEILING.saturating_add(ABANDON_GRACE),
    );
    // The accounting of a run that outlives the caller is a GUARD rather than a branch, so
    // it happens however this invocation ends: the wait running out, the caller's request
    // being cancelled while it is suspended (which drops this future at the await below),
    // or the answer arriving. Without it a cancelled invocation would hand its slot back
    // and leave an uncounted thread running, which is no cap at all.
    let accounting = RunAccounting {
        lifecycle: Arc::clone(&lifecycle),
    };
    let waited = ::tokio::time::timeout(wait, receiver).await;
    drop(accounting);
    drop(permit);
    let finished = match waited {
        Ok(Ok(finished)) => finished,
        Ok(Err(_)) => {
            return Err(Unrunnable::Internal(
                "the invocation thread ended without reporting".to_string(),
            ))
        }
        Err(_) => {
            // The run is still going after its budget and the grace on top of it. It has
            // been given up on: the slot goes back to the pool and the guard above counted
            // the run as abandoned until its thread ends.
            return Err(Unrunnable::Internal(format!(
                "the invocation was given up on after {} seconds",
                wait.as_secs()
            )));
        }
    };

    let invoked = answered(finished, &request_id, &reported, budget, engine, started);
    // The log an invocation wrote is not part of its answer: Lambda delivers it to the
    // function's own log group, which is where a client reads it ([`log_delivery`]). The
    // delivery is made as the function's execution role and before this invocation is
    // answered, so a client reading the group straight after the call finds the run's lines.
    if let (Some(aws), Some(target)) = (reported.aws.as_ref(), reported.logs.as_ref()) {
        log_delivery::deliver(
            aws,
            target,
            &reported.region,
            &log_stream_name(&reported.executed_version, &reported.request_id, started),
            &invoked.log_events,
        )
        .await;
    }
    Ok(invoked)
}

/// Marks a run's thread as FINISHED on every exit of that thread, including an unwind out
/// of the interpreter, and gives back the count of a run the caller had already given up
/// on.
///
/// Whichever of this guard and [`RunAccounting`] marks the lifecycle first decides the
/// accounting, so an abandoned run is counted once and released once.
struct ThreadLifecycle {
    /// The run's lifecycle, shared with the task that waits for it.
    lifecycle: Arc<AtomicU8>,
}

impl Drop for ThreadLifecycle {
    fn drop(&mut self) {
        if self.lifecycle.swap(RUN_FINISHED, Ordering::SeqCst) == RUN_ABANDONED {
            release_abandoned_run();
        }
    }
}

/// Counts a run that is still going when the caller stops waiting for it, on EVERY way of
/// stopping: the wait running out, and the invocation's future being dropped where it waits
/// (a cancelled request).
///
/// A run whose thread has already ended is not abandoned, and the thread's own mark is what
/// says so — exactly one of the two marks the lifecycle, so an abandoned run is counted once
/// and released once ([`release_abandoned_run`]).
struct RunAccounting {
    /// The run's lifecycle, shared with its thread.
    lifecycle: Arc<AtomicU8>,
}

impl Drop for RunAccounting {
    fn drop(&mut self) {
        // Counted FIRST and then released if the thread got there first, so the count is
        // never observed low while a claim is in flight: the thread's own release can only
        // run after this claim, whichever of the two marks the lifecycle first.
        claim_abandoned_run();
        if self.lifecycle.swap(RUN_ABANDONED, Ordering::SeqCst) == RUN_FINISHED {
            // The thread ended on its own, so there is nothing outstanding to count. Its
            // mark stands and the claim above is given back.
            self.lifecycle.store(RUN_FINISHED, Ordering::SeqCst);
            release_abandoned_run();
        }
    }
}

/// How a finished run is reported.
fn answered(
    finished: Finished,
    request_id: &str,
    invocation: &Invocation,
    budget: Budget,
    engine: Engine,
    started: SystemTime,
) -> Invoked {
    let (payload, function_error) = match &finished.completion {
        Completion::Returned(payload) => (payload.clone().into_bytes(), None),
        // A response-streaming handler's answer is the bytes it WROTE. The stream IS the
        // answer, and one that has been begun cannot be withdrawn, so no failure is
        // attributed to the invocation: the caller is delivered what the handler wrote, and
        // a handler that failed before its first write delivers nothing. Its failure is in
        // the run's LOG, as the `Invoke Error` record the runtime writes for it.
        Completion::Streamed(chunks) => (chunks.concat().into_bytes(), None),
        Completion::Raised {
            error_type,
            error_message,
            stack_trace,
        } => (
            error_document(
                error_message,
                Some(error_type),
                request_id,
                stack_trace,
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        Completion::Stopped(Stop::Deadline) => (
            error_document(
                &format!(
                    "{request_id} Task timed out after {:.2} seconds",
                    budget.wall_clock.as_secs_f64()
                ),
                None,
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        // A run the memory ceiling ended is one whose runtime was killed, which is what
        // AWS reports it as: "Runtime exited with error: signal: killed".
        Completion::Stopped(Stop::Memory) => (
            error_document(
                &format!("RequestId: {request_id} Error: {RUNTIME_KILLED}"),
                Some(RUNTIME_EXIT_ERROR),
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        // A run the RESPONSE ceiling ended produced an answer Lambda will not carry, and
        // Lambda attributes that failure to the response rather than reporting it as a
        // runtime that exited: the error names the response-size class and states the
        // quota the response passed.
        Completion::Stopped(Stop::Response) => (
            error_document(
                &format!(
                    "Response payload size exceeded maximum allowed payload size ({} bytes).",
                    budget.response_bytes
                ),
                Some(RESPONSE_SIZE_TOO_LARGE),
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        Completion::Stopped(stop) => (
            error_document(
                stop.reason(),
                Some(RUNTIME_EXIT_ERROR),
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        Completion::Internal(why) => (
            error_document(
                why,
                Some("Runtime.InternalError"),
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
    };
    let log = run_log(
        request_id,
        &invocation.executed_version,
        invocation.memory_size,
        &finished,
    );
    Invoked {
        payload,
        function_error,
        log_events: log_delivery::events(&log.lines, log.opened, started, finished.elapsed),
        log: log.text(),
        duration: finished.elapsed,
        chunks: match &finished.completion {
            Completion::Streamed(chunks) => {
                chunks.iter().map(|chunk| chunk.clone().into_bytes()).collect()
            }
            _ => Vec::new(),
        },
    }
}

/// The `errorType` of a run whose runtime ended instead of answering.
const RUNTIME_EXIT_ERROR: &str = "Runtime.ExitError";

/// The `errorType` of a run whose answer was larger than an invocation may return. The
/// failure is the RESPONSE's, so it is attributed to the function's response rather than
/// reported as a runtime that exited.
const RESPONSE_SIZE_TOO_LARGE: &str = "Function.ResponseSizeTooLarge";

/// What AWS reports as the message of such a run when the sandbox ended it.
const RUNTIME_KILLED: &str = "Runtime exited with error: signal: killed";

/// The member the error document carries the handler's stack under, which is the one the
/// function's own runtime reports it under: `stackTrace` for a Python handler, `trace` for
/// a node one.
fn stack_trace_member(engine: Engine) -> &'static str {
    match engine {
        Engine::Python => "stackTrace",
        Engine::Node => "trace",
    }
}

/// Whether the error payload carries the invocation's own request id.
///
/// A Python runtime's error payload names it (`requestId` beside `stackTrace`); a node
/// runtime's does not — the id is carried in the RESPONSE rather than in the body, so the
/// payload a node handler's failure answers with is exactly the three members
/// `{errorType, errorMessage, trace}`.
fn reports_request_id(engine: Engine) -> bool {
    match engine {
        Engine::Python => true,
        Engine::Node => false,
    }
}

/// The error payload an invocation that failed answers with: the members the function's
/// own runtime reports for a handler that raised.
fn error_document(
    message: &str,
    error_type: Option<&str>,
    request_id: &str,
    stack_trace: &[String],
    engine: Engine,
) -> Vec<u8> {
    let mut document = ::serde_json::Map::new();
    document.insert(
        "errorMessage".to_string(),
        ::serde_json::Value::String(message.to_string()),
    );
    if let Some(error_type) = error_type {
        document.insert(
            "errorType".to_string(),
            ::serde_json::Value::String(error_type.to_string()),
        );
    }
    if reports_request_id(engine) {
        document.insert(
            "requestId".to_string(),
            ::serde_json::Value::String(request_id.to_string()),
        );
    }
    document.insert(
        stack_trace_member(engine).to_string(),
        ::serde_json::Value::Array(
            stack_trace
                .iter()
                .map(|frame| ::serde_json::Value::String(frame.clone()))
                .collect(),
        ),
    );
    ::serde_json::Value::Object(document).to_string().into_bytes()
}

/// The LOG of one run, as Lambda writes it: the invocation's start and end lines around
/// what the handler wrote, and the report line that states what the run spent.
///
/// Every number in it is measured: the duration is the run's own, and the memory is what
/// the run was allowed to hold (see [`python`], [`node`]).
///
/// It is kept as its LINES because that is what CloudWatch Logs holds it as — one event per
/// line ([`log_delivery`]) — and the whole text an invocation answers a log tail from is
/// those same lines.
#[derive(Debug)]
struct RunLog {
    /// Every line of the log, in order and without its line break.
    lines: Vec<String>,
    /// How many of those lines OPEN the log: the framing Lambda writes before the handler's
    /// own output.
    opened: usize,
}

impl RunLog {
    /// The whole log as text, each line closed by its break.
    fn text(&self) -> String {
        let mut text = String::new();
        for line in &self.lines {
            text.push_str(line);
            text.push('\n');
        }
        text
    }
}

fn run_log(request_id: &str, version: &str, memory_size: i32, finished: &Finished) -> RunLog {
    let duration_ms = finished.elapsed.as_secs_f64() * 1_000.0;
    let billed_ms = duration_ms.ceil().max(1.0);
    let mut lines = vec![format!("START RequestId: {request_id} Version: {version}")];
    let opened = lines.len();
    if !finished.log.is_empty() {
        let mut written: Vec<&str> = finished.log.split('\n').collect();
        // A log that ends on a line break closes its last line rather than opening an empty
        // one, so the piece after that break is not a line of the log.
        if written.last().is_some_and(|last| last.is_empty()) {
            written.pop();
        }
        lines.extend(written.into_iter().map(str::to_string));
    }
    lines.push(format!("END RequestId: {request_id}"));
    lines.push(format!(
        "REPORT RequestId: {request_id}\tDuration: {duration_ms:.2} ms\tBilled Duration: \
         {billed_ms:.0} ms\tMemory Size: {memory_size} MB"
    ));
    RunLog { lines, opened }
}

/// The event text the handler is called with, or `None` for an invocation that carried no
/// payload at all.
fn event_text(payload: &[u8]) -> Option<String> {
    if payload.is_empty() {
        return None;
    }
    String::from_utf8(payload.to_vec()).ok()
}

/// What one run may spend: the function's own `Timeout`, clamped to the range the model
/// allows for it and to this module's ceiling, plus the bounds that are this emulator's.
#[must_use]
pub fn run_budget(timeout_seconds: i32) -> Budget {
    let clamped = i64::from(timeout_seconds).clamp(MIN_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS);
    let wall_clock = Duration::from_secs(u64::try_from(clamped).unwrap_or(1)).min(HOST_WAIT_CEILING);
    Budget {
        wall_clock,
        steps: MAX_RUN_STEPS,
        memory_bytes: MAX_RUN_MEMORY_BYTES,
        log_bytes: MAX_RUN_LOG_BYTES,
        response_bytes: MAX_RESPONSE_BYTES,
        recursion_limit: MAX_RUN_RECURSION,
    }
}

/// The memory one run may HOLD: the function's own `MemorySize`, clamped to the range the
/// model allows for it and to this module's own per-run ceiling.
///
/// This is what an engine with an allocation ceiling of its own is given ([`node`]), so
/// that a function's configured memory is the memory its handler really has.
#[must_use]
pub fn run_heap_bytes(memory_size: i32) -> usize {
    let clamped = i64::from(memory_size).clamp(MIN_MEMORY_SIZE_MB, MAX_MEMORY_SIZE_MB);
    let bytes = u64::try_from(clamped)
        .unwrap_or(1)
        .saturating_mul(BYTES_PER_MB)
        .min(MAX_RUN_MEMORY_BYTES);
    usize::try_from(bytes).unwrap_or(usize::MAX)
}

/// The ENVIRONMENT one run sees: the function's own variables, plus the ones the runtime
/// sets for a handler to read its own identity out of.
///
/// This is the whole environment: no host environment variable is in it, and no credential
/// of this emulator is either.
fn runtime_environment(invocation: &Invocation, started: SystemTime) -> BTreeMap<String, String> {
    let mut environment = invocation.variables.clone();
    let reserved: &[(&str, String)] = &[
        ("AWS_LAMBDA_FUNCTION_NAME", invocation.function_name.clone()),
        (
            "AWS_LAMBDA_FUNCTION_VERSION",
            invocation.executed_version.clone(),
        ),
        (
            "AWS_LAMBDA_FUNCTION_MEMORY_SIZE",
            invocation.memory_size.to_string(),
        ),
        (
            "AWS_LAMBDA_LOG_GROUP_NAME",
            invocation.logs.as_ref().map_or_else(
                || super::default_log_group(&invocation.function_name),
                |target| target.log_group.clone(),
            ),
        ),
        (
            "AWS_LAMBDA_LOG_STREAM_NAME",
            log_stream_name(&invocation.executed_version, &invocation.request_id, started),
        ),
        ("AWS_REGION", invocation.region.clone()),
        ("AWS_DEFAULT_REGION", invocation.region.clone()),
        (
            "AWS_LAMBDA_INITIALIZATION_TYPE",
            invocation.initialization_type.to_string(),
        ),
        // "AWS_LAMBDA_RUNTIME_API – … The host and port of the runtime API", which a
        // managed runtime's own environment always names. This emulator serves the
        // invocation to the handler in process rather than over that interface, so the
        // variable names the loopback address the interface is addressed at and nothing
        // reads it.
        ("AWS_LAMBDA_RUNTIME_API", "127.0.0.1:9001".to_string()),
        ("LAMBDA_TASK_ROOT", "/var/task".to_string()),
        ("LAMBDA_RUNTIME_DIR", "/var/runtime".to_string()),
        (
            "_HANDLER",
            invocation.handler.clone().unwrap_or_default(),
        ),
        (
            "AWS_EXECUTION_ENV",
            format!(
                "AWS_Lambda_{}",
                invocation.runtime.clone().unwrap_or_default()
            ),
        ),
    ];
    for (name, value) in reserved {
        environment.insert((*name).to_string(), value.clone());
    }
    // The variables a managed runtime's own sandbox sets beside those, each at the value
    // AWS documents for it. These are NOT reserved: the function's own variable of the same
    // name is its value, so a function that sets its time zone or its module path runs with
    // the one it set.
    let defaults: &[(&str, String)] = &[
        ("TZ", ":UTC".to_string()),
        ("LANG", "en_US.UTF-8".to_string()),
        (
            "LD_LIBRARY_PATH",
            "/var/lang/lib:/lib64:/usr/lib64:/var/runtime:/var/runtime/lib:/var/task:\
             /var/task/lib:/opt/lib"
                .to_string(),
        ),
    ];
    for (name, value) in defaults {
        environment
            .entry((*name).to_string())
            .or_insert_with(|| value.clone());
    }
    if engine_for(invocation.runtime.as_deref()) == Some(Engine::Node) {
        // "NODE_PATH – The Node.js library path": the layer directory for this runtime's
        // own major version, the layer's version-independent directory, and the runtime's
        // own modules, which is the search path a `require` of a bare name walks.
        let major = node::node_major(invocation.runtime.as_deref().unwrap_or_default());
        environment.entry("NODE_PATH".to_string()).or_insert_with(|| {
            format!(
                "/opt/nodejs/node{major}/node_modules:/opt/nodejs/node_modules:\
                 /var/runtime/node_modules:/var/runtime:/var/task"
            )
        });
    }
    environment
}

/// The log stream one run writes to: the date the run started, the version it ran, and the
/// invocation's own id — the form a CloudWatch Logs stream of a Lambda function has.
///
/// The date is the `YYYY/MM/DD` of the instant the run started, taken from the one
/// timestamp rendering this crate already has ([`super::iso8601`], whose calendar
/// arithmetic is `aws_smithy_types`'s) rather than computed here.
fn log_stream_name(version: &str, request_id: &str, started: SystemTime) -> String {
    let rendered = super::iso8601(started);
    let date = rendered.split('T').next().unwrap_or_default().replace('-', "/");
    format!(
        "{date}/[{version}]{}",
        request_id.replace('-', "")
    )
}

/// A run that is still going.
const RUN_RUNNING: u8 = 0;
/// A run whose thread has ended.
const RUN_FINISHED: u8 = 1;
/// A run the caller gave up on.
const RUN_ABANDONED: u8 = 2;

/// The run slots, shared by every invocation this process serves.
fn run_slots() -> &'static Arc<::tokio::sync::Semaphore> {
    static SLOTS: OnceLock<Arc<::tokio::sync::Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(::tokio::sync::Semaphore::new(MAX_CONCURRENT_RUNS)))
}

/// How many abandoned runs are outstanding.
fn abandoned() -> &'static AtomicUsize {
    static ABANDONED: AtomicUsize = AtomicUsize::new(0);
    &ABANDONED
}

/// How many runs are outstanding past their budget.
#[must_use]
pub fn abandoned_runs() -> usize {
    abandoned().load(Ordering::SeqCst)
}

/// Count one run as abandoned, for as long as its thread lasts.
fn claim_abandoned_run() {
    abandoned().fetch_add(1, Ordering::SeqCst);
}

/// Release the count of one abandoned run, when its thread finally ends.
fn release_abandoned_run() {
    let mut held = abandoned_runs();
    while held > 0 {
        match abandoned().compare_exchange(held, held - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return,
            Err(seen) => held = seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runtimes this emulator can run a package for, and which engine runs each — one
    /// row per class of `Runtime` value the model declares (source:
    /// `smithy_query(query="shape", shape="Runtime")`).
    #[test]
    fn a_runtime_resolves_to_the_engine_that_runs_it() {
        let cases: &[(Option<&str>, Option<Engine>)] = &[
            (Some("python3.12"), Some(Engine::Python)),
            (Some("python3.13"), Some(Engine::Python)),
            (Some("python3.9"), Some(Engine::Python)),
            (Some("nodejs22.x"), Some(Engine::Node)),
            (Some("nodejs20.x"), Some(Engine::Node)),
            (Some("nodejs18.x"), Some(Engine::Node)),
            (Some("nodejs"), Some(Engine::Node)),
            (Some("java21"), None),
            (Some("dotnet8"), None),
            (Some("ruby3.3"), None),
            (Some("provided.al2023"), None),
            (Some("Image"), None),
            (None, None),
        ];
        for (runtime, expected) in cases {
            let actual = engine_for(*runtime);
            assert_eq!(
                actual, *expected,
                "case {runtime:?}: expected the engine {expected:?}, got {actual:?}"
            );
            assert_eq!(
                runs_handler(*runtime),
                expected.is_some(),
                "case {runtime:?}: a runtime runs exactly when an engine here runs it"
            );
        }
    }

    /// A stored `MemorySize` is clamped into the range the model allows before it bounds a
    /// run, and never past this module's own per-run ceiling — one row per bound and one
    /// past each.
    #[test]
    fn a_stored_memory_size_is_clamped_before_it_bounds_a_run() {
        let cases: &[(i32, u64)] = &[
            (0, 128),
            (-7, 128),
            (128, 128),
            (256, 256),
            (512, 512),
            (10_240, MAX_RUN_MEMORY_BYTES / BYTES_PER_MB),
            (32_768, MAX_RUN_MEMORY_BYTES / BYTES_PER_MB),
            (100_000, MAX_RUN_MEMORY_BYTES / BYTES_PER_MB),
        ];
        for (configured, expected_mb) in cases {
            let actual = run_heap_bytes(*configured) as u64 / BYTES_PER_MB;
            assert_eq!(
                actual, *expected_mb,
                "case {configured} MB configured: expected a {expected_mb} MB ceiling, got \
                 {actual}"
            );
        }
    }

    /// The error document carries the handler's stack under the member the function's own
    /// runtime reports it under — one row per engine.
    #[test]
    fn the_error_document_names_the_stack_the_way_the_runtime_does() {
        let cases: &[(Engine, &str, &str)] = &[
            (Engine::Python, "stackTrace", "trace"),
            (Engine::Node, "trace", "stackTrace"),
        ];
        for (engine, expected, absent) in cases {
            let document = error_document(
                "boom",
                Some("Error"),
                "11111111-2222-3333-4444-555555555555",
                &["  at handler".to_string()],
                *engine,
            );
            let parsed: ::serde_json::Value =
                ::serde_json::from_slice(&document).expect("the error document is JSON");
            assert!(
                parsed.get(*expected).is_some_and(::serde_json::Value::is_array),
                "case {engine:?}: expected the stack under {expected}, got {parsed}"
            );
            assert!(
                parsed.get(*absent).is_none(),
                "case {engine:?}: {absent} is another runtime's member, got {parsed}"
            );
        }
    }

    /// A stored `Timeout` is clamped into the range the model allows before it bounds a
    /// run, and never past this module's own ceiling — one row per bound and one past each.
    #[test]
    fn a_stored_timeout_is_clamped_before_it_bounds_a_run() {
        let cases: &[(i32, u64)] = &[
            (0, 1),
            (-7, 1),
            (1, 1),
            (3, 3),
            (29, 29),
            (900, HOST_WAIT_CEILING.as_secs()),
            (100_000, HOST_WAIT_CEILING.as_secs()),
        ];
        for (configured, expected) in cases {
            let actual = run_budget(*configured).wall_clock.as_secs();
            assert_eq!(
                actual, *expected,
                "case {configured} s configured: expected a {expected} s run budget, got {actual}"
            );
        }
    }

    /// The log tail is the LAST 4 KB of the log, base64-encoded — one row per size class
    /// around that bound.
    #[test]
    fn the_log_tail_is_the_last_four_kilobytes() {
        for size in [0_usize, 1, LOG_TAIL_BYTES - 1, LOG_TAIL_BYTES, LOG_TAIL_BYTES + 1, 64 * 1024]
        {
            let invoked = Invoked {
                payload: Vec::new(),
                function_error: None,
                log: "L".repeat(size),
                log_events: Vec::new(),
                duration: Duration::ZERO,
                chunks: Vec::new(),
            };
            let decoded = ::base64::Engine::decode(
                &::base64::engine::general_purpose::STANDARD,
                invoked.log_tail(),
            )
            .expect("the tail is base64");
            let expected = size.min(LOG_TAIL_BYTES);
            assert_eq!(
                decoded.len(),
                expected,
                "case a {size}-byte log: expected a {expected}-byte tail, got {}",
                decoded.len()
            );
        }
    }

    /// The environment a run sees carries the function's own variables and the ones the
    /// runtime sets, and nothing of the host.
    #[test]
    fn the_run_environment_is_the_functions_own_plus_the_runtimes() {
        let mut variables = BTreeMap::new();
        variables.insert("SECRET".to_string(), "value".to_string());
        let invocation = Invocation {
            function_name: "probe".to_string(),
            executed_version: "7".to_string(),
            invoked_function_arn: "arn:aws:lambda:us-west-2:000000000000:function:probe:7"
                .to_string(),
            region: "us-west-2".to_string(),
            runtime: Some("python3.12".to_string()),
            handler: Some("lambda_function.lambda_handler".to_string()),
            variables,
            memory_size: 256,
            timeout: 3,
            architecture: "x86_64".to_string(),
            zip_bytes: None,
            layer_zips: Vec::new(),
            payload: Vec::new(),
            client_context: None,
            tenant_id: None,
            request_id: "11111111-2222-3333-4444-555555555555".to_string(),
            initialization_type: crate::state::execution_slot::INITIALIZATION_TYPE_ON_DEMAND,
            aws: None,
            logs: None,
        };
        let environment = runtime_environment(&invocation, SystemTime::UNIX_EPOCH);
        let cases: &[(&str, &str)] = &[
            ("SECRET", "value"),
            ("AWS_LAMBDA_FUNCTION_NAME", "probe"),
            ("AWS_LAMBDA_FUNCTION_VERSION", "7"),
            ("AWS_LAMBDA_FUNCTION_MEMORY_SIZE", "256"),
            ("AWS_LAMBDA_LOG_GROUP_NAME", "/aws/lambda/probe"),
            ("AWS_REGION", "us-west-2"),
            ("LAMBDA_TASK_ROOT", "/var/task"),
            ("_HANDLER", "lambda_function.lambda_handler"),
            ("AWS_EXECUTION_ENV", "AWS_Lambda_python3.12"),
            ("AWS_LAMBDA_INITIALIZATION_TYPE", "on-demand"),
        ];
        for (name, expected) in cases {
            let actual = environment.get(*name).map(String::as_str);
            assert_eq!(
                actual,
                Some(*expected),
                "case {name}: expected {expected:?}, got {actual:?}"
            );
        }
        for absent in ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "PATH", "HOME"] {
            assert!(
                !environment.contains_key(absent),
                "case {absent}: no host or credential variable reaches a run, got {:?}",
                environment.get(absent)
            );
        }
        let stream = environment
            .get("AWS_LAMBDA_LOG_STREAM_NAME")
            .map(String::as_str)
            .unwrap_or_default();
        assert_eq!(
            stream, "1970/01/01/[7]11111111222233334444555555555555",
            "expected the stream to name the date, the version and the invocation, got {stream:?}"
        );
    }
}

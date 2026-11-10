// SPDX-License-Identifier: CC0-1.0

//! What the RUN of a durable function's invocation does to the EXECUTION that invocation
//! started.
//!
//! A durable execution is the handle on one invocation of a durable function: `Invoke`
//! registers it and answers its `DurableExecutionArn` ("The ARN of the durable execution
//! that was started"), and `GetDurableExecution` reports its `Status`, which is "`RUNNING`,
//! `SUCCEEDED`, `FAILED`, `TIMED_OUT`, [or] `STOPPED`". So the terminal status is whatever
//! the INVOCATION came to, and this module is the one place that reads a run's outcome as
//! that status ([`run_outcome`]) and settles the stored execution on it ([`settle`]).
//!
//! It is called where the run HAPPENS rather than where the execution is read: an execution
//! whose invocation has finished is already in its terminal status when the next read
//! arrives, and no read invents a transition. The only other transitions an execution has
//! are the ones that were already owed — the `ExecutionTimeout` the clock owes
//! ([`super::DurableExecutionRecord::materialize`]) and the stop a caller makes
//! ([`super::DurableExecutionRecord::stop`]) — and neither of them is re-opened by a run
//! settling afterwards.

use super::*;

/// The members a runtime's error document carries, which is where the error of a handler
/// that FAILED is read from: the runtimes report `errorMessage` and `errorType`
//  (`super::execution`'s own `error_document` writes both).
const ERROR_MESSAGE_MEMBER: &str = "errorMessage";
/// See [`ERROR_MESSAGE_MEMBER`].
const ERROR_TYPE_MEMBER: &str = "errorType";

/// The outcome a durable execution is settled on for the run `run`.
///
/// A handler that RETURNED makes the execution succeed carrying the result it returned —
/// the model types `Result` as the JSON text of that result, so a payload that is not text
/// is reported as no result rather than as mangled text. A run that FAILED makes it fail
/// carrying the error the run reported: the `errorType` and `errorMessage` of the error
/// document the function's own runtime produced, or, for a run this emulator could not make
/// at all, the reason it could not — with no error type, because no function raised.
#[must_use]
pub fn run_outcome(run: &Result<Invoked, Unrunnable>) -> DurableOutcome {
    let invoked = match run {
        Ok(invoked) => invoked,
        Err(why) => {
            return DurableOutcome::Failed(ErrorObject {
                error_message: Some(unrunnable_reason(why)),
                ..ErrorObject::default()
            })
        }
    };
    if invoked.function_error.is_none() {
        return DurableOutcome::Succeeded(
            String::from_utf8(invoked.payload.clone())
                .ok()
                .filter(|result| !result.is_empty()),
        );
    }
    // The document the run FAILED with, wherever the run keeps it: for a response-streaming
    // run the payload is the bytes the handler had already written and the document is kept
    // beside them, so one failure is read the same way however it was produced
    // ([`Invoked::error_document`]).
    let document = invoked
        .error_document()
        .and_then(|document| ::serde_json::from_str::<::serde_json::Value>(&document).ok());
    let member = |name: &str| {
        document
            .as_ref()
            .and_then(|document| document.get(name))
            .and_then(::serde_json::Value::as_str)
            .map(str::to_string)
    };
    DurableOutcome::Failed(ErrorObject {
        error_message: member(ERROR_MESSAGE_MEMBER),
        error_type: member(ERROR_TYPE_MEMBER),
        ..ErrorObject::default()
    })
}

/// Settle the durable execution `name` of `function` on `outcome`, if the store still holds
/// it.
///
/// Nothing is created here: an execution the store no longer holds — a function deleted
/// while its invocation ran — is one there is nothing to settle.
pub fn settle(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    function: &str,
    name: &str,
    outcome: &DurableOutcome,
) {
    let mut store = state.lock();
    let now = SystemTime::now();
    let Some(execution) = store
        .functions
        .get_mut(function)
        .and_then(|function| function.durable_executions.get_mut(name))
    else {
        return;
    };
    // Whatever the clock ALREADY owed is applied first, so a run that finishes after its
    // execution's `ExecutionTimeout` fell due does not overwrite the timeout that was owed
    // at that earlier instant.
    execution.materialize(now);
    execution.settle(outcome, now);
}

/// Why the function an invocation addressed never ran, in one sentence — the same reading
/// the asynchronous queue records on its own entry, so a caller is told one thing about one
/// run however they read it. The subject is the EXECUTION, which is the work this run is.
fn unrunnable_reason(why: &Unrunnable) -> String {
    super::async_invocation::unrunnable_reason(why, THIS_EXECUTION)
}

/// What the reason sentences call the work a durable execution's run is.
const THIS_EXECUTION: &str = "this durable execution";

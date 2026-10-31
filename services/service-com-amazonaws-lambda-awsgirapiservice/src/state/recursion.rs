// SPDX-License-Identifier: CC0-1.0

//! Lambda's RECURSIVE-LOOP guardrail: what it counts, when it STOPS an invocation, and
//! what it meters when it does.
//!
//! `PutFunctionRecursionConfig` states the whole contract: "Lambda can detect certain
//! types of recursive loops shortly after they occur. When Lambda detects a recursive
//! loop and your function's recursive loop detection configuration is set to Terminate,
//! it stops your function being invoked and notifies you", while with `Allow` "Lambda
//! doesn't take any action when it detects your function being invoked as part of a
//! recursive loop". So the setting is only observable through what the guardrail DOES,
//! and this module is that behaviour:
//!
//! * COUNT. A loop is one request coming back round into the same function, so what is
//!   counted is per REQUEST CHAIN and per function: how many invocations of this function
//!   the chain a request belongs to has produced so far ([`FunctionRecord::count_chain_invocation`]).
//!   The chain a request belongs to is the trace id its tracing header carries — the id
//!   every hop of one chain carries, which is what a caller propagates when it calls one
//!   service from inside another ([`request_chain`]) — so a request that carries no
//!   tracing header belongs to no chain this emulator can place, and no loop can be
//!   detected from it. Counting is Lambda's own bookkeeping and happens whatever the
//!   function's configuration says, because the configuration decides only what is done
//!   once a loop IS detected.
//! * STOP. Past [`MAX_REQUEST_CHAIN_INVOCATIONS`] invocations of one function in one
//!   chain, Lambda has detected the loop. A function configured `Terminate` then has the
//!   invocation STOPPED — nothing is queued and nothing runs — and one configured `Allow`
//!   has it served exactly as any other ([`guardrail`]).
//! * METER. Every invocation the guardrail stops is one datapoint of
//!   [`super::metrics::RECURSIVE_INVOCATIONS_DROPPED_METRIC`] for that function, which is
//!   how a client learns the guardrail fired at all
//!   ([`publish_recursive_invocation_dropped`]). An invocation nothing stopped meters
//!   nothing, so a function whose configuration is `Allow` has no such series at all
//!   rather than a zero-valued one.
//!
//! The counts are the only thing kept here, on the function record itself, and they are
//! BOUNDED from both sides: a function tracks at most [`MAX_TRACKED_REQUEST_CHAINS`]
//! chains, and a chain whose last hop is further back than [`REQUEST_CHAIN_IDLE_WINDOW`]
//! is forgotten, because no invocation of it can still be in flight to close the loop.
//! The chain id itself is a caller-supplied header, so it is admitted only as the
//! fixed-width trace id `service_common`'s one reading of a tracing header recognises.

use std::time::{Duration, SystemTime};

use super::metrics::{cloudwatch_state, FunctionMetrics};
use super::FunctionRecord;

/// The `RecursiveLoop` value that has Lambda stop a loop it detects: "If you set your
/// function's recursive loop detection configuration to Terminate, Lambda stops your
/// function being invoked and notifies you when it detects your function being invoked as
/// part of a recursive loop" (`PutFunctionRecursionConfig$RecursiveLoop`). It is also the
/// value a function carries before anyone sets one (see [`super::DEFAULT_RECURSIVE_LOOP`]).
pub const RECURSIVE_LOOP_TERMINATE: &str = "Terminate";

/// The other `RecursiveLoop` value, from the model's enum (`Allow | Terminate`): "If you
/// set your function's recursive loop detection configuration to Allow, Lambda doesn't
/// take any action when it detects your function being invoked as part of a recursive
/// loop."
pub const RECURSIVE_LOOP_ALLOW: &str = "Allow";

/// How many invocations of one function ONE request chain may produce before Lambda has
/// detected a recursive loop.
///
/// Lambda's recursive-loop detection counts how many times a request has been processed
/// and stops the chain once that count passes 16, which is the number AWS publishes for
/// the guardrail. So the first sixteen hops of a chain are served and the seventeenth is
/// the one a `Terminate` function has stopped.
pub const MAX_REQUEST_CHAIN_INVOCATIONS: u32 = 16;

/// The request header a tracing header arrives on, as X-Ray spells it.
pub const TRACE_HEADER: &str = "x-amzn-trace-id";

/// How many request chains ONE function keeps a count for.
///
/// An EMULATOR-CHOSEN ceiling, like [`super::MAX_QUEUED_ASYNC_INVOCATIONS`]: the counts
/// are held in memory and a caller chooses the chain id, so past this point the chain
/// whose last hop is oldest makes room for the newest rather than the store growing
/// without bound. It is far above [`MAX_REQUEST_CHAIN_INVOCATIONS`], so the chains of a
/// function being driven by many callers at once are each still counted to the limit.
pub const MAX_TRACKED_REQUEST_CHAINS: usize = 64;

/// How long after a chain's last hop the count kept for it is forgotten.
///
/// One invocation of a function can run for at most [`super::MAX_TIMEOUT_SECONDS`] ("The
/// maximum allowed value is 900 seconds"), so an invocation arriving longer than that
/// after the previous hop of the same chain cannot have been made by it: the chain it
/// belongs to is a new one that reuses the id, and it is counted from the start.
pub const REQUEST_CHAIN_IDLE_WINDOW: Duration = Duration::from_secs(900);

/// Lambda's sentence for an invocation its recursive-loop guardrail stopped, which is what
/// the modelled `RecursiveInvocationException` documents it as: this is the whole of what
/// the client is told, because the loop is a property of the chain rather than of the one
/// request that was refused.
pub const RECURSIVE_INVOCATION_MESSAGE: &str =
    "Lambda has detected your function being invoked in a recursive loop with other Amazon \
     Web Services resources and stopped your function's invocation.";

/// What ONE function has seen of ONE request chain.
#[derive(Debug, Clone)]
pub struct RequestChainRecord {
    /// The chain's trace id, as [`request_chain`] read it off the request.
    pub root: String,
    /// How many invocations of this function that chain has produced.
    pub invocations: u32,
    /// When the last of them arrived; see [`REQUEST_CHAIN_IDLE_WINDOW`].
    pub last_seen: SystemTime,
}

/// What the guardrail decided about one invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guardrail {
    /// No loop was detected, or the function's configuration is
    /// [`RECURSIVE_LOOP_ALLOW`] and Lambda "doesn't take any action": the invocation is
    /// served exactly as any other.
    Proceed,
    /// Lambda detected the loop and the function's configuration is
    /// [`RECURSIVE_LOOP_TERMINATE`], so this invocation is STOPPED: nothing is queued and
    /// nothing runs, and the drop is metered.
    Stopped,
}

/// The request CHAIN this request belongs to: the trace id of the tracing header it
/// carries, or `None` when it carries none.
///
/// A caller that invokes a function from inside another call propagates that header, which
/// is what makes the hops of one chain recognisable as one chain. Nothing is invented for a
/// request that carries no header: such a request starts and ends a chain of its own, and
/// no loop can be detected from it.
#[must_use]
pub fn request_chain(ctx: &smithy_server_runtime::OperationContext<'_>) -> Option<String> {
    ctx.parts()
        .headers
        .get(TRACE_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(::service_common::xray_trace::trace_root)
        .map(str::to_string)
}

/// Count one invocation of `record` arriving at `now` as part of the chain `chain` names,
/// and report whether Lambda's guardrail STOPS it.
///
/// The count is kept whatever the function's configuration is, because the configuration
/// decides only what Lambda does once it has detected the loop; the decision then reads
/// the configuration the function holds.
pub fn guardrail(
    record: &mut FunctionRecord,
    chain: Option<&str>,
    now: SystemTime,
) -> Guardrail {
    let Some(root) = chain else {
        return Guardrail::Proceed;
    };
    let invocations = record.count_chain_invocation(root, now);
    if invocations > MAX_REQUEST_CHAIN_INVOCATIONS
        && record.recursive_loop == RECURSIVE_LOOP_TERMINATE
    {
        Guardrail::Stopped
    } else {
        Guardrail::Proceed
    }
}

impl FunctionRecord {
    /// Count one invocation of this function, arriving at `now`, against the request chain
    /// `root` identifies, and answer how many invocations of it that chain has now
    /// produced.
    ///
    /// A chain whose last hop is further back than [`REQUEST_CHAIN_IDLE_WINDOW`] is
    /// forgotten first, so a later request that reuses the id is counted from the start
    /// rather than inheriting a count no live chain holds.
    pub fn count_chain_invocation(&mut self, root: &str, now: SystemTime) -> u32 {
        self.request_chains.retain(|chain| {
            chain
                .last_seen
                .checked_add(REQUEST_CHAIN_IDLE_WINDOW)
                .is_none_or(|until| until > now)
        });
        if let Some(chain) = self
            .request_chains
            .iter_mut()
            .find(|chain| chain.root == root)
        {
            chain.invocations = chain.invocations.saturating_add(1);
            chain.last_seen = now;
            return chain.invocations;
        }
        self.request_chains.push_back(RequestChainRecord {
            root: root.to_string(),
            invocations: 1,
            last_seen: now,
        });
        while self.request_chains.len() > MAX_TRACKED_REQUEST_CHAINS {
            self.request_chains.pop_front();
        }
        1
    }
}

/// Publish the ONE datapoint an invocation the guardrail stopped is metered by: a count of
/// 1 on the [`super::metrics::RECURSIVE_INVOCATIONS_DROPPED_METRIC`] series of the function
/// `function_name` names, so a reader summing a window over that series gets the number of
/// invocations the guardrail dropped in it.
///
/// It goes into the CloudWatch store of the SAME engine ([`super::metrics`]), which is the
/// store `GetMetricStatistics`, `GetMetricData` and `ListMetrics` answer from. An engine
/// carrying no CloudWatch has nowhere to publish to, so nothing is. Call it with no store
/// lock held: it takes the CloudWatch store's own.
pub fn publish_recursive_invocation_dropped<Eng: Send + Sync + 'static>(
    engine: &Eng,
    function_name: &str,
) {
    FunctionMetrics::new(cloudwatch_state(engine))
        .publish_recursive_invocations_dropped(function_name);
}

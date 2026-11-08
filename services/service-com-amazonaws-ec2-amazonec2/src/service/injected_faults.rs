// SPDX-License-Identifier: CC0-1.0

//! The fault a RUNNING fault-injection experiment imposes on this service's API — the
//! decision taken ahead of every operation, before any handler reaches the store.
//!
//! ## The rule this implements
//!
//! A fault-injection experiment whose action injects an API error makes the calls it names
//! FAIL for as long as that action runs: the roles its target resolved to, the operations of
//! this service it listed, the share of the calls it asked for, and nothing else — another
//! principal, another operation, another service and an instant outside the action's own
//! window are all served exactly as before. The failure is this service's answer, because it
//! is this service's API the call reached, which is why the decision is taken here rather
//! than by the service that runs the experiment.
//!
//! The leases are read out of the shared registry in this same engine
//! ([`::service_common::injected_api_faults`]), whose only writer is the service that runs
//! the experiments; nothing here writes to it, and nothing here reaches the experiment's own
//! store.
//!
//! ## Where it falls in the sequence
//!
//! AFTER the authorization gate beside it ([`super::authorization`]): a call the caller may
//! not make at all is refused as such, and an injected fault fails a call the caller could
//! otherwise have made. Both gates resolve the request's principal through the SAME
//! resolution ([`super::authorization::request_principals`]), so the role a policy decision
//! is taken for and the role a fault is injected against are one answer.
//!
//! ## What it costs a request that is not affected
//!
//! One lock and one `is_empty` while no experiment is injecting anything, which is the state
//! the emulator is in unless a client started such an experiment: the principal is resolved
//! only once a lease exists to be about it. With leases held, the walk is bounded by the
//! registry's own ceilings ([`::service_common::injected_api_faults::MAX_LEASES`]).

use std::time::SystemTime;

use ::service_common::injected_api_faults::{call_draw, InjectedApiFaults, InjectedFault};

/// The emulator's injected-fault registry in the shared engine.
///
/// A SHARED type ([`InjectedApiFaults`]) and not a field of this service's own state: the
/// service that runs the experiment and this one, whose API the experiment's fault affects,
/// have to reach ONE registry rather than two copies of it. `None` for an engine that is not
/// the shared one, where no experiment can have published anything.
pub fn injected_api_faults<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<InjectedApiFaults>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<InjectedApiFaults>)
}

/// The fault a running experiment injects into this request, or `None` when none does.
///
/// `operation` is the operation's own name, which is the routing key the request named.
///
/// The instant is NOW, because whether a fault is live is a fact about when the call arrived:
/// an action's window opens and closes on the run's own clock, whether or not anybody polled
/// the experiment. The share is drawn per CALL ([`call_draw`]), so a partial injection
/// affects a share of the calls rather than a fixed set of them, and a full one affects every
/// one.
pub fn injected_fault<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
) -> Option<InjectedFault> {
    let registry = injected_api_faults(engine)?;
    // Nothing is injected anywhere, which is the ordinary state: the principal is not
    // resolved at all for a request no lease could be about. The guard is released before
    // the principal is resolved, because resolving it reads two OTHER services' stores.
    if registry.lock().leases().is_empty() {
        return None;
    }
    let principals = super::authorization::request_principals(engine, ctx);
    if principals.is_empty() {
        return None;
    }
    // One acquisition decides: the lease set the answer is read from is the one this guard
    // holds, so a lease published while the answer is being read cannot be half-applied.
    let held = registry.lock();
    held.fault_for(
        super::authorization::ACTION_PREFIX,
        operation,
        &principals,
        SystemTime::now(),
        call_draw(),
    )
    .cloned()
}

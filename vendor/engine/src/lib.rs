// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Shared engine crate — `InMemoryEngine` with a type-map for per-service state.
//!
//! Each service crate defines its own state struct and accesses it via
//! `engine.state::<MyState>()`. The returned `ServiceState<T>` wraps
//! `Arc<Mutex<T>>` and can be held across await points or passed around freely.
//!
//! # Execution and native listeners
//!
//! Both are in scope: serve them **in-process, faithfully** — or answer honestly
//! where you cannot. Build them from maintained crates: an interpreter or engine
//! for a language, a protocol server, a storage engine. Containers and external
//! processes are not available to a generated emulator yet; everything else the real
//! service's behaviour needs is.
//!
//! - **User-provided code** (Lambda `Invoke`, ECS `RunTask`, ...) and any query or
//!   expression language the service evaluates run under that language's documented
//!   semantics, so the result is what the real service returns for ANY code of that
//!   runtime, not only for the code a test happens to use.
//! - **A native listener protocol** (a SQL database, a key-value cache, a message
//!   broker, a search engine) may be served by the emulator itself: run the protocol
//!   server and its engine in-process, and hand a connection handler to
//!   `engine.listeners().tcp(handler)` (or `.http(handler, policy)` for an HTTP
//!   resource). A protocol that negotiates encryption in-band (PostgreSQL
//!   `SSLRequest`, MySQL greeting-first) uses `.tcp_plaintext(handler)` and, to
//!   upgrade, the core-owned `.tls_upgrader()`. The service never binds a socket, chooses a port, or configures TLS
//!   itself — the core owns the socket, TLS and bind interface and returns the host
//!   and port to report in the control plane's endpoint fields, so a client connects
//!   with its normal driver. Report an HTTP endpoint as `listener.url()` (or
//!   `listener.authority()` for a `host:port` field without a scheme), never a
//!   `format!` of `host()` and `port()`, which breaks on a wildcard or IPv6 bind. A
//!   URL a caller fetches later (one the modeled operation returns presigned) comes
//!   from `ctx.url_signer()`, and the base URL of an output member that carries a
//!   resource's address from `ctx.endpoint()`, not from a listener of the service's own.
//!
//! **Never fabricate a result:** do not echo the input back as the output, do not
//! pattern-match source text, and do not hand-write an evaluator or engine for a
//! subset of a language — a subset sized to the inputs seen so far answers every
//! other input wrongly while reporting success.
//!
//! Where the engine available differs from the real one, answer what it cannot do
//! faithfully with the engine's own error, the scaffold's 501 or the modelled error the
//! service declares, and keep everything around it real: resolving the resource,
//! versions, permissions, status codes, endpoints, logs and asynchronous delivery are
//! the service's own behaviour, and they are
//! implemented whether or not the code can run. Never `todo!()`/`panic!()` in a
//! handler — the server contains a panic as a `500 InternalFailure`, an answer the
//! real service never gives.
//!
//! Two invariants, because in-process execution has **no isolation boundary**:
//!
//! 1. **Bound it.** Wrap each execution in a wall-clock timeout
//!    (`tokio::time::timeout`), give a synchronous interpreter an instruction or step
//!    budget (a timeout cannot preempt a CPU-bound loop running on the request
//!    thread), and cap any captured output, or code that loops forever takes the
//!    whole emulator down rather than just its own request.
//! 2. **Contain it.** Code and engines reach only the emulator's own endpoints and
//!    state: no outbound connection to any other host, and no host file outside the
//!    emulator's own state. A filesystem a runtime exposes is one the emulator owns,
//!    and a service call the code makes is answered by this engine.
#![forbid(unsafe_code)]

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub use emulator_server::EMULATOR_ACCOUNT_ID;
pub use emulator_server::{extract_akid, emulator_account_id, emulator_arn};

/// Handle to a service's state. Clone-cheap (Arc inside). Call `.lock()` to access.
pub struct ServiceState<T>(Arc<Mutex<T>>);

// `Clone` is implemented by hand rather than derived: `#[derive(Clone)]` on a
// generic tuple struct adds a `T: Clone` bound, which would make
// `ServiceState<MyState>` cloneable ONLY when the service's state struct is
// itself `Clone`. Cloning the handle just clones the inner `Arc`, so no bound on
// `T` is warranted — and requiring one would force every service author to
// derive `Clone` on state that is never cloned (or, more likely, to give up and
// stop passing the handle around).
impl<T> Clone for ServiceState<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> ServiceState<T> {
    /// Lock the state for exclusive access.
    ///
    /// Recovers from a poisoned lock rather than propagating the panic. A state
    /// lock is poisoned when a handler panics while holding the guard; a
    /// long-lived emulator must stay available to every later request, so a
    /// single bad request must not wedge the service into returning errors until
    /// the process restarts. The guarded value is recovered with
    /// [`PoisonError::into_inner`]: the state may reflect a partial write from the
    /// panicking handler, which the next handler overwrites as it serves its own
    /// request, so the recovered state is sound for the emulator's purpose.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The shared engine type. One instance is built per emulator and shared across
/// every operation, so all requests observe the same store; reach a service's
/// state through `state::<T>()`, never by constructing an engine, or the state a
/// request writes is dropped when it returns.
pub struct InMemoryEngine {
    states: Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    listeners: Arc<emulator_server::Listeners>,
}

impl InMemoryEngine {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
            listeners: Arc::new(emulator_server::Listeners::new()),
        }
    }

    /// The shared listener handle a service uses to open a resource's native
    /// (`tcp`) or HTTP (`http`) listener — e.g. the Postgres port an RDS
    /// instance hosts, or an API Gateway invoke endpoint. The core owns the
    /// socket, TLS and (for HTTP) the browser checks; the service writes only
    /// the protocol handler. The returned handle is cheap to clone.
    pub fn listeners(&self) -> Arc<emulator_server::Listeners> {
        Arc::clone(&self.listeners)
    }

    /// Get a handle to a service's state, initializing with `Default` if absent.
    ///
    /// Construct-and-insert is ATOMIC: it resolves through a single
    /// `entry().or_insert_with(...)` while holding one lock, so concurrent
    /// first-touch of the same `T` cannot have two callers each build a value and
    /// one silently discard the other's. A non-atomic check-then-insert here
    /// (read-lock miss, drop, write-lock, insert) loses that race and hands two
    /// request handlers *different* state objects for the same service — a
    /// corruption that only shows up under concurrent load and looks like
    /// randomly-vanishing writes.
    pub fn state<T: Default + Send + 'static>(&self) -> ServiceState<T> {
        let mut map = self.states.lock().unwrap_or_else(PoisonError::into_inner);
        let arc = map
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Arc::new(Mutex::new(T::default()))))
            .downcast_ref::<Arc<Mutex<T>>>()
            .expect("type mismatch in engine state map")
            .clone();
        ServiceState(arc)
    }
}

impl Default for InMemoryEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for InMemoryEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryEngine").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct AlphaState {
        hits: u32,
    }

    #[derive(Default)]
    struct BetaState {
        label: String,
    }

    /// Two handles to the same state type observe the SAME underlying value —
    /// this is the whole contract the router relies on when several operation
    /// handlers in one service each call `engine.state::<T>()`.
    #[test]
    fn repeated_state_access_returns_the_same_instance() {
        let engine = InMemoryEngine::new();
        engine.state::<AlphaState>().lock().hits += 1;
        engine.state::<AlphaState>().lock().hits += 1;
        assert_eq!(engine.state::<AlphaState>().lock().hits, 2);
    }

    /// Distinct state types are isolated: one service's writes are invisible to
    /// another's, even though both live in the same engine.
    #[test]
    fn distinct_state_types_do_not_alias() {
        let engine = InMemoryEngine::new();
        engine.state::<AlphaState>().lock().hits = 7;
        engine.state::<BetaState>().lock().label = "beta".to_string();
        assert_eq!(engine.state::<AlphaState>().lock().hits, 7);
        assert_eq!(engine.state::<BetaState>().lock().label, "beta");
    }

    /// A fresh engine starts from `Default`, and separate engines never share
    /// state.
    #[test]
    fn separate_engines_have_independent_state() {
        let a = InMemoryEngine::new();
        let b = InMemoryEngine::default();
        a.state::<AlphaState>().lock().hits = 5;
        assert_eq!(a.state::<AlphaState>().lock().hits, 5);
        assert_eq!(b.state::<AlphaState>().lock().hits, 0);
    }

    /// `ServiceState` is a handle, not a copy: cloning it (or holding it across an
    /// await point) still refers to the one value in the engine.
    ///
    /// Note the state struct here is deliberately NOT `Clone` — cloning the handle
    /// must not require the service's state to be cloneable.
    #[test]
    fn service_state_handle_is_shared_not_copied() {
        let engine = InMemoryEngine::new();
        let handle = engine.state::<AlphaState>();
        let cloned = handle.clone();
        handle.lock().hits = 3;
        assert_eq!(cloned.lock().hits, 3);
        assert_eq!(engine.state::<AlphaState>().lock().hits, 3);
    }

    /// THE RACE: many threads first-touch the same state type simultaneously.
    /// Every increment must survive. With a non-atomic check-then-insert, threads
    /// that all miss the read lock each construct a fresh `T::default()` and the
    /// last writer wins, so the total comes out short.
    #[test]
    fn concurrent_first_touch_does_not_lose_writes() {
        use std::sync::Barrier;

        const THREADS: u32 = 32;
        let engine = Arc::new(InMemoryEngine::new());
        let barrier = Arc::new(Barrier::new(THREADS as usize));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let engine = Arc::clone(&engine);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    // Maximise the chance every thread races on the very first access.
                    barrier.wait();
                    engine.state::<AlphaState>().lock().hits += 1;
                })
            })
            .collect();

        for h in handles {
            h.join().expect("worker thread panicked");
        }

        assert_eq!(
            engine.state::<AlphaState>().lock().hits,
            THREADS,
            "a concurrent first-touch dropped writes: state<T>() is not atomic"
        );
    }

    /// A panic while the state lock is held must NOT wedge the service: the lock
    /// is poisoned by the unwinding handler, and every later request still
    /// acquires it and sees the committed state. Without poison recovery the next
    /// `lock()` panics, so a single bad request turns a long-lived emulator into a
    /// permanent 500 until the process restarts.
    #[test]
    fn panic_while_holding_state_lock_does_not_wedge_later_requests() {
        let engine = Arc::new(InMemoryEngine::new());

        // One request handler commits a write, then panics while still holding
        // the guard — poisoning the state lock.
        let poisoner = {
            let engine = Arc::clone(&engine);
            std::thread::spawn(move || {
                let state = engine.state::<AlphaState>();
                let mut guard = state.lock();
                guard.hits = 11;
                panic!("handler panicked while holding the state lock");
            })
        };
        assert!(
            poisoner.join().is_err(),
            "the poisoning handler was expected to panic"
        );

        // The next request still locks the SAME state and sees the committed
        // write — the lock recovered from poisoning rather than propagating it.
        assert_eq!(
            engine.state::<AlphaState>().lock().hits,
            11,
            "a poisoned state lock wedged the service: lock() did not recover"
        );

        // And the recovered lock is fully usable for subsequent writes.
        engine.state::<AlphaState>().lock().hits += 1;
        assert_eq!(engine.state::<AlphaState>().lock().hits, 12);
    }

    /// The engine's internal type-map lock recovers from poisoning too, so a
    /// panic that poisons it does not wedge `state()` for every later
    /// request.
    #[test]
    fn engine_recovers_after_type_map_lock_poisoned() {
        let engine = Arc::new(InMemoryEngine::new());
        engine.state::<AlphaState>().lock().hits = 4;

        // Poison the type-map lock itself: panic while holding the guard that
        // `state()` takes.
        let poisoner = {
            let engine = Arc::clone(&engine);
            std::thread::spawn(move || {
                let _guard = engine.states.lock().unwrap();
                panic!("panicked while holding the engine type-map lock");
            })
        };
        assert!(poisoner.join().is_err());

        // `state()` still works and sees the earlier write.
        assert_eq!(engine.state::<AlphaState>().lock().hits, 4);
    }

    /// The engine is `Debug` without exposing its contents (it holds `dyn Any`,
    /// which cannot be formatted) — the router logs it as part of larger structs.
    #[test]
    fn debug_is_opaque_but_present() {
        let rendered = format!("{:?}", InMemoryEngine::new());
        assert!(rendered.contains("InMemoryEngine"), "got: {rendered}");
    }
}

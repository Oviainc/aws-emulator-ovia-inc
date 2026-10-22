// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-neptunedata-amazonneptunedataplane service.
//!
//! Two kinds of record live here, and every implemented operation computes its answer
//! from them:
//!
//! * [`EngineStatusRecord`] — the one DB instance `GetEngineStatus` reports on. It is
//!   built ONCE, when this state is first reached (the instant this emulator's engine
//!   "started"), so every later read answers the same `startTime` instead of a fresh
//!   clock reading.
//! * [`GremlinQueryRecord`] — one per Gremlin query the engine has accepted, keyed by
//!   query id. `ListGremlinQueries` and `GetGremlinQueryStatus` report on these and
//!   `CancelGremlinQuery` mutates them.
//!
//! Nothing in this crate admits a query record yet: the Gremlin execution endpoints
//! (`ExecuteGremlinQuery`, `ExecuteGremlinExplainQuery`, `ExecuteGremlinProfileQuery`)
//! still answer HTTP 501, because this emulator has no in-process engine that can
//! evaluate a TinkerPop traversal — and a query record, an evaluation time or a result
//! for a traversal nothing ran would be a fabrication. The registry is therefore
//! honestly empty until a real Gremlin engine is wired in, and the status family above
//! reports exactly what it holds.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    FailureByQueryException, GremlinQueryStatus, InternalFailureException,
    InvalidParameterException, QueryEvalStats,
};

/// `GetEngineStatusOutput$status` for an instance that is not experiencing problems:
/// the member's own documentation says "Set to `healthy` if the instance is not
/// experiencing problems".
pub const ENGINE_STATUS_HEALTHY: &str = "healthy";

/// `GetEngineStatusOutput$role` for the primary instance: the member's documentation
/// says "Set to `reader` if the instance is a read-replica, or to `writer` if the
/// instance is the primary instance". This emulator serves one instance, and it accepts
/// writes, so it is the primary.
pub const ENGINE_ROLE_WRITER: &str = "writer";

/// `GetEngineStatusOutput$dfeQueryEngine`'s documented default — the member's own
/// documentation names `viaQueryHint` "(the default)".
pub const DFE_QUERY_ENGINE_DEFAULT: &str = "viaQueryHint";

/// The Neptune engine version this emulator reports. A released Neptune engine version,
/// carried on the record so one place states it; unprefixed, because the member's
/// documentation reserves the `Patch-` prefix for a manually patched engine.
pub const DB_ENGINE_VERSION: &str = "1.3.2.0";

/// The Apache TinkerPop version `GetEngineStatusOutput$gremlin` reports.
pub const GREMLIN_LANGUAGE_VERSION: &str = "tinkerpop-3.6.2";

/// The SPARQL version `GetEngineStatusOutput$sparql` reports.
pub const SPARQL_LANGUAGE_VERSION: &str = "sparql-1.1";

/// The openCypher version `GetEngineStatusOutput$opencypher` reports.
pub const OPENCYPHER_LANGUAGE_VERSION: &str = "Neptune-9.0.20190305-1.0";

/// The cluster query timeout, in milliseconds, reported under `settings` — the member's
/// own documentation names `clusterQueryTimeoutInMs` as what `settings` carries, and
/// 120000 ms is the documented default of the `neptune_query_timeout` DB cluster
/// parameter it reflects.
pub const CLUSTER_QUERY_TIMEOUT_MS: &str = "120000";

/// The settings key [`CLUSTER_QUERY_TIMEOUT_MS`] is reported under.
pub const CLUSTER_QUERY_TIMEOUT_SETTING: &str = "clusterQueryTimeoutInMs";

/// `CancelGremlinQueryOutput$status` for a cancellation the engine accepted. It reports
/// the status of the cancellation request itself, not of the query.
pub const CANCELLATION_ACCEPTED: &str = "200 OK";

/// The engine state `GetEngineStatus` reports: one record, built at state init.
#[derive(Debug, Clone)]
pub struct EngineStatusRecord {
    /// `healthy` for an instance with no problem to report.
    pub status: String,
    /// When this emulator's engine process started. Fixed at state init: a later read
    /// reports this instant, never a fresh clock reading.
    pub start_time: SystemTime,
    pub db_engine_version: String,
    pub role: String,
    pub dfe_query_engine: String,
    pub gremlin_version: String,
    pub sparql_version: String,
    pub opencypher_version: String,
    /// The Lab Mode parameters this engine carries, by parameter name.
    pub lab_mode: BTreeMap<String, String>,
    /// How many transactions are being rolled back right now.
    pub rolling_back_trx_count: i32,
    /// When the earliest transaction being rolled back started. `None` while nothing is
    /// rolling back, which is what keeps the member off the wire.
    pub rolling_back_trx_earliest_start_time: Option<SystemTime>,
    /// Each optional engine feature's status, by feature name.
    pub features: BTreeMap<String, String>,
    /// The cluster settings this engine runs with, by setting name.
    pub settings: BTreeMap<String, String>,
}

impl Default for EngineStatusRecord {
    fn default() -> Self {
        Self {
            status: ENGINE_STATUS_HEALTHY.to_string(),
            start_time: SystemTime::now(),
            db_engine_version: DB_ENGINE_VERSION.to_string(),
            role: ENGINE_ROLE_WRITER.to_string(),
            dfe_query_engine: DFE_QUERY_ENGINE_DEFAULT.to_string(),
            gremlin_version: GREMLIN_LANGUAGE_VERSION.to_string(),
            sparql_version: SPARQL_LANGUAGE_VERSION.to_string(),
            opencypher_version: OPENCYPHER_LANGUAGE_VERSION.to_string(),
            // Neptune's two engine-level Lab Mode parameters at their documented
            // defaults: `ObjectIndex` is off unless a cluster turns it on, and
            // read/write conflict detection is on.
            lab_mode: BTreeMap::from([
                ("ObjectIndex".to_string(), "disabled".to_string()),
                ("ReadWriteConflictDetection".to_string(), "enabled".to_string()),
            ]),
            // No transaction is rolling back in a freshly started engine, so the
            // earliest-rollback instant has nothing to report.
            rolling_back_trx_count: 0,
            rolling_back_trx_earliest_start_time: None,
            // Every optional engine feature is off, and each entry below says something
            // true of this emulator: there is no DFE result cache, the stream endpoints
            // are not implemented, and neither log is collected.
            features: BTreeMap::from([
                ("AuditLog".to_string(), "disabled".to_string()),
                ("ResultCache".to_string(), "disabled".to_string()),
                ("SlowQueryLogs".to_string(), "disabled".to_string()),
                ("Streams".to_string(), "disabled".to_string()),
            ]),
            settings: BTreeMap::from([(
                CLUSTER_QUERY_TIMEOUT_SETTING.to_string(),
                CLUSTER_QUERY_TIMEOUT_MS.to_string(),
            )]),
        }
    }
}

/// How far a Gremlin query the engine accepted has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryState {
    /// Accepted and queued, not evaluating yet.
    Waiting,
    /// Evaluating now.
    Running,
    /// Finished on its own.
    Done,
    /// Stopped by `CancelGremlinQuery`.
    Cancelled,
}

impl QueryState {
    /// Whether the engine still owes this query an answer — accepted but not completed,
    /// which is what `ListGremlinQueriesOutput$acceptedQueryCount` counts ("accepted but
    /// not yet completed, including queries in the queue").
    pub fn is_accepted(self) -> bool {
        matches!(self, Self::Waiting | Self::Running)
    }

    /// Whether the query is evaluating right now, which is what
    /// `ListGremlinQueriesOutput$runningQueryCount` counts.
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether the query has finished, one way or the other. A finished query cannot be
    /// cancelled and is no longer one of "the current queries" the list reports.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

/// One Gremlin query the engine accepted, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct GremlinQueryRecord {
    /// The id the query is reached by — `GetGremlinQueryStatus`'s and
    /// `CancelGremlinQuery`'s `queryId` label, and the key this record is stored under.
    pub query_id: String,
    /// The query exactly as it was submitted.
    pub query_string: String,
    /// The `serializer` (the submitting request's `accept` header), if it named one. Set
    /// by whatever admits the query — `ExecuteGremlinQuery`, which is still a 501 — so no
    /// path in this crate writes it yet.
    pub serializer: Option<String>,
    /// When the engine accepted the query.
    pub accepted_at: SystemTime,
    /// When evaluation began; `None` while the query is still queued.
    pub started_at: Option<SystemTime>,
    /// When evaluation ended; `None` while the query has not finished.
    pub finished_at: Option<SystemTime>,
    pub state: QueryState,
    /// `QueryEvalStats$cancelled`: whether this query was cancelled.
    pub cancelled: bool,
    /// How many subqueries this query has.
    pub subqueries: u64,
}

impl GremlinQueryRecord {
    /// A record for a query accepted at `accepted_at` and still queued.
    pub fn accepted(
        query_id: impl Into<String>,
        query_string: impl Into<String>,
        accepted_at: SystemTime,
    ) -> Self {
        Self {
            query_id: query_id.into(),
            query_string: query_string.into(),
            serializer: None,
            accepted_at,
            started_at: None,
            finished_at: None,
            state: QueryState::Waiting,
            cancelled: false,
            subqueries: 0,
        }
    }

    /// Milliseconds from `from` to `to`, saturated into the modelled `Integer`. A `to`
    /// before `from` is zero rather than negative: both bounds are instants this record
    /// stores, and a negative wait is not a value the member can carry.
    fn millis_between(from: SystemTime, to: SystemTime) -> i32 {
        let millis = to.duration_since(from).map_or(0, |d| d.as_millis());
        i32::try_from(millis.min(i32::MAX as u128)).unwrap_or(i32::MAX)
    }

    /// `QueryEvalStats$waited`: how long the query waited before evaluation began, or
    /// how long it has been waiting so far while it is still queued.
    ///
    /// A query cancelled while still queued never started, but its wait DID end — at the
    /// instant it finished — so the span closes there rather than following the clock for
    /// ever.
    pub fn waited_ms(&self, now: SystemTime) -> i32 {
        let waited_until = self.started_at.or(self.finished_at).unwrap_or(now);
        Self::millis_between(self.accepted_at, waited_until)
    }

    /// `QueryEvalStats$elapsed`: how long the query has been evaluating. Zero while it
    /// has not started, and frozen at the instant it finished.
    pub fn elapsed_ms(&self, now: SystemTime) -> i32 {
        match self.started_at {
            None => 0,
            Some(started) => Self::millis_between(started, self.finished_at.unwrap_or(now)),
        }
    }

    /// This record's `QueryEvalStats`, as of `now`.
    pub fn eval_stats(&self, now: SystemTime) -> QueryEvalStats {
        QueryEvalStats {
            waited: Some(self.waited_ms(now)),
            elapsed: Some(self.elapsed_ms(now)),
            cancelled: Some(self.cancelled),
            subqueries: Some(::aws_smithy_types::Document::Number(
                ::aws_smithy_types::Number::PosInt(self.subqueries),
            )),
        }
    }

    /// The `GremlinQueryStatus` projection `ListGremlinQueries` and
    /// `GetGremlinQueryStatus` report.
    pub fn status(&self, now: SystemTime) -> GremlinQueryStatus {
        GremlinQueryStatus {
            query_id: Some(self.query_id.clone()),
            query_string: Some(self.query_string.clone()),
            query_eval_stats: Some(self.eval_stats(now)),
        }
    }

    /// Cancel this query, unless it has already finished.
    ///
    /// Returns whether this call is what cancelled it, so a caller can tell a first
    /// cancel from a repeat. The record is KEPT either way: `GetGremlinQueryStatus` has
    /// to go on reporting `cancelled` for the id afterwards, and keeping it is also what
    /// makes the second cancel of one id answer exactly like the first.
    pub fn cancel(&mut self, now: SystemTime) -> bool {
        if self.state.is_terminal() {
            return false;
        }
        self.state = QueryState::Cancelled;
        self.cancelled = true;
        self.finished_at = Some(now);
        true
    }
}

/// Per-service state for com-amazonaws-neptunedata-amazonneptunedataplane. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNeptunedataAmazonneptunedataplaneState {
    /// The one engine record `GetEngineStatus` reports, built when this state is first
    /// reached.
    pub engine_status: EngineStatusRecord,
    /// Every Gremlin query the engine has accepted, keyed by query id. A `BTreeMap` is
    /// what gives `ListGremlinQueries` a stable order over the ids.
    pub gremlin_queries: BTreeMap<String, GremlinQueryRecord>,
}

impl ComAmazonawsNeptunedataAmazonneptunedataplaneState {
    /// How many accepted-but-not-completed queries the registry holds.
    pub fn accepted_query_count(&self) -> usize {
        self.gremlin_queries
            .values()
            .filter(|query| query.state.is_accepted())
            .count()
    }

    /// How many queries are evaluating right now.
    pub fn running_query_count(&self) -> usize {
        self.gremlin_queries
            .values()
            .filter(|query| query.state.is_running())
            .count()
    }

    /// The current queries, oldest id first: the running ones, plus the queued ones when
    /// `include_waiting` is set.
    pub fn current_queries(
        &self,
        include_waiting: bool,
        now: SystemTime,
    ) -> Vec<GremlinQueryStatus> {
        self.gremlin_queries
            .values()
            .filter(|query| {
                query.state.is_running() || (include_waiting && query.state == QueryState::Waiting)
            })
            .map(|query| query.status(now))
            .collect()
    }
}

/// An instant as the engine-status timestamps report it: the ISO-8601 (RFC 3339) form
/// `aws_smithy_types` renders a `DateTime` in.
///
/// The format error the renderer declares is reachable only for a year outside the
/// range it can print, which no instant this emulator stores can be, so an unprintable
/// instant reports as the empty string rather than failing a status read.
pub fn engine_timestamp(instant: SystemTime) -> String {
    let rendered = ::aws_smithy_types::DateTime::from(instant)
        .fmt(::aws_smithy_types::date_time::Format::DateTime);
    debug_assert!(
        rendered.is_ok(),
        "an instant this emulator stores must be printable: {rendered:?}"
    );
    rendered.unwrap_or_default()
}

/// The counter the second half of a request id comes from, and the per-process tag the
/// first half carries. Neither is service state: they name responses, not resources, so
/// they live beside the id generator rather than in the store.
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static PROCESS_TAG: OnceLock<u64> = OnceLock::new();

/// A fresh id naming ONE response, for the `requestId` a modelled Neptune error carries.
///
/// The member documents it only as "the ID of the request" and the service publishes no
/// format, so the one requirement is that two responses carry different ids. It is built
/// from a tag fixed when this process first needed an id and a counter, which is unique
/// within a run and across concurrent requests; the id names a request for correlation,
/// is derived from nothing the caller sent, and is not a credential.
pub fn new_request_id() -> String {
    let tag = *PROCESS_TAG.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since_epoch| since_epoch.as_nanos() as u64)
    });
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{tag:016x}-{sequence:016x}")
}

/// At most this many characters of a request-supplied value are echoed back in an error
/// message. Neptune names the offending value in its message, so echoing it is right, but
/// the length is the caller's choice and the message is not the place to carry it whole.
const ECHOED_VALUE_LIMIT: usize = 128;

/// A request-supplied value as an error message may carry it: control characters escaped
/// (so a message cannot forge a line in anything that renders it) and bounded in length.
fn echoed(value: &str) -> String {
    value.escape_debug().take(ECHOED_VALUE_LIMIT).collect()
}

/// The `InvalidParameterException` payload for a `queryId` the registry does not hold.
///
/// `code` carries the HTTP status, which is what the member's own documentation says it
/// is ("The HTTP status code returned with the exception"), and `detailedMessage` names
/// the member and the value that was wrong, so the caller can fix the request from it.
pub fn invalid_query_id(query_id: &str) -> InvalidParameterException {
    InvalidParameterException {
        detailed_message: format!("Supplied queryId {} is invalid", echoed(query_id)),
        request_id: new_request_id(),
        code: "400".to_string(),
    }
}

/// What a request that could not reach this service's store at all is told: a
/// server-side failure, not anything the caller sent. One statement of the message and
/// the status, so the two payloads below cannot drift apart.
const STORE_UNAVAILABLE_MESSAGE: &str = "the neptunedata engine store is unavailable";
const SERVER_ERROR_STATUS: &str = "500";

/// The `FailureByQueryException` payload for a request that could not reach this
/// service's store.
pub fn store_unavailable_failure() -> FailureByQueryException {
    FailureByQueryException {
        detailed_message: STORE_UNAVAILABLE_MESSAGE.to_string(),
        request_id: new_request_id(),
        code: SERVER_ERROR_STATUS.to_string(),
    }
}

/// The same server-side failure for an operation that models `InternalFailureException`
/// instead.
pub fn store_unavailable_internal() -> InternalFailureException {
    InternalFailureException {
        detailed_message: STORE_UNAVAILABLE_MESSAGE.to_string(),
        request_id: new_request_id(),
        code: SERVER_ERROR_STATUS.to_string(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNeptunedataAmazonneptunedataplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>)
}

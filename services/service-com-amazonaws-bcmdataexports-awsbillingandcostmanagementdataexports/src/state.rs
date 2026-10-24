// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports service.
//!
//! The service's one durable resource is the data export: created by `CreateExport`,
//! read back by `GetExport` / `ListExports` / `ListTagsForResource`, overwritten by
//! `UpdateExport` and removed by `DeleteExport`. Every one of those handlers computes
//! its answer from [`StoredExport`] values held here, so a create is observable by every
//! later reader and a delete is observable by none.
//!
//! The ARN/region/token/timestamp helpers below live beside the store rather than in any
//! one operation file because several operations mint or parse exactly the same values,
//! and two copies of an ARN rule is how a `GetExport` stops finding what `CreateExport`
//! stored.
//!
//! Beside the store there is also the TABLE CATALOG ([`catalog`], re-exported here): the
//! tables an account can query, their table properties and their column schemas. That
//! catalog is fixed service data — the same for every account, and nothing a caller does
//! changes it — so it is a static rather than a field of the state struct: there is
//! nothing to persist and nothing to mutate, and `GetTable` (with `ListTables`, which
//! reports the same entries) must answer the same catalog on every call. It lives beside
//! the store, not inside one operation file, for the same reason the ARN rule does: two
//! operations read it. [`query`] reads a submitted `DataQuery` against that catalog, for
//! the two operations that accept an export definition, and [`destination`] reads the
//! `DestinationConfigurations` of the same definition for the same two operations.
//!
//! Two more things the store holds, each read by a layer rather than by one handler:
//!
//! * the recorded EXECUTIONS of an export ([`StoredExecution`]), which `GetExecution`
//!   reports one of and `ListExecutions` pages over. The service OWES each export a
//!   delivery — one when `CreateExport` persists a definition and one whenever
//!   `UpdateExport` replaces it — and records an execution of it on the clock, without
//!   any caller call: the delivery is held on the export as a
//!   [`PendingDelivery`] until its instant falls due, and
//!   [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::materialize_due`]
//!   turns whatever the clock already owes into execution records. See
//!   [`StoredExecution`] for why the delivery this emulator cannot perform ends in the
//!   service's own modelled failure state.
//!
//! [`authorization`] is beside them: who a request speaks for and whether IAM lets it do
//! what it asks, for the operations whose model declares `AccessDeniedException`. It holds
//! nothing itself — the principal comes from STS's store and the policies from IAM's — so
//! it is a reader of other services' state rather than a field of this one.

use crate::ops::types::{
    ExecutionStatus, Export, ExportStatus, ResourceNotFoundException, ResourceTag,
    ValidationException, ValidationExceptionField,
};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

pub mod authorization;
pub mod catalog;
pub mod destination;
pub mod query;

// The catalog's types, data and refusals are reached as `crate::state::…` by the two
// operations that read it, exactly as they were before the catalog grew its own module.
// The query reader is reached as `crate::state::…` by the two operations that accept an
// export definition, for the same reason the catalog is: both of them read it.
pub use query::{
    effective_table_configurations, query_tables, validate_data_query, QueryRefusal,
    TableConfigurations,
};

// The destination reader is reached as `crate::state::…` by those same two operations:
// a submitted destination configuration is judged the same way whichever of them
// accepted it.
pub use destination::{
    delivers_output_encoding, supported_compressions, validate_destination_configurations,
    SUPPORTED_OUTPUT_ENCODINGS,
};

// The authorization decision is reached as `crate::state::…` by every operation whose
// model declares `AccessDeniedException`; see [`authorization`] for why the others are
// not judged.
pub use authorization::authorize;

pub use catalog::{
    primary_billing_view_arn, table_spec, unknown_table, ColumnGroup, ColumnSpec,
    PropertyDefault, TablePropertySpec, TableSpec, PROPERTY_ANY_VALUE,
    PROPERTY_BILLING_VIEW_ARN, PROPERTY_DEPRECATED, PROPERTY_FALSE, PROPERTY_FILTER,
    PROPERTY_INCLUDE_ALL_RECOMMENDATIONS, PROPERTY_INCLUDE_CAPACITY_RESERVATION_DATA,
    PROPERTY_INCLUDE_IAM_PRINCIPAL_DATA, PROPERTY_INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY,
    PROPERTY_INCLUDE_RESOURCES, PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
    PROPERTY_TIME_GRANULARITY, PROPERTY_TRUE, TABLE_CATALOG,
};

/// This service's SigV4 signing name, which is also its ARN namespace (see
/// `OPERATIONS.md`: "sigv4 signing name: bcm-data-exports", "ARN namespace:
/// bcm-data-exports").
pub const SIGNING_NAME: &str = "bcm-data-exports";

/// The region an ARN carries when the request is not signed, so no SigV4 credential
/// scope names one. Matches the region every generated emulator's shared config
/// defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `ResourceType` a `ResourceNotFoundException` reports for an export ARN that
/// names nothing: the Smithy shape the ARN identifies.
pub const EXPORT_RESOURCE_TYPE: &str = "Export";

/// The `ResourceType` a `ResourceNotFoundException` reports for an `ExecutionId` that
/// names no execution of the export it was asked about.
pub const EXECUTION_RESOURCE_TYPE: &str = "Execution";

/// The resource-ARN segment every export of this service is named under:
/// `export/<Export.Name>`.
pub const EXPORT_RESOURCE_PREFIX: &str = "export/";

/// The `ExportStatusCode` reported for an export that is operating normally. One of
/// `ExportStatusCode`'s two modelled values (see
/// `crate::ops::types::EXPORT_STATUS_CODE_VALUES`: `HEALTHY`, `UNHEALTHY`).
pub const EXPORT_STATUS_HEALTHY: &str = "HEALTHY";

/// The most exports this emulator retains, after which `CreateExport` answers the
/// modelled `ServiceQuotaExceededException` (HTTP 402, "You've reached the limit on the
/// number of resources you can create, or exceeded the size of an individual resource").
///
/// AWS enforces a per-account export quota through that same error; the published number
/// is not cited here, so this value is the EMULATOR's choice — set well above any
/// plausible test or demo workload. Its job is to bound the caller-driven growth of
/// [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::exports`],
/// which lives for the life of the process: without it, repeated `CreateExport` calls
/// grow the store without limit.
pub const MAX_EXPORTS: usize = 1000;

/// The `ServiceCode` a quota failure reports: this service's endpoint prefix.
pub const SERVICE_CODE: &str = SIGNING_NAME;

/// The quota a `ServiceQuotaExceededException` from `CreateExport` names.
pub const EXPORTS_PER_ACCOUNT_QUOTA_CODE: &str = "ExportsPerAccount";

/// The `ExecutionStatusCode` of a delivery the service has STARTED but not finished: the
/// first status an execution carries. One of the modelled values (see
/// `crate::ops::types::EXECUTION_STATUS_CODE_VALUES`).
pub const EXECUTION_STATUS_INITIATION_IN_PROCESS: &str = "INITIATION_IN_PROCESS";

/// The terminal `ExecutionStatusCode` of a delivery whose QUERY did not run: the step a
/// delivery fails at when the data this service exports cannot be queried at all, before
/// anything is written to the destination bucket. One of the modelled values.
pub const EXECUTION_STATUS_QUERY_FAILURE: &str = "QUERY_FAILURE";

/// The `ExecutionStatusReason` of a delivery that failed on the SERVICE's own side rather
/// than on anything about the caller's export: one of the modelled values (smithy_query
/// shape `ExecutionStatusReason`: `INSUFFICIENT_PERMISSION`, `BILL_OWNER_CHANGED`,
/// `INTERNAL_FAILURE`, `DEPRECATED`), and the only one of the four that does not make a
/// claim about the caller's permissions, bill ownership or query.
pub const EXECUTION_STATUS_REASON_INTERNAL_FAILURE: &str = "INTERNAL_FAILURE";

/// How long after the call that owes an export a delivery the service INITIATES it.
///
/// CHOSEN, not documented: AWS publishes no latency for a data-export delivery (it
/// promises only that the export refreshes when the source data updates), so this is the
/// emulator's own span. It is never zero — a caller listing executions straight after
/// `CreateExport` still sees the export before its first delivery exists — and it is short
/// enough that no test has to wait long for one. The instant is STORED on the export
/// ([`PendingDelivery::initiate_at`]) and materialised by whichever request next reaches
/// the store; nothing sleeps for it.
pub const DELIVERY_INITIATION_DELAY: Duration = Duration::from_millis(500);

/// How long after a delivery is initiated it reaches its terminal status.
///
/// CHOSEN for the same reason, and bounded the same way: a caller that reads an execution
/// as soon as it appears sees it in its non-terminal `INITIATION_IN_PROCESS` status, and
/// the terminal status is due a short span later. Stored on the execution
/// ([`StoredExecution::completion_due_at`]), never slept on.
pub const DELIVERY_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// How many executions one export keeps.
///
/// EMULATOR resource guard, not an AWS retention: AWS publishes no window for
/// execution history, so this value bounds
/// [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::executions`],
/// which lives for the life of the process and grows by one delivery per `CreateExport`
/// and per `UpdateExport`. The OLDEST execution of the export is dropped first, so the
/// deliveries a caller is most likely to read are the ones kept.
pub const MAX_EXECUTIONS_PER_EXPORT: usize = 100;

/// The delivery the service owes an export and has not initiated yet.
///
/// AWS delivers a data export without being asked — "The cadence for Amazon Web Services
/// to update the data export in your S3 bucket" (smithy_query shape `RefreshCadence`) —
/// so the delivery is owed by the CLOCK rather than by any operation a caller can call,
/// and what is stored here is the two instants it moves at plus the definition it runs.
/// All three are fixed by the call that owed it ([`DELIVERY_INITIATION_DELAY`] after it),
/// so WHICH later request materialises it cannot change the execution anybody reads.
#[derive(Debug, Clone)]
pub struct PendingDelivery {
    /// The instant the service initiates this delivery, and the instant the execution it
    /// records is created at.
    pub initiate_at: SystemTime,
    /// Which delivery of its export this is: the count of deliveries that export had been
    /// owed before it. No later delivery of the same export carries the same number, which
    /// is what makes the `ExecutionId` minted for it unique within the export (see
    /// [`execution_id`]).
    pub sequence: u64,
    /// The instant that execution reaches its terminal status, [`DELIVERY_SETTLES_AFTER`]
    /// later. Fixed here, with the initiation, rather than recomputed when the delivery is
    /// initiated, so neither instant depends on when a request happened to materialise it.
    pub complete_at: SystemTime,
    /// The definition this delivery runs, as it stood when the delivery was owed —
    /// "This export data is a snapshot from when the execution was generated"
    /// (smithy_query shape `GetExecutionResponse`, member `Export`).
    pub export: Export,
}

/// One recorded execution of an export: the delivery AWS runs for it, as
/// `GetExecution`/`ListExecutions` report it.
///
/// The service RECORDS the delivery, and does not perform it. An execution is the record
/// of a data-plane delivery — a query run against this account's billing data and a file
/// written to the caller's S3 bucket — and this emulator holds no billing data and writes
/// no such file, so the delivery it records cannot succeed. It therefore ends in the
/// service's OWN modelled failure state ([`EXECUTION_STATUS_QUERY_FAILURE`]) carrying the
/// modelled reason for a failure on the service's side
/// ([`EXECUTION_STATUS_REASON_INTERNAL_FAILURE`]), rather than reporting a
/// `DELIVERY_SUCCESS` for a file nothing wrote. Nothing is claimed for work nothing ran:
/// the export's `ExportStatus.LastRefreshedAt` stays absent, because no export file was
/// ever generated (see [`fresh_status`]).
#[derive(Debug, Clone)]
pub struct StoredExecution {
    /// The id this execution is addressed by, unique within its export.
    pub execution_id: String,
    /// The ARN of the export that generated it.
    pub export_arn: String,
    /// The status of this execution.
    pub status: ExecutionStatus,
    /// "This export data is a snapshot from when the execution was generated"
    /// (smithy_query shape `GetExecutionResponse`, member `Export`): the definition as it
    /// stood then, which a later `UpdateExport` does not change.
    pub export: Export,
    /// The instant this delivery was initiated at. Orders the executions of one export in
    /// time, which is how the oldest is the one [`MAX_EXECUTIONS_PER_EXPORT`] drops.
    pub initiated_at: SystemTime,
    /// The instant this execution reaches its terminal status, while it has not reached it
    /// yet; `None` once it has. A transition the CLOCK owes, materialised from this stored
    /// instant rather than by sleeping — see
    /// [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::materialize_due`].
    pub completion_due_at: Option<SystemTime>,
}

/// One persisted data export: the submitted definition, the server-owned status, and
/// the tags `CreateExport` associated with it.
#[derive(Debug, Clone)]
pub struct StoredExport {
    /// The server-minted ARN, which is also this export's key in
    /// [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::exports`].
    pub arn: String,
    /// The export definition as submitted, with `ExportArn` set to [`Self::arn`].
    pub export: Export,
    /// Server-owned status and timestamps.
    pub status: ExportStatus,
    /// The tags associated with this export, keyed by tag key so a key is unique.
    pub tags: BTreeMap<String, String>,
    /// The delivery the service owes this export and has not initiated yet, if any: set
    /// by the call that owed it (`CreateExport`, and each `UpdateExport`) and cleared when
    /// the clock makes it due and it is recorded as an execution.
    pub pending_delivery: Option<PendingDelivery>,
    /// How many deliveries this export has been owed, including the one still pending and
    /// any an update superseded. The next delivery's [`PendingDelivery::sequence`], and
    /// never reused, so no two deliveries of this export mint the same `ExecutionId`.
    pub deliveries_owed: u64,
}

/// Per-service state for com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports. A handler reaches it through [`service_state`].
///
/// There is ONE store, and no second index beside it: the ARN [`export_arn`] mints is a
/// function of the region and `Export.Name`, so the key of this map already IS the name
/// index, and `CreateExport` refuses a name in use by looking the minted ARN up here. A
/// second map keyed by the bare name would also refuse a name already used in a
/// DIFFERENT region, which the ARN says is a different export.
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState {
    /// The primary store, keyed by the minted export ARN. `BTreeMap` so every list
    /// walks the exports in one stable order and a page token can name a position in it.
    pub exports: BTreeMap<String, StoredExport>,
    /// The recorded executions, keyed by `(export ARN, execution id)` — the pair
    /// `GetExecution` addresses one by. `BTreeMap` so the executions of one export are a
    /// contiguous range in one stable order, which is the order `ListExecutions` pages
    /// over.
    pub executions: BTreeMap<(String, String), StoredExecution>,
}

impl ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState {
    /// The recorded execution `execution_id` of the export `export_arn`, if there is one.
    pub fn execution(&self, export_arn: &str, execution_id: &str) -> Option<&StoredExecution> {
        self.executions
            .get(&(export_arn.to_string(), execution_id.to_string()))
    }

    /// The recorded executions OF ONE EXPORT, in execution-id order.
    ///
    /// The store is keyed by `(export ARN, execution id)`, so the executions of one export
    /// are the contiguous range that starts at that ARN's smallest possible key: the walk
    /// visits each of them exactly once, in one stable order, and visits no execution of
    /// any other export.
    pub fn executions_of<'a>(
        &'a self,
        export_arn: &'a str,
    ) -> impl Iterator<Item = &'a StoredExecution> + 'a {
        self.executions
            .range((export_arn.to_string(), String::new())..)
            .take_while(move |((arn, _), _)| arn == export_arn)
            .map(|(_, execution)| execution)
    }

    /// Record `execution`, replacing any earlier record of the same execution of the same
    /// export.
    pub fn insert_execution(&mut self, execution: StoredExecution) {
        let key = (execution.export_arn.clone(), execution.execution_id.clone());
        self.executions.insert(key, execution);
    }

    /// Apply everything the CLOCK already owes, as of `now`: initiate each delivery whose
    /// instant has passed and move each initiated execution that is due its terminal
    /// status onto it.
    ///
    /// Every handler of this service calls this before it answers, so no caller has to
    /// poll a particular operation for a delivery to happen. This is a write a REPORTING
    /// operation may make: each change it applies was owed at `now` whether or not anybody
    /// called — the instants are stored on the export and on the execution by the earlier
    /// call that owed them ([`PendingDelivery::initiate_at`],
    /// [`StoredExecution::completion_due_at`]) and every value written is derived from
    /// those stored instants, so a pass that runs late writes exactly what a pass that ran
    /// on time would have written, and running it twice changes nothing.
    pub fn materialize_due(&mut self, now: SystemTime) {
        let initiated: Vec<(String, PendingDelivery)> = self
            .exports
            .iter_mut()
            .filter_map(|(arn, export)| {
                // Taken only when it is DUE: one predicate decides both whether this
                // delivery leaves the export and whether it is initiated below.
                export
                    .pending_delivery
                    .take_if(|delivery| delivery.initiate_at <= now)
                    .map(|delivery| (arn.clone(), delivery))
            })
            .collect();
        for (export_arn, delivery) in initiated {
            self.initiate_delivery(&export_arn, delivery);
        }

        for execution in self.executions.values_mut() {
            if execution
                .completion_due_at
                .is_some_and(|due_at| due_at <= now)
            {
                complete_delivery(execution);
            }
        }
    }

    /// Record `delivery` of the export `export_arn` as a started execution.
    fn initiate_delivery(&mut self, export_arn: &str, delivery: PendingDelivery) {
        let initiated_at = timestamp(delivery.initiate_at);
        self.insert_execution(StoredExecution {
            execution_id: execution_id(export_arn, delivery.sequence, delivery.initiate_at),
            export_arn: export_arn.to_string(),
            status: ExecutionStatus {
                status_code: Some(EXECUTION_STATUS_INITIATION_IN_PROCESS.to_string()),
                // A delivery that has not finished has nothing to report a reason for.
                status_reason: None,
                created_at: Some(initiated_at),
                completed_at: None,
                last_updated_at: Some(initiated_at),
            },
            export: delivery.export,
            initiated_at: delivery.initiate_at,
            completion_due_at: Some(delivery.complete_at),
        });
        self.trim_executions_of(export_arn);
    }

    /// Drop the oldest executions of `export_arn` until it holds at most
    /// [`MAX_EXECUTIONS_PER_EXPORT`].
    fn trim_executions_of(&mut self, export_arn: &str) {
        // The common path is an export inside the bound, which costs a count and no
        // allocation.
        if self.executions_of(export_arn).count() <= MAX_EXECUTIONS_PER_EXPORT {
            return;
        }
        let mut held: Vec<(SystemTime, String)> = self
            .executions_of(export_arn)
            .map(|execution| (execution.initiated_at, execution.execution_id.clone()))
            .collect();
        // Oldest first, and by id where two deliveries share an instant, so the walk is
        // the same on every pass.
        held.sort_unstable();
        let doomed = held.len() - MAX_EXECUTIONS_PER_EXPORT;
        for (_, execution_id) in held.into_iter().take(doomed) {
            self.executions
                .remove(&(export_arn.to_string(), execution_id));
        }
    }

    /// Forget every recorded execution of the export `export_arn`.
    ///
    /// An execution is addressed by `(ExportArn, ExecutionId)`, so an execution of an
    /// export that no longer exists can be addressed by nobody: `DeleteExport` calls this
    /// so no reader reports the history of an export the store no longer holds.
    pub fn remove_executions_of(&mut self, export_arn: &str) {
        let doomed: Vec<(String, String)> = self
            .executions
            .range((export_arn.to_string(), String::new())..)
            .take_while(|((arn, _), _)| arn == export_arn)
            .map(|(key, _)| key.clone())
            .collect();
        for key in doomed {
            self.executions.remove(&key);
        }
    }
}

/// Domain separator for a minted `ExecutionId`, so the digest of a delivery cannot be read
/// as any other value this service derives with the same hash.
const EXECUTION_ID_DOMAIN: &[u8] = b"bcm-data-exports/execution-id/v1\0";

/// Bytes of digest an `ExecutionId` carries, hex-encoded — wide enough that two deliveries
/// cannot be confused for one another.
const EXECUTION_ID_BYTES: usize = 16;

/// The `ExecutionId` of the `sequence`-th delivery of `export_arn`, initiated at `at`.
///
/// An execution id is an opaque `GenericString` a caller only ever hands back (smithy_query
/// operation `GetExecution`, member `ExecutionId`), so it is DERIVED rather than drawn at
/// random: the digest of this service's domain separator, the export's ARN, the delivery's
/// sequence within that export and the instant it was initiated at. The sequence is what
/// makes it unique within the export — no later delivery of the same export reuses a
/// number — and the digest is the one this crate already mints its opaque page tokens with,
/// rather than a second mechanism beside it.
fn execution_id(export_arn: &str, sequence: u64, at: SystemTime) -> String {
    use ::sha2::Digest;
    let nanos = at
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(EXECUTION_ID_DOMAIN);
    hasher.update(export_arn.as_bytes());
    hasher.update(b"\0");
    hasher.update(sequence.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    ::hex::encode(&hasher.finalize()[..EXECUTION_ID_BYTES])
}

/// Move `execution` onto the terminal status of a delivery this emulator cannot perform.
///
/// A delivery queries this account's billing data and writes a file to the caller's S3
/// bucket. This emulator holds no billing data and writes no such file, so the delivery
/// fails at the step that would have run the query, with the modelled reason for a failure
/// on the SERVICE's side: the caller's permissions, bill ownership and query are not what
/// stopped it. The instants come from the stored due instant, not from the clock this pass
/// reads, so a pass that runs late records what one that ran on time would have recorded.
fn complete_delivery(execution: &mut StoredExecution) {
    let Some(completed_at) = execution.completion_due_at.take().map(timestamp) else {
        return;
    };
    execution.status.status_code = Some(EXECUTION_STATUS_QUERY_FAILURE.to_string());
    execution.status.status_reason = Some(EXECUTION_STATUS_REASON_INTERNAL_FAILURE.to_string());
    execution.status.completed_at = Some(completed_at);
    execution.status.last_updated_at = Some(completed_at);
}

/// The delivery the service owes an export whose definition `export` was persisted at
/// `at`, as that export's `sequence`-th: due [`DELIVERY_INITIATION_DELAY`] later, of exactly
/// that definition.
///
/// Both operations that persist a definition — `CreateExport` and `UpdateExport` — owe the
/// export a delivery of what they persisted, so they schedule it through this one place.
pub fn delivery_owed_at(at: SystemTime, export: &Export, sequence: u64) -> PendingDelivery {
    let initiate_at = at + DELIVERY_INITIATION_DELAY;
    PendingDelivery {
        initiate_at,
        sequence,
        complete_at: initiate_at + DELIVERY_SETTLES_AFTER,
        export: export.clone(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState>)
}

/// The current wall-clock instant, as the modelled `Timestamp` type, at WHOLE-SECOND
/// precision.
///
/// RECORDED AWS behaviour: an `ExportStatus` timestamp comes back carrying no sub-second
/// part — the recorded instants are whole epoch seconds, and a replay of the recording
/// against an emulator emitting a fractional epoch value reports the difference in shape.
/// The instants this service records are therefore truncated here, in the ONE place that
/// mints them, rather than at each serializer, where one path could keep a fraction the
/// others drop.
///
/// The instant itself is wall-clock, so it is a per-run value: what is pinned is its
/// type, its units and its precision, not the number.
pub fn now() -> ::aws_smithy_types::DateTime {
    timestamp(SystemTime::now())
}

/// `at` as the modelled `Timestamp` type, at the WHOLE-SECOND precision [`now`] explains.
///
/// Every instant this service reports goes through here, including the ones taken from a
/// stored due instant rather than from the clock, so no path reports a fraction the others
/// drop.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let seconds = at
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs() as i64);
    ::aws_smithy_types::DateTime::from_secs(seconds)
}

/// The region an ARN minted for this request carries: the region of the request's
/// SigV4 credential scope, else [`DEFAULT_REGION`].
///
/// A scope region that could not appear in the modelled `Arn` pattern's region segment
/// (`[-a-z0-9]*`) is not used, so a minted ARN always satisfies the pattern the model
/// declares for it.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region_segment(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `value` is a legal region segment of the modelled `Arn` pattern
/// `^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`.
fn is_arn_region_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The ARN of the export called `name` in `region`:
/// `arn:aws:bcm-data-exports:<region>:<account>:export/<name>`.
///
/// Built through `engine::emulator_arn`, so the account segment is the one canonical
/// emulator account id rather than a literal.
pub fn export_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("{EXPORT_RESOURCE_PREFIX}{name}"),
    )
}

/// The `Message` a `ResourceNotFoundException` of the EXPORT-DEFINITION operations
/// carries (`GetExport`, `UpdateExport`, `DeleteExport`).
///
/// Recorded AWS behavior: BCM Data Exports answers a lookup of an export ARN that
/// resolves to nothing with this one fixed string, whatever the ARN was — an ARN in the
/// caller's own account naming an export that was never created and an ARN naming a
/// DIFFERENT account get byte-identical replies, so the body confirms nothing about the
/// ARN the caller supplied. The message is therefore a constant and never interpolates
/// the request: the identity of the ARN that was not found is reported in the modelled
/// `ResourceId` member ("The identifier of the resource that was not found"), which is
/// where the model puts it.
pub const EXPORT_NOT_FOUND_MESSAGE: &str = "No matching definition found";

/// The `Message` a `ResourceNotFoundException` of the RESOURCE-TAGGING operations carries
/// (`ListTagsForResource`, `TagResource`, `UntagResource`).
///
/// Recorded AWS behavior: the tagging operations word the same condition as a resource
/// miss rather than as a missing export definition — the wire recording for a tag
/// listing of an absent export ARN, and the one for a `TagResource` of it, both carry this
/// string where the definition operations carry [`EXPORT_NOT_FOUND_MESSAGE`]. Like that
/// one it is a fixed, request-independent constant: which ARN was not found is reported in
/// `ResourceId`.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The `Message` a `ResourceNotFoundException` of the EXECUTION lookup carries — the
/// resource `GetExecution` addresses.
///
/// Recorded AWS behavior: a `GetExecution` whose pair resolves to no execution reports the
/// EXECUTION as missing, with this string, and does so whether it was the `ExecutionId`
/// that named no execution of its export or the `ExportArn` that named no export at all:
/// the execution is the resource the operation addresses, and the export ARN only scopes
/// it. Like the two above it is a fixed, request-independent constant naming the KIND of
/// resource that was missed; WHICH identifier missed is reported in `ResourceId` /
/// `ResourceType`.
pub const EXECUTION_NOT_FOUND_MESSAGE: &str = "Execution not found";

/// The modelled `ResourceNotFoundException` for an export ARN that names nothing, worded
/// with `message`: [`EXPORT_NOT_FOUND_MESSAGE`] on the export-definition operations and
/// [`RESOURCE_NOT_FOUND_MESSAGE`] on the resource-tagging ones.
pub fn resource_not_found(arn: &str, message: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: message.to_string(),
        resource_id: arn.to_string(),
        resource_type: EXPORT_RESOURCE_TYPE.to_string(),
    }
}

/// The modelled `ResourceNotFoundException` an export-definition operation answers an
/// export ARN that names nothing with.
pub fn export_not_found(arn: &str) -> ResourceNotFoundException {
    resource_not_found(arn, EXPORT_NOT_FOUND_MESSAGE)
}

/// The modelled `ResourceNotFoundException` `GetExecution` answers an `ExportArn` that
/// names no export with: the EXECUTION it was asked for is what is missing (see
/// [`EXECUTION_NOT_FOUND_MESSAGE`]), while the members still name the export ARN that
/// resolved to nothing, because that is what the caller has to fix.
pub fn execution_export_not_found(export_arn: &str) -> ResourceNotFoundException {
    resource_not_found(export_arn, EXECUTION_NOT_FOUND_MESSAGE)
}

/// The HTTP status this service answers `ResourceNotFoundException` with.
///
/// The error SHAPE carries `@httpError 404`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), but the SERVICE
/// answers the not-found condition with 400 carrying that same shape, its members
/// and its `__type`: the wire recordings for a read, an update, a delete, a tag listing, a
/// `TagResource` and an execution lookup of a resource that resolves to nothing all answer
/// HTTP 400. A client
/// branches on the status as well as on the code — and an HTTP-level proxy treats a 404
/// as a routing failure rather than an API error — so the pair it sees has to be the pair
/// the service sends. The status is stated here once and applied through
/// [`crate::scaffolded::Scaffolded::ModeledWithStatus`], which leaves the shape, its
/// members and its wire code the model's.
pub const EXPORT_NOT_FOUND_STATUS: u16 = 400;

/// [`export_not_found`] lifted into an operation's error channel, answered with
/// [`EXPORT_NOT_FOUND_STATUS`].
///
/// `wrap` is the calling operation's own `ResourceNotFoundException` variant, so every
/// operation that looks an export ARN up reports the miss through this ONE place and none
/// of them can drift back to the shape's own status.
pub fn export_not_found_error<E>(
    arn: &str,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    not_found_error(export_not_found(arn), wrap)
}

/// As [`export_not_found_error`], for the resource-tagging operations, which word the
/// condition with [`RESOURCE_NOT_FOUND_MESSAGE`].
pub fn tagged_resource_not_found_error<E>(
    arn: &str,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    not_found_error(resource_not_found(arn, RESOURCE_NOT_FOUND_MESSAGE), wrap)
}

/// `error` lifted into an operation's error channel at [`EXPORT_NOT_FOUND_STATUS`].
pub fn not_found_error<E>(
    error: ResourceNotFoundException,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(wrap(error), EXPORT_NOT_FOUND_STATUS)
}

/// The modelled not-found for an `ExecutionId` that names no execution of the export it
/// was asked about.
///
/// It carries [`EXECUTION_NOT_FOUND_MESSAGE`], the sentence of the resource kind this
/// lookup missed — an execution, not the export definition the `ExportArn` resolved to.
/// Like every other not-found of this service the sentence is a constant: the identity of
/// what was not found is reported in the modelled `ResourceId`, not in the prose.
pub fn execution_not_found(execution_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: EXECUTION_NOT_FOUND_MESSAGE.to_string(),
        resource_id: execution_id.to_string(),
        resource_type: EXECUTION_RESOURCE_TYPE.to_string(),
    }
}

/// The message a tagging operation refuses a `ResourceArn` of another account with.
///
/// Recorded AWS behavior: an ARN that names a different account is refused by the tagging
/// operations with this sentence — which says nothing about the ARN, because the refusal
/// is reached BEFORE any lookup and so cannot report on a resource it never consulted.
/// (The recorded text is an example of the sentence; the requirement is the
/// `ValidationException` classification and that the refusal does not depend on
/// existence.)
pub const RESOURCE_NOT_TAGGABLE_MESSAGE: &str = "Resource doesn't support tagging";

/// The modelled 400 a tagging operation refuses an unusable `ResourceArn` with.
fn not_taggable() -> ValidationException {
    ValidationException {
        message: RESOURCE_NOT_TAGGABLE_MESSAGE.to_string(),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "ResourceArn".to_string(),
            message: "Must name a resource of this account.".to_string(),
        }]),
    }
}

/// Refuse `arn` unless it names a resource of THIS account.
///
/// AWS validates a tagging ARN's account before any existence lookup, so an ARN naming
/// another account is a `ValidationException` (400) rather than a
/// `ResourceNotFoundException`: no caller of this account can tag another account's
/// resource, whatever this account holds, and deciding it without a lookup is exactly
/// what keeps the answer from reporting on a resource AWS never consults — the
/// cross-tenant probe this ordering rules out.
///
/// What is left to check is only the ACCOUNT: the modelled `Arn` pattern
/// (`^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`) is
/// already enforced at the boundary, so the partition, the service and the shape of the
/// account and resource segments hold by the time a handler runs. The RESOURCE segment is
/// deliberately not judged here: an ARN of this account whose resource names no export is
/// a resource this service looked for and did not find, which is the 404.
pub fn validate_taggable_resource_arn(arn: &str) -> Result<(), ValidationException> {
    let names_this_account = arn
        .split(':')
        .nth(ARN_ACCOUNT_SEGMENT)
        .is_some_and(|account| account == ::engine::emulator_account_id());
    if names_this_account {
        Ok(())
    } else {
        Err(not_taggable())
    }
}

/// The 0-based position of the account segment in an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
const ARN_ACCOUNT_SEGMENT: usize = 4;

/// The status a freshly created export carries.
///
/// `LastRefreshedAt` is "The timestamp of when the export was last generated"
/// (smithy_query shape `ExportStatus`, member `LastRefreshedAt`), and it stays absent for
/// the life of the export in this emulator: a delivery here never writes an export file
/// (see [`StoredExecution`]), so there is no instant at which one was generated and none
/// is reported. The failure of a delivery is reported where the model puts an execution's
/// own outcome — on the execution, in its `ExecutionStatus` — and not as a verdict on the
/// definition: the two modelled `ExportStatusCode` values say whether the EXPORT is
/// healthy, and an export whose only failure is this emulator's missing data plane
/// ([`EXECUTION_STATUS_REASON_INTERNAL_FAILURE`], a failure on the service's side) is not
/// a misconfigured export.
pub fn fresh_status(at: ::aws_smithy_types::DateTime) -> ExportStatus {
    ExportStatus {
        status_code: Some(EXPORT_STATUS_HEALTHY.to_string()),
        status_reason: None,
        created_at: Some(at),
        last_updated_at: Some(at),
        last_refreshed_at: None,
    }
}

/// Bytes of the integrity tag an opaque page token carries ahead of its position.
const PAGE_TOKEN_TAG_LEN: usize = 8;

/// Domain separator for the page-token tag, so a token minted for this service's listings
/// cannot be read as anything else.
const PAGE_TOKEN_DOMAIN: &[u8] = b"bcm-data-exports/page-token/v1\0";

/// Which SEQUENCE a page token names a position in: the listing that cut it and, where
/// that listing pages over the contents of one resource, which resource's contents.
///
/// A token is bound to that sequence as well as to its position, because the paginated
/// listings page over different key spaces (`ListExports` over export ARNs,
/// `ListTagsForResource` over one export's tag keys, `ListTables` over the table catalog's
/// names, `ListExecutions` over one export's execution ids): a token handed to a sequence
/// it was never cut from names a position in some other sequence — a tag key where an
/// execution id is expected, or a position in a DIFFERENT export's executions — and must
/// be refused rather than resumed at an unrelated offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTokenKind<'a> {
    /// `ListExports`, paging over the export ARNs in [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::exports`].
    Exports,
    /// `ListTagsForResource`, paging over the tag keys of the export `resource_arn` names.
    ResourceTags {
        /// The export whose tags this token walks.
        resource_arn: &'a str,
    },
    /// `ListTables`, paging over the table names of [`TABLE_CATALOG`].
    Tables,
    /// `ListExecutions`, paging over the execution ids of the export `export_arn` names.
    Executions {
        /// The export whose executions this token walks.
        export_arn: &'a str,
    },
}

impl PageTokenKind<'_> {
    /// Mix the bytes that distinguish this sequence into the tag's preimage.
    ///
    /// A per-resource listing mixes the resource's ARN as well as the listing, and every
    /// field is NUL-terminated — an ARN of this service cannot contain a NUL (the modelled
    /// `Arn` pattern admits none), so no two sequences share a preimage.
    fn mix(self, hasher: &mut ::sha2::Sha256) {
        use ::sha2::Digest;
        match self {
            Self::Exports => hasher.update(b"Exports\0"),
            Self::ResourceTags { resource_arn } => {
                hasher.update(b"ResourceTags\0");
                hasher.update(resource_arn.as_bytes());
                hasher.update(b"\0");
            }
            Self::Tables => hasher.update(b"Tables\0"),
            Self::Executions { export_arn } => {
                hasher.update(b"Executions\0");
                hasher.update(export_arn.as_bytes());
                hasher.update(b"\0");
            }
        }
    }
}

/// The integrity tag of `key` for `kind`: the leading [`PAGE_TOKEN_TAG_LEN`] bytes of
/// SHA-256 over the service domain separator, the sequence, and the key.
fn page_token_tag(kind: PageTokenKind<'_>, key: &str) -> Vec<u8> {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(PAGE_TOKEN_DOMAIN);
    kind.mix(&mut hasher);
    hasher.update(key.as_bytes());
    hasher.finalize()[..PAGE_TOKEN_TAG_LEN].to_vec()
}

/// A page token naming the key the NEXT page of `kind` starts at: an integrity tag
/// followed by the key, hex-encoded. Opaque — a client cannot read a position out of it
/// and must hand it back verbatim — and self-verifying, so a token this listing never
/// issued is rejected instead of being read as some arbitrary position in it.
fn encode_page_token(kind: PageTokenKind<'_>, key: &str) -> String {
    let mut bytes = page_token_tag(kind, key);
    bytes.extend_from_slice(key.as_bytes());
    ::hex::encode(bytes)
}

/// The key a token issued by [`encode_page_token`] for `kind` names, or `None` when this
/// sequence never issued it: not hex, too short to carry a tag, not UTF-8, or carrying a
/// tag that does not match the sequence and position it claims.
fn decode_page_token(kind: PageTokenKind<'_>, token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    if bytes.len() < PAGE_TOKEN_TAG_LEN {
        return None;
    }
    let (tag, key) = bytes.split_at(PAGE_TOKEN_TAG_LEN);
    let key = String::from_utf8(key.to_vec()).ok()?;
    (page_token_tag(kind, &key) == tag).then_some(key)
}

/// One page of `items` (ordered by their keys): at most `max_results` values starting at
/// the key `token` names, plus the token that resumes immediately after the page.
///
/// `None` when `token` was not issued by the `kind` sequence — the caller answers that
/// with the modelled `ValidationException`. The returned token is `None` on the last
/// page, so a paginating client stops.
pub fn page<T: Clone>(
    kind: PageTokenKind<'_>,
    items: &[(String, T)],
    max_results: Option<i32>,
    token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let start = match token {
        Some(token) if !token.is_empty() => {
            let key = decode_page_token(kind, token)?;
            items.partition_point(|(item_key, _)| item_key.as_str() < key.as_str())
        }
        _ => 0,
    };
    // An absent MaxResults means "no caller-imposed limit". The modelled range (1..300)
    // is already enforced at the boundary, so a present one is at least 1; the floor
    // keeps that true here independently of boundary validation, because a zero-sized
    // page handed out with a resume token would make a paginating client loop forever.
    let limit = max_results.map_or(items.len(), |max| max.max(1) as usize);
    let end = start.saturating_add(limit).min(items.len());
    let values = items[start..end]
        .iter()
        .map(|(_, value)| value.clone())
        .collect();
    let next = items
        .get(end)
        .map(|(key, _)| encode_page_token(kind, key));
    Some((values, next))
}

/// The most tags one export holds.
///
/// The modelled `ResourceTagList` is `@length 0..200` (smithy_query shape
/// `ResourceTagList`), and that list is both what a tagging request submits and what
/// `ListTagsForResource` reports for an export — so an export holding more than this many
/// tags could not be reported within its own modelled bound. A request that would take an
/// export past it is refused with the modelled `ValidationException` instead.
pub const MAX_RESOURCE_TAGS: usize = 200;

/// A submitted `ResourceTagList` collapsed into the keyed map the store holds, or the
/// modelled refusal for a list that names one key twice.
///
/// "Each tag consists of a key and a value, and each key must be unique for the resource"
/// (the modelled doc for `ResourceTags`), so a list carrying one key twice names two
/// values for one tag and cannot be satisfied. Lives here because every operation that
/// accepts a `ResourceTagList` — `CreateExport` and `TagResource` — must read it the same
/// way, or one of them would silently keep the last value for a duplicated key.
pub fn tag_map(tags: Vec<ResourceTag>) -> Result<BTreeMap<String, String>, ValidationException> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for tag in tags {
        if map.insert(tag.key.clone(), tag.value).is_some() {
            return Err(duplicate_tag_key(&tag.key));
        }
    }
    Ok(map)
}

/// The modelled 400 for a `ResourceTags` list that carries `key` more than once.
pub fn duplicate_tag_key(key: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "ResourceTags carries the key {} more than once; each tag key must be unique for the resource.",
            clip_for_message(key)
        ),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "ResourceTags".to_string(),
            message: "Each tag key must be unique for the resource.".to_string(),
        }]),
    }
}

/// The modelled 400 for a request that would leave an export holding `total` tags, past
/// [`MAX_RESOURCE_TAGS`].
pub fn too_many_tags(total: usize) -> ValidationException {
    ValidationException {
        message: format!(
            "The request would leave the resource with {total} tags; a resource holds at most {MAX_RESOURCE_TAGS}."
        ),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "ResourceTags".to_string(),
            message: format!("A resource holds at most {MAX_RESOURCE_TAGS} tags."),
        }]),
    }
}

/// The tags of an export as the modelled list, ordered by key.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<ResourceTag> {
    tags.iter()
        .map(|(key, value)| ResourceTag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The `ValidationExceptionReason` a refusal of one named input member reports. One of
/// the modelled values (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`:
/// `cannotParse`, `fieldValidationFailed`, `other`, `unknownOperation`).
pub const VALIDATION_REASON_FIELD_FAILED: &str = "fieldValidationFailed";

/// The most characters of a caller-supplied value a refusal message echoes.
///
/// `TableName` admits 1024 characters and a table property value 16384 (smithy_query
/// shapes `TableName`, `TableProperties`), while `ValidationException$Message` admits
/// only 1024 — so an echoed value is clipped to keep a refusal built from an outsized
/// one inside the message's own modelled bound.
const MESSAGE_VALUE_CLIP: usize = 128;

/// `value` as a refusal message quotes it: itself when short, else its first
/// [`MESSAGE_VALUE_CLIP`] characters and an ellipsis.
pub(crate) fn clip_for_message(value: &str) -> String {
    if value.chars().count() <= MESSAGE_VALUE_CLIP {
        return value.to_string();
    }
    let head: String = value.chars().take(MESSAGE_VALUE_CLIP).collect();
    format!("{head}...")
}

// The export's data query — its parsing, its bounds and its refusals — lives in
// [`query`], which `CreateExport` and `UpdateExport` both read through
// [`validate_data_query`]: a definition cannot be judged one way by one and another way
// by the other.

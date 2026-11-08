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
//!   `UpdateExport` replaces it — and performs and records it on the clock, without
//!   any caller call: the delivery is held on the export as a
//!   [`PendingDelivery`] until its instant falls due, and
//!   [`delivery::materialize_due_deliveries`] runs whatever the clock already owes — the
//!   export's query and the write of its file into the destination bucket — and records
//!   each one as an execution. See [`delivery`] for what a delivery writes and where.
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
pub mod delivery;
pub mod destination;
pub mod query;

// The DELIVERY of an export — the file it writes into the destination bucket and the
// statuses the execution recording it moves through — is reached as `crate::state::…` by
// every handler, because every handler applies what the clock owes before it answers.
pub use delivery::{
    drive_delivery_clock, materialize_due_deliveries, pipeline_status, DeliveryOutcome,
    DueDelivery, DELIVERY_PIPELINE, EXECUTION_STATUS_DELIVERY_FAILURE,
    EXECUTION_STATUS_DELIVERY_IN_PROCESS, EXECUTION_STATUS_DELIVERY_SUCCESS,
    EXECUTION_STATUS_QUERY_IN_PROCESS, EXECUTION_STATUS_QUERY_QUEUED,
    EXECUTION_STATUS_REASON_INSUFFICIENT_PERMISSION, OVERWRITE_REPORT,
};

// The catalog's types, data and refusals are reached as `crate::state::…` by the two
// operations that read it, exactly as they were before the catalog grew its own module.
// The query reader is reached as `crate::state::…` by the two operations that accept an
// export definition, for the same reason the catalog is: both of them read it.
pub use query::{
    delivered_columns, resolve_data_query, Projected, QueryRefusal, TableConfigurations,
    MAX_DELIVERED_COLUMNS,
};

// The destination reader is reached as `crate::state::…` by those same two operations:
// a submitted destination configuration is judged the same way whichever of them
// accepted it.
pub use destination::{
    delivers_output_encoding, supported_compressions, validate_delivery_grant,
    validate_destination_configurations, BUCKET_PERMISSION_VALIDATION_FAILED, DELIVERY_ACTION,
    DELIVERY_SERVICE_PRINCIPAL, SUPPORTED_OUTPUT_ENCODINGS,
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
    PROPERTY_TIME_GRANULARITY, PROPERTY_TRUE, TABLE_CATALOG, TABLE_PROPERTIES_FIELD,
};

/// This service's SigV4 signing name, which is also its ARN namespace (see
/// `OPERATIONS.md`: "sigv4 signing name: bcm-data-exports", "ARN namespace:
/// bcm-data-exports").
pub const SIGNING_NAME: &str = "bcm-data-exports";

/// The Region every ARN this service mints carries.
///
/// RECORDED AWS behaviour: BCM Data Exports is a REGION-PINNED control plane. An export
/// ARN it answers carries this one Region whatever Region the client is configured for —
/// a create signed for another Region is answered an ARN in this one, and the resource a
/// refusal of such a call names is in this one too. That is why the S3 bucket policy a
/// delivery is granted by conditions `aws:SourceArn` on an ARN in this Region: an ARN
/// stamped with the caller's Region would match no policy written for the service.
///
/// The modelled `Arn` pattern leaves the Region segment unconstrained
/// (`^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`), so
/// the pinning is recorded behaviour rather than a model constraint.
pub const SERVICE_REGION: &str = "us-east-1";

/// The `ResourceType` a `ResourceNotFoundException` reports for an export ARN that
/// names nothing: the Smithy shape the ARN identifies.
pub const EXPORT_RESOURCE_TYPE: &str = "Export";

/// The `ResourceType` a `ResourceNotFoundException` reports for an `ExecutionId` that
/// names no execution of the export it was asked about.
pub const EXECUTION_RESOURCE_TYPE: &str = "Execution";

/// The resource-ARN segment every export of this service is named under:
/// `export/<Export.Name>/<generated id>` (see [`export_arn`]).
pub const EXPORT_RESOURCE_PREFIX: &str = "export/";

/// The resource segment of the ARN that names EVERY export of this account: the resource
/// an authorization decision about an export that does not exist yet is taken against
/// (see [`any_export_arn`]).
pub const ANY_EXPORT_RESOURCE: &str = "export/*";

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

/// The `ServiceCode` a quota failure reports.
///
/// The model does not leave this one to the emulator: it states the single value the
/// member always carries — "The service code that exceeded quota. It will always be
/// “AWSBillingAndCostManagementDataExports”" (smithy_query query="doc"
/// shape="ServiceQuotaExceededException" member="ServiceCode"), and `ThrottlingException`'s
/// own `ServiceCode` is documented with the same fixed value — so that value is what the
/// member carries, whatever the request. It is this service's Smithy service name, NOT its
/// endpoint prefix / SigV4 signing name ([`SIGNING_NAME`]), which is what ARNs and the
/// credential scope are built from.
pub const SERVICE_CODE: &str = "AWSBillingAndCostManagementDataExports";

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
/// the terminal status is due a short span later. This span is also what the service's own
/// steps for the delivery are spread over ([`DELIVERY_PIPELINE`]), so a caller polling an
/// execution across it observes each of them. Stored on the execution
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
/// The service PERFORMS the delivery and records it: it runs the export's data query and
/// writes the resulting file into the destination bucket named by the definition, through the
/// object store of the Amazon S3 service on this same engine, and the status this execution
/// reports is the outcome of that write — see [`delivery`]. A delivery that was written ends
/// in the modelled [`EXECUTION_STATUS_DELIVERY_SUCCESS`] and moves the export's
/// `ExportStatus.LastRefreshedAt` to the instant it finished; one the destination refused
/// ends in the modelled [`EXECUTION_STATUS_DELIVERY_FAILURE`] carrying the reason it was
/// refused for, and nothing is claimed for a file that was not written.
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
/// There is ONE store, and no second index beside it: the ARN [`export_arn`] mints carries
/// `Export.Name` ahead of the export's generated id, so the key of this map already IS the
/// name index and [`Self::export_named`] reads it as one.
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
    /// The export called `name`, whatever generated id its ARN carries.
    ///
    /// The name a caller chooses is unique among the exports this account HOLDS, while the
    /// ARN is not a function of it (see [`export_arn`]), so a lookup by name is a walk of
    /// the ARNs minted under [`export_arn_prefix`] rather than a `get` of one key.
    /// `Export.Name` cannot carry a `/` (smithy_query shape `Export`, member `Name`:
    /// `pattern ^[0-9A-Za-z\-_]+$`) and neither can a generated id, so exactly one segment
    /// follows that prefix for an export of this name and a longer tail belongs to another
    /// one.
    pub fn export_named(&self, name: &str) -> Option<&StoredExport> {
        let prefix = export_arn_prefix(name);
        self.exports
            .range(prefix.clone()..)
            .take_while(|(arn, _)| arn.starts_with(&prefix))
            .find(|(arn, _)| !arn[prefix.len()..].contains('/'))
            .map(|(_, export)| export)
    }

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

    /// Apply what the CLOCK already owes this service's SCHEDULE, as of `now`: initiate
    /// each delivery whose instant has passed, and move each initiated execution onto the
    /// pipeline status in force for it.
    ///
    /// This is the half of the clock's work that needs only this service's own store.
    /// Performing a due delivery — running the query and writing the file into the
    /// destination bucket — needs the object store too, so it is driven by
    /// [`materialize_due_deliveries`], which is what every handler calls and which calls
    /// this first.
    ///
    /// This is a write a REPORTING operation may make: each change it applies was owed at
    /// `now` whether or not anybody called — the instants are stored on the export and on
    /// the execution by the earlier call that owed them ([`PendingDelivery::initiate_at`],
    /// [`StoredExecution::completion_due_at`]) and every value written is derived from
    /// those stored instants, so a pass that runs late writes exactly what a pass that ran
    /// on time would have written, and running it twice changes nothing.
    pub(crate) fn materialize_due(&mut self, now: SystemTime) {
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
            advance_pipeline(execution, now);
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
    /// Which keys belong to one export is decided in ONE place — [`Self::executions_of`] —
    /// so the set this removes is exactly the set a listing reports: the borrow of the
    /// store ends at the `collect`, which is what lets the removals below take it mutably.
    pub fn remove_executions_of(&mut self, export_arn: &str) {
        let doomed: Vec<String> = self
            .executions_of(export_arn)
            .map(|execution| execution.execution_id.clone())
            .collect();
        for execution_id in doomed {
            self.executions
                .remove(&(export_arn.to_string(), execution_id));
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

/// Move `execution` onto the pipeline status in force for it at `now`.
///
/// A delivery that has not reached its terminal status is at one of the steps the service
/// takes for it ([`DELIVERY_PIPELINE`]), decided by the two instants stored on the execution
/// — the one it was initiated at and the one its terminal status is due at — so the status a
/// reader sees is the one the clock had already moved it to, and a pass that runs late
/// reports it as having moved when it fell due rather than when the pass ran. An execution
/// that has reached its terminal status is left alone: nothing moves afterwards.
fn advance_pipeline(execution: &mut StoredExecution, now: SystemTime) {
    let Some(complete_at) = execution.completion_due_at else {
        return;
    };
    // A delivery whose terminal status is already due does not move through the pipeline:
    // what it owes is the write that finishes it, which `materialize_due_deliveries`
    // performs.
    if complete_at <= now {
        return;
    }
    let Some((code, moved_at)) = pipeline_status(execution.initiated_at, complete_at, now) else {
        return;
    };
    if execution.status.status_code.as_deref() == Some(code) {
        return;
    }
    execution.status.status_code = Some(code.to_string());
    execution.status.last_updated_at = Some(timestamp(moved_at));
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

/// The ARN of the export called `name` whose generated id is `export_id`:
/// `arn:aws:bcm-data-exports:<this service's Region>:<account>:export/<name>/<export_id>`.
///
/// An export is identified by the ARN the SERVICE mints for it, and that ARN carries a
/// server-generated id after the caller's name: the name is only unique among the exports the
/// account currently holds, so a name alone cannot identify the export it named yesterday as
/// well as the one holding it now. Because the id enters the ARN, a name freed by a delete
/// and taken again by a later create belongs to a DIFFERENT export with an ARN of its own,
/// and no ARN this service ever minted names a resource it did not mint it for.
///
/// The Region segment is [`SERVICE_REGION`] and NOT the region the request was signed for:
/// this is a region-pinned control plane, so the identity it mints does not move with the
/// caller's configuration.
///
/// Built through `engine::emulator_arn`, so the account segment is the one canonical
/// emulator account id rather than a literal.
pub fn export_arn(name: &str, export_id: &str) -> String {
    format!("{}{export_id}", export_arn_prefix(name))
}

/// Everything of an [`export_arn`] for `name` but the generated id: the prefix every ARN
/// this service has ever minted for that name starts with, and which
/// [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::export_named`]
/// looks the current one up by.
pub fn export_arn_prefix(name: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        SERVICE_REGION,
        &format!("{EXPORT_RESOURCE_PREFIX}{name}/"),
    )
}

/// The ARN naming EVERY export of this account:
/// `arn:aws:bcm-data-exports:<this service's Region>:<account>:export/*`.
///
/// RECORDED AWS behaviour: this is the resource an authorization decision about a create is
/// taken against, and the resource its refusal names. A create's export does not exist when
/// the decision is made — and the ARN it will be given carries a server-generated id no
/// policy author could have written down ([`export_arn`]) — so the decision is about the
/// export resource of this account rather than about one export.
pub fn any_export_arn() -> String {
    ::engine::emulator_arn(SIGNING_NAME, SERVICE_REGION, ANY_EXPORT_RESOURCE)
}

/// Domain separator for a minted export id, so the digest behind one cannot be read as any
/// other value this service derives with the same hash.
const EXPORT_ID_DOMAIN: &[u8] = b"bcm-data-exports/export-id/v1\0";

/// How many export ids this process has minted. Every mint takes the next number, so two
/// mints never share a preimage even when the clock does not advance between them — which
/// is what makes the id unique rather than merely unlikely to repeat.
static EXPORT_IDS_MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// A fresh generated id for an export the service is about to mint an ARN for.
///
/// Derived from NOTHING THE REQUEST CARRIES: the id is what makes an export's ARN its own,
/// so two exports created under the same name — a create, a delete, and a create of that
/// name again — must not share one, and any value computed from the name, the region or the
/// definition would repeat for exactly that case. What it is computed from is this process's
/// own mint counter and the instant of the mint, digested with the hash this crate already
/// mints its other opaque ids with ([`execution_id`], and the page tokens below), rather
/// than a second mechanism beside them.
///
/// It is laid out in the form this service's export ARNs carry after the name — the 8-4-4-4-12
/// hex of a version 4 UUID, with that version's own version and variant bits set — and
/// satisfies the resource segment of the modelled `Arn` pattern (`[-a-zA-Z0-9/:_]+`).
pub fn new_export_id() -> String {
    use ::sha2::Digest;
    let sequence = EXPORT_IDS_MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(EXPORT_ID_DOMAIN);
    hasher.update(sequence.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; EXPORT_ID_BYTES];
    bytes.copy_from_slice(&digest[..EXPORT_ID_BYTES]);
    // The version nibble (4) and the variant bits (0b10) of the form above.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Bytes of digest an export id is laid out from: the 16 a UUID carries.
const EXPORT_ID_BYTES: usize = 16;

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
    not_found_exception(message, arn, EXPORT_RESOURCE_TYPE)
}

/// The modelled `ResourceNotFoundException` reporting `resource_id` of kind
/// `resource_type` with the sentence `message` — every member held inside the bound the
/// model gives it.
///
/// All three members target `GenericString`, `@length(0, 1024)`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), while the
/// identifier the refusal reports is a caller-supplied value whose OWN modelled shape
/// admits more — an `Arn` runs to 2048 characters, twice what the member it is reported
/// in admits — and the boundary validator hands such a value straight to the handler. So
/// the identifier gives way to the member's bound, exactly as an echoed resource gives
/// way to the room a refusal sentence leaves it ([`clip_to_budget`]): an identifier the
/// member has room for is reported WHOLE, because it is the identity of what was not
/// found and a client reads it to know which of its own requests failed
/// (`tests/all/not_found_message.rs`), and a longer one is reported by its leading
/// characters, which is what a client can still match its request against.
fn not_found_exception(
    message: &str,
    resource_id: &str,
    resource_type: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: clip_to_budget(message, GENERIC_STRING_LENGTH_LIMIT),
        resource_id: clip_to_budget(resource_id, GENERIC_STRING_LENGTH_LIMIT),
        resource_type: clip_to_budget(resource_type, GENERIC_STRING_LENGTH_LIMIT),
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
    not_found_exception(
        EXECUTION_NOT_FOUND_MESSAGE,
        execution_id,
        EXECUTION_RESOURCE_TYPE,
    )
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
    field_validation_failed(
        "ResourceArn",
        RESOURCE_NOT_TAGGABLE_MESSAGE.to_string(),
        "Must name a resource of this account.".to_string(),
    )
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
///
/// The account segment is read through the shared ARN parse, so this service's idea of
/// which field of an ARN is the account is the tree's one idea of it.
pub fn validate_taggable_resource_arn(arn: &str) -> Result<(), ValidationException> {
    let names_this_account = ::service_common::arn::parse(arn)
        .is_some_and(|parsed| parsed.account == ::engine::emulator_account_id());
    if names_this_account {
        Ok(())
    } else {
        Err(not_taggable())
    }
}

/// The status a freshly created export carries.
///
/// `LastRefreshedAt` is "The timestamp of when the export was last generated" (smithy_query
/// shape `ExportStatus`, member `LastRefreshedAt`), and a newly created export has not been
/// generated yet: the delivery it is owed has not run, so there is no instant to report
/// until one has — [`delivery`] sets it to the instant the delivery that wrote an export
/// file finished. The outcome of a delivery is reported where the model puts it — on the
/// execution, in its `ExecutionStatus` — and not as a verdict on the definition: the two
/// modelled `ExportStatusCode` values say whether the EXPORT is healthy.
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
/// sequence never issued it: not hex, too short to carry a tag, not UTF-8, carrying a tag
/// that does not match the sequence and position it claims, or spelled any way other than
/// the ONE spelling [`encode_page_token`] writes.
///
/// Decoding the bytes is only the first half of reading a token: `::hex::decode` reads more
/// spellings than `::hex::encode` writes — it accepts `A-F` as well as the `a-f` the minter
/// emits — and the tag is computed over the DECODED bytes, so it is identical for every
/// spelling of one position. A token is therefore kept only when it is, byte for byte, the
/// token this listing would mint for the position it decodes to: re-minting is what
/// compares the two, so the accepted spelling is the ENCODER's rather than a second grammar
/// maintained beside it, and a re-cased copy of an issued token — a string no response of
/// this service ever carried, so not one a caller could hand back unchanged — is refused
/// like every other value this listing did not issue.
fn decode_page_token(kind: PageTokenKind<'_>, token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    if bytes.len() < PAGE_TOKEN_TAG_LEN {
        return None;
    }
    let (_, key) = bytes.split_at(PAGE_TOKEN_TAG_LEN);
    let key = String::from_utf8(key.to_vec()).ok()?;
    (encode_page_token(kind, &key) == token).then_some(key)
}

/// The sentence every listing refuses a `NextToken` it never issued with.
const UNISSUED_PAGE_TOKEN_MESSAGE: &str =
    "NextToken is not a pagination token issued by this service.";

/// The input member a page-token refusal names.
const PAGE_TOKEN_MEMBER: &str = "NextToken";

/// One page of `items` for the `kind` sequence, or the calling operation's own modelled
/// `ValidationException` for a `token` that sequence never issued.
///
/// `wrap` is that operation's `ValidationException` variant and `token_requirement` the
/// sentence its own `NextToken` field carries, so every paginated listing reports the
/// unissued-token condition — the modelled 400 naming member [`PAGE_TOKEN_MEMBER`] with
/// reason [`VALIDATION_REASON_FIELD_FAILED`] — through this ONE place and none of them can
/// drift from the others. This is the only way to reach `page`, whose `None` is exactly
/// that condition.
pub fn page_or_token_refusal<T: Clone, E>(
    kind: PageTokenKind<'_>,
    items: &[(String, T)],
    max_results: Option<i32>,
    token: Option<&str>,
    token_requirement: &str,
    wrap: impl FnOnce(ValidationException) -> E,
) -> Result<(Vec<T>, Option<String>), E> {
    page(kind, items, max_results, token).ok_or_else(|| wrap(unissued_page_token(token_requirement)))
}

/// The modelled 400 a listing answers a `NextToken` it never issued with, carrying
/// `token_requirement` as what the listing's own token must be.
fn unissued_page_token(token_requirement: &str) -> ValidationException {
    field_validation_failed(
        PAGE_TOKEN_MEMBER,
        UNISSUED_PAGE_TOKEN_MESSAGE.to_string(),
        token_requirement.to_string(),
    )
}

/// One page of `items` (ordered by their keys): at most `max_results` values starting at
/// the key `token` names, plus the token that resumes immediately after the page.
///
/// `None` when `token` was not issued by the `kind` sequence. Private: a listing reaches
/// it through [`page_or_token_refusal`], which turns that `None` into the one modelled
/// refusal every listing of this service shares. The returned token is `None` on the last
/// page, so a paginating client stops.
fn page<T: Clone>(
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

/// The tag-key prefix Amazon Web Services reserves for the tags it generates itself.
///
/// Amazon Web Services' tagging rules reserve this prefix for its own use: a caller can
/// neither create nor edit nor delete a tag key carrying it. The model cannot express the
/// rule — `ResourceTagKey` is `@length 1..128` with NO `@pattern` (smithy_query shape
/// `ResourceTagKey`) — so every path on this surface that takes a caller-supplied tag key
/// enforces it instead ([`check_tag_keys_not_reserved`]). The comparison is
/// case-insensitive, because the namespace is reserved however the prefix is cased.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` names a tag in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// `str::get` rather than a byte-range slice (`key[..4]`), which PANICS when the key
/// carries a multi-byte character straddling that byte index — `ResourceTagKey` carries no
/// `@pattern`, so a key of any characters at all passes boundary validation and reaches
/// here. `get` answers `None` for a key shorter than the prefix or one whose boundary
/// falls mid-character, which is exactly "not reserved".
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// `Ok(())` when no key in `keys` is reserved, else the modelled 400 naming `member` and
/// the offending keys.
///
/// One helper so every operation that takes caller-supplied tag keys — `CreateExport` and
/// `TagResource` through [`tag_map`], and `UntagResource` with its `ResourceTagKeys` —
/// refuses that one namespace the same way and cannot drift apart. The offenders are
/// sorted and deduplicated, so a request naming one of them twice reports it once and the
/// text does not depend on iteration order, each is clipped the way every other refusal of
/// this service quotes a caller-supplied value, and the LIST of them is budgeted by
/// [`clip_list_for_message`] — a tagging request names up to 200 keys, so quoting all of
/// them would carry the message past its own modelled bound. The refusal itself is built
/// through [`field_validation_failed`], like every other this service words.
pub fn check_tag_keys_not_reserved<'a>(
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), ValidationException> {
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .collect();
    if reserved.is_empty() {
        return Ok(());
    }
    reserved.sort_unstable();
    reserved.dedup();
    let quoted: Vec<String> = reserved.into_iter().map(clip_for_message).collect();
    let quoted: Vec<&str> = quoted.iter().map(String::as_str).collect();
    // The sentence is built before the keys so the room they get is what it leaves of the
    // message's own modelled bound (the trailing `.` included), rather than a figure
    // restated here that a reworded sentence would silently invalidate.
    let sentence = format!(
        "{member} names tag keys in the reserved {RESERVED_TAG_KEY_PREFIX} namespace, \
         which a caller cannot set or remove: "
    );
    let offenders = clip_list_for_message(
        &quoted,
        ", ",
        MESSAGE_LENGTH_LIMIT.saturating_sub(sentence.len() + ".".len()),
    );
    Err(field_validation_failed(
        member,
        format!("{sentence}{offenders}."),
        format!(
            "A tag key must not begin with {RESERVED_TAG_KEY_PREFIX}; that prefix is \
             reserved for the tags Amazon Web Services generates."
        ),
    ))
}

/// A submitted `ResourceTagList` collapsed into the keyed map the store holds, or the
/// modelled refusal for a list that names one key twice or claims the reserved namespace.
///
/// "Each tag consists of a key and a value, and each key must be unique for the resource"
/// (the modelled doc for `ResourceTags`), so a list carrying one key twice names two
/// values for one tag and cannot be satisfied. A key in the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace is refused for the reason stated there, and
/// refused for the WHOLE list before any of it is collapsed, so a request mixing an
/// ordinary pair with a reserved one stores neither. Lives here because every operation
/// that accepts a `ResourceTagList` — `CreateExport` and `TagResource` — must read it the
/// same way, or one of them would silently keep the last value for a duplicated key or
/// seed a key the other refuses.
pub fn tag_map(tags: Vec<ResourceTag>) -> Result<BTreeMap<String, String>, ValidationException> {
    check_tag_keys_not_reserved(TAG_LIST_MEMBER, tags.iter().map(|tag| tag.key.as_str()))?;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for tag in tags {
        if map.insert(tag.key.clone(), tag.value).is_some() {
            return Err(duplicate_tag_key(&tag.key));
        }
    }
    Ok(map)
}

/// The input member a submitted `ResourceTagList` arrives in, which both operations that
/// accept one name identically (smithy_query operations `CreateExport`, `TagResource`).
pub const TAG_LIST_MEMBER: &str = "ResourceTags";

/// The input member `UntagResource` names the tag keys to remove in (smithy_query
/// operation `UntagResource`).
pub const TAG_KEY_LIST_MEMBER: &str = "ResourceTagKeys";

/// The modelled 400 for a `ResourceTags` list that carries `key` more than once.
pub fn duplicate_tag_key(key: &str) -> ValidationException {
    field_validation_failed(
        TAG_LIST_MEMBER,
        format!(
            "ResourceTags carries the key {} more than once; each tag key must be unique for the resource.",
            clip_for_message(key)
        ),
        "Each tag key must be unique for the resource.".to_string(),
    )
}

/// The modelled 400 for a request that would leave an export holding `total` tags, past
/// [`MAX_RESOURCE_TAGS`].
pub fn too_many_tags(total: usize) -> ValidationException {
    field_validation_failed(
        TAG_LIST_MEMBER,
        format!(
            "The request would leave the resource with {total} tags; a resource holds at most {MAX_RESOURCE_TAGS}."
        ),
        format!("A resource holds at most {MAX_RESOURCE_TAGS} tags."),
    )
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
/// shapes `TableName`, `TableProperties`), while a refusal message admits only
/// [`MESSAGE_LENGTH_LIMIT`] — so an echoed value is clipped to leave room for the sentence
/// around it. Clipping each value is not by itself the message's bound: how many values a
/// sentence states is the caller's to choose too, which is what [`clip_list_for_message`]
/// holds, and what the assembled sentence runs to is what [`field_validation_failed`]
/// holds.
const MESSAGE_VALUE_CLIP: usize = 128;

/// What a clipped value ends with, standing for the characters that were dropped.
const ELLIPSIS: &str = "...";

/// `value` as a refusal message quotes it: itself when short, else its first
/// [`MESSAGE_VALUE_CLIP`] characters and an ellipsis.
pub(crate) fn clip_for_message(value: &str) -> String {
    if value.chars().count() <= MESSAGE_VALUE_CLIP {
        return value.to_string();
    }
    let head: String = value.chars().take(MESSAGE_VALUE_CLIP).collect();
    format!("{head}{ELLIPSIS}")
}

/// `value` in at most `budget` bytes: itself when it fits, else as much of it as the
/// budget admits — cut on a character boundary, never mid-UTF-8 — and an ellipsis.
///
/// Takes the budget rather than reading one, so a sentence can hand a caller-supplied
/// value the room that is LEFT once the clause around it is accounted for: a value an
/// echoing sentence cannot fit is the caller's, while the clause around it is what tells
/// the caller what to do, so it is the value that gives way.
pub(crate) fn clip_to_budget(value: &str, budget: usize) -> String {
    if value.len() <= budget {
        return value.to_string();
    }
    let mut end = budget.saturating_sub(ELLIPSIS.len()).min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    match value.get(..end) {
        Some(head) if !head.is_empty() => format!("{head}{ELLIPSIS}"),
        _ => String::new(),
    }
}

/// The modelled 400 that reports ONE named input member at fault: `message` as the
/// refusal's own sentence and `requirement` as what that member must carry.
///
/// Every refusal this service words itself is built here, so the modelled
/// `GenericString` members it fills — `Message`, and the `Name` and `Message` of the one
/// `ValidationExceptionField` it names — are held inside [`MESSAGE_LENGTH_LIMIT`] in ONE
/// place rather than at each sentence, and the reason it reports is
/// [`VALIDATION_REASON_FIELD_FAILED`] for all of them. A sentence that budgeted its own
/// echoes ([`clip_for_message`], [`clip_list_for_message`]) is already inside the bound
/// and passes through unchanged; this is what holds the bound for every other.
pub fn field_validation_failed(
    member: &str,
    message: String,
    requirement: String,
) -> ValidationException {
    ValidationException {
        message: clip_to_budget(&message, MESSAGE_LENGTH_LIMIT),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: clip_to_budget(member, MESSAGE_LENGTH_LIMIT),
            message: clip_to_budget(&requirement, MESSAGE_LENGTH_LIMIT),
        }]),
    }
}

/// The most a value reported in a modelled `GenericString` member may run to, in bytes.
///
/// Every string member of every error shape of this service targets `GenericString`,
/// `@length(0, 1024)` (smithy_query shape `GenericString`), and that bound counts
/// CHARACTERS — a byte budget bounds those too, since no character is shorter than a byte,
/// and is what a value's own parts can be measured in as they are assembled.
pub(crate) const GENERIC_STRING_LENGTH_LIMIT: usize = 1024;

/// The most a refusal message may run to, in bytes: the bound of the `GenericString` it
/// is reported in ([`GENERIC_STRING_LENGTH_LIMIT`]).
pub(crate) const MESSAGE_LENGTH_LIMIT: usize = GENERIC_STRING_LENGTH_LIMIT;

/// How a refusal message quotes the several values it was built from: `items` joined with
/// `separator` for as many as `budget` bytes admit, and the number of items left unquoted
/// stated in place of them (`…, and 191 more`).
///
/// [`clip_for_message`] bounds ONE echoed value, but a refusal built from a LIST is as long
/// as the list, and these lists are caller-sized: a tagging request names up to 200 tag
/// keys of 128 characters each (smithy_query shapes `ResourceTagKeyList`,
/// `ResourceTagList`, `ResourceTagKey`) and a refused request is stated clause by clause,
/// one clause per offending value — either of which runs to tens of thousands of
/// characters, while a message admits [`MESSAGE_LENGTH_LIMIT`]. So the WHOLE quote is
/// budgeted and not merely each item of it, and the result is at most `budget` bytes
/// INCLUDING the count it ends with: a caller passes what its own sentence leaves of the
/// message's bound, and the refusal then stays inside that bound however many values the
/// request carried.
///
/// The items quoted are the leading ones, so the quote is a prefix of the list as the
/// caller sent it rather than an arbitrary selection, and a budget too small for even the
/// first value whole still quotes what it can of it — because a refusal naming no
/// offending value leaves the caller nothing to act on. Whatever is not quoted WHOLE is
/// counted, so the quote and the count together always account for the list.
pub(crate) fn clip_list_for_message(items: &[&str], separator: &str, budget: usize) -> String {
    if items.is_empty() {
        return String::new();
    }
    // What the trailing count costs when `left` items go unquoted.
    let tail = |left: usize| -> String {
        if left == 0 {
            String::new()
        } else {
            format!("{separator}and {left} more")
        }
    };

    let mut quoted = String::new();
    let mut taken = 0usize;
    for (index, item) in items.iter().enumerate() {
        let joiner = if index == 0 { "" } else { separator };
        let left = items.len() - index - 1;
        if quoted.len() + joiner.len() + item.len() + tail(left).len() > budget {
            break;
        }
        quoted.push_str(joiner);
        quoted.push_str(item);
        taken = index + 1;
    }
    if taken == 0 {
        // Not even the first value fits whole. As much of it as the budget admits is
        // quoted — clipped by [`clip_to_budget`], so the one clipping rule of this crate
        // decides where the cut falls — and the count then covers EVERY value, this one
        // included, so nothing is reported as quoted that was not quoted whole. A budget
        // with room for neither quotes nothing: the sentence still states the member and
        // the rule.
        let Some(first) = items.first() else {
            return String::new();
        };
        let counted = tail(items.len());
        let fragment = clip_to_budget(first, budget.saturating_sub(counted.len()));
        return if fragment.is_empty() {
            String::new()
        } else {
            format!("{fragment}{counted}")
        };
    }
    format!("{quoted}{}", tail(items.len() - taken))
}

// The export's data query — its parsing, its bounds and its refusals — lives in
// [`query`], which `CreateExport` and `UpdateExport` both read through
// [`resolve_data_query`]: a definition cannot be judged one way by one and another way
// by the other.

#[cfg(test)]
mod tests {
    use super::*;

    /// The budget [`clip_list_for_message`] promises, over the list sizes, item sizes and
    /// character widths a request can carry. The end-to-end rule — that the refusals built
    /// on it satisfy `ValidationException$Message`'s modelled bound — is asserted over the
    /// wire in `tests/all/refusal_message_bound.rs`; this pins the contract the builders
    /// rely on, including budgets too small for one whole item.
    #[test]
    fn a_quoted_list_stays_inside_its_budget_and_counts_what_it_left_out() {
        for (label, count, item_chars, filler, budget) in [
            ("one short item", 1usize, 4usize, 'k', 1024usize),
            ("a handful of short items", 8, 4, 'k', 1024),
            ("the tag-list ceiling of short items", MAX_RESOURCE_TAGS, 4, 'k', 1024),
            (
                "the tag-list ceiling at the tag-key ceiling",
                MAX_RESOURCE_TAGS,
                MESSAGE_VALUE_CLIP,
                'k',
                1024,
            ),
            (
                // Four bytes per character, so the characters fit where the bytes do not.
                "multi-byte items at the tag-key ceiling",
                MAX_RESOURCE_TAGS,
                MESSAGE_VALUE_CLIP,
                '𝄞',
                1024,
            ),
            ("a budget below one whole item", 20, 64, 'k', 40),
            ("a budget below one multi-byte item", 20, 64, '𝄞', 40),
            ("no budget at all", 20, 64, 'k', 0),
        ] {
            let items: Vec<String> = (0..count)
                .map(|index| format!("{index:03}{}", filler.to_string().repeat(item_chars)))
                .collect();
            let items: Vec<&str> = items.iter().map(String::as_str).collect();
            let quoted = clip_list_for_message(&items, ", ", budget);

            assert!(
                quoted.len() <= budget,
                "case {label:?}: expected at most {budget} bytes, got {}: {quoted:?}",
                quoted.len()
            );
            let spelled = items.iter().filter(|item| quoted.contains(**item)).count();
            let left_out: usize = quoted
                .rsplit_once("and ")
                .and_then(|(_, tail)| tail.strip_suffix(" more"))
                .and_then(|count| count.parse().ok())
                .unwrap_or(0);
            if quoted.is_empty() {
                // Only a budget with room for nothing at all quotes nothing.
                assert_eq!(
                    budget, 0,
                    "case {label:?}: a budget of {budget} bytes must quote something"
                );
                continue;
            }
            assert_eq!(
                spelled + left_out,
                count,
                "case {label:?}: expected all {count} items accounted for, got {spelled} spelled and {left_out} counted: {quoted:?}"
            );
            // The quote begins at the head of the list the caller sent: every item of a
            // row opens with its own three-digit index.
            assert!(
                quoted.starts_with(&items[0][..3]),
                "case {label:?}: expected the quote to begin with the first item, got {quoted:?}"
            );
            if budget >= MESSAGE_LENGTH_LIMIT {
                assert!(
                    quoted.contains(items[0]),
                    "case {label:?}: a message-sized budget must quote the first item whole, got {quoted:?}"
                );
            }
        }
    }

    /// An empty list has nothing to quote: the callers only build a refusal when they
    /// found an offending value, and the helper must not invent one.
    #[test]
    fn an_empty_list_quotes_nothing() {
        let quoted = clip_list_for_message(&[], ", ", MESSAGE_LENGTH_LIMIT);
        assert!(
            quoted.is_empty(),
            "expected no quote for an empty list, got {quoted:?}"
        );
    }
}

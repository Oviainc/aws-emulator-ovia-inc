// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-omics-omics service.
//!
//! This slice holds the service's primary resources: the **private workflow**
//! (`CreateWorkflow` / `GetWorkflow` / `ListWorkflows` / `DeleteWorkflow`), the
//! **versions** of that workflow (`CreateWorkflowVersion` / `GetWorkflowVersion`), the
//! **run** started from it (`StartRun` / `GetRun` / `ListRuns` / `DeleteRun`), the
//! **run group** that caps the compute those runs may use (`CreateRunGroup` /
//! `GetRunGroup` / `ListRunGroups`), the **run cache** a run reads task outputs from (`CreateRunCache` /
//! `GetRunCache`) and the **sequence store** that holds read sets
//! (`CreateSequenceStore` / `GetSequenceStore`). Each resource is stored exactly once, keyed by the numeric id
//! the model's `^[0-9]+$` patterns describe (a version by its name, inside its
//! workflow's record), so every operation reads and writes the one record.
//!
//! Every store is a `BTreeMap<u64, _>` keyed by that numeric id. The ids are minted
//! from a monotonic counter, so the map's own key order IS creation order: a page
//! token can simply name the id the next page starts at, and the order is stable
//! across calls even when records in between are deleted.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ContainerRegistryMap, DefinitionRepository, DefinitionRepositoryDetails, SequenceStoreS3Access,
    SseConfig, WorkflowParameter,
};

/// The ARN service namespace for this service (`arnNamespace`/sigv4 name `omics`).
pub const ARN_SERVICE: &str = "omics";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The resource portion a workflow ARN carries before its id.
pub const WORKFLOW_ARN_RESOURCE_PREFIX: &str = "workflow/";

/// What a workflow VERSION ARN carries between its workflow's id and the version
/// name: a version is named inside its workflow's resource path, which is what
/// `CreateWorkflowVersion` documents when it says "Version names appear in the workflow
/// version ARN".
pub const WORKFLOW_VERSION_ARN_INFIX: &str = "/version/";

/// The resource portion a run ARN carries before its id.
pub const RUN_ARN_RESOURCE_PREFIX: &str = "run/";

/// The resource portion a sequence store ARN carries before its id.
pub const SEQUENCE_STORE_ARN_RESOURCE_PREFIX: &str = "sequenceStore/";

/// The resource portion a run cache ARN carries before its id.
pub const RUN_CACHE_ARN_RESOURCE_PREFIX: &str = "runCache/";

/// The resource portion a run group ARN carries before its id.
pub const RUN_GROUP_ARN_RESOURCE_PREFIX: &str = "runGroup/";

/// The first id either counter mints.
///
/// The modelled `WorkflowId` / `RunId` are `^[0-9]+$` with length 1..18, so any
/// decimal below 10^18 is admissible; starting at seven digits keeps every id this
/// emulator mints the same width as the ids the real service hands out, and
/// `u64`-sized for the whole life of a process.
pub const FIRST_RESOURCE_ID: u64 = 1_000_000;

/// How far above [`FIRST_RESOURCE_ID`] the run counter starts.
///
/// Workflow ids and run ids are separate namespaces on the real service too, so an
/// overlap would still be correct — but a cross-type mix-up (a workflow id handed to
/// `GetRun`) would then resolve to an unrelated record instead of answering "not found",
/// masking the mistake. Offsetting the run counter makes that mix-up visible.
pub const RUN_ID_OFFSET: u64 = 500_000;

/// How far above [`FIRST_RESOURCE_ID`] the run group counter starts, for the same
/// reason [`RUN_ID_OFFSET`] exists: a run group id handed to `GetRun` (or a run id
/// handed to `GetRunGroup`) resolves to nothing and is answered "not found" instead of
/// reaching an unrelated record. The offsets are far enough apart that no store
/// can reach another's range under the caps below.
pub const RUN_GROUP_ID_OFFSET: u64 = 250_000;

/// How far above [`FIRST_RESOURCE_ID`] the run cache counter starts, for the same
/// reason, and far enough from [`RUN_ID_OFFSET`] and [`RUN_GROUP_ID_OFFSET`] that no
/// cache id can reach a run's or a run group's range under the caps below.
pub const RUN_CACHE_ID_OFFSET: u64 = 750_000;

/// `RunCacheStatus` a run cache carries once it exists.
///
/// From the modelled `RunCacheStatus` enum (ACTIVE | DELETED | FAILED), none of whose
/// values is a transient "being created" state: a cache the service answers a create
/// with is already usable, so nothing is owed to the clock here.
pub const RUN_CACHE_STATUS_ACTIVE: &str = "ACTIVE";

/// `CacheBehavior` applied when `CreateRunCache` omits `cacheBehavior`.
///
/// From that member's own documentation: "If you don't specify a value, the default
/// behavior is CACHE_ON_FAILURE."
pub const DEFAULT_CACHE_BEHAVIOR: &str = "CACHE_ON_FAILURE";

/// `WorkflowStatus` a freshly created private workflow carries, from the modelled
/// `WorkflowStatus` enum.
pub const WORKFLOW_STATUS_CREATING: &str = "CREATING";
/// The `WorkflowStatus` the clock owes a `CREATING` workflow.
pub const WORKFLOW_STATUS_ACTIVE: &str = "ACTIVE";

/// The first id the sequence-store counter mints.
///
/// The modelled `SequenceStoreId` is `^[0-9]+$` with length 10..36, so — unlike a
/// workflow or run id — a sequence store id must be at least TEN digits and the
/// counter above cannot serve it. Starting here makes every id this emulator mints
/// exactly ten digits wide, inside that length and inside `u64` for the whole life of
/// a process.
pub const FIRST_SEQUENCE_STORE_ID: u64 = 1_000_000_000;

/// `SequenceStoreStatus` a freshly created sequence store carries, from the modelled
/// `SequenceStoreStatus` enum.
pub const SEQUENCE_STORE_STATUS_CREATING: &str = "CREATING";
/// The `SequenceStoreStatus` the clock owes a `CREATING` sequence store, from the same
/// enum.
pub const SEQUENCE_STORE_STATUS_ACTIVE: &str = "ACTIVE";

/// The `ETagAlgorithmFamily` applied when `CreateSequenceStore` omits
/// `eTagAlgorithmFamily`.
///
/// From that member's own documentation: "The ETag algorithm family to use for ingested
/// read sets. The default value is MD5up."
pub const DEFAULT_E_TAG_ALGORITHM_FAMILY: &str = "MD5up";

/// `WorkflowType` of every workflow `CreateWorkflow` mints: the model's own
/// `PRIVATE`, the type that operation's documentation describes ("Creates a private
/// workflow").
pub const WORKFLOW_TYPE_PRIVATE: &str = "PRIVATE";

/// The `RunStatus` sequence a run walks, from the modelled `RunStatus` enum, in the
/// order the clock advances it through them. The last entry is terminal: no further
/// transition is owed once it is reached.
///
/// It ends in `FAILED`, not in `COMPLETED`. This emulator is a control plane: it
/// records a run faithfully — its workflow, role, parameters, storage, timestamps and
/// output location — but it runs no WDL, Nextflow or CWL engine and writes no output
/// object, so no run here can ever have succeeded. Reporting `COMPLETED` would tell a
/// caller a workflow produced outputs that do not exist, so the progression ends in
/// the modelled failure status carrying [`RUN_NOT_EXECUTED_FAILURE_REASON`] and
/// [`RUN_NOT_EXECUTED_STATUS_MESSAGE`], which say exactly why.
pub const RUN_STATUS_SEQUENCE: &[&str] = &["PENDING", "STARTING", "RUNNING", "FAILED"];

/// The `RunStatus` at which a run's `startTime` is stamped: the step at which the real
/// service considers the run started. From the same modelled `RunStatus` enum, and the
/// entry of [`RUN_STATUS_SEQUENCE`] the stamp belongs to, so the sequence and the
/// timestamp rule cannot drift apart.
pub const RUN_STATUS_RUNNING: &str = "RUNNING";

/// `RunStatus$failureReason` for a run no workflow engine executed.
///
/// Inside the modelled `RunFailureReason` length (1..64).
pub const RUN_NOT_EXECUTED_FAILURE_REASON: &str =
    "No workflow engine is emulated, so the run never executed.";

/// `RunStatus$statusMessage` for the same run, saying what the emulator did and did not
/// do.
pub const RUN_NOT_EXECUTED_STATUS_MESSAGE: &str =
    "This emulator records runs but executes no WDL, Nextflow or CWL workflow engine, \
     so the run ended without running and wrote no outputs.";

/// `RunStatus` values a run can hold once nothing further is owed to it.
///
/// From the modelled `RunStatus` enum: every value that is not one of the three
/// in-flight states (`PENDING`, `STARTING`, `RUNNING`) or the transient `STOPPING`.
pub const TERMINAL_RUN_STATUSES: &[&str] = &["COMPLETED", "DELETED", "CANCELLED", "FAILED"];

/// The `RunStatus` values `DeleteRun` admits.
///
/// From that operation's own documentation: "You can only delete a run that has reached
/// a COMPLETED, FAILED, or CANCELLED stage." A run in any other status is refused with
/// the modelled `ConflictException`.
pub const DELETABLE_RUN_STATUSES: &[&str] = &["COMPLETED", "FAILED", "CANCELLED"];

/// Whether `status` is one `DeleteRun` admits, from [`DELETABLE_RUN_STATUSES`].
pub fn is_deletable_run_status(status: &str) -> bool {
    DELETABLE_RUN_STATUSES.contains(&status)
}

/// What an operation on a workflow's VERSIONS answers when the workflow itself is not
/// one this emulator holds: there is no version collection to reach when its workflow
/// is absent.
///
/// The variable part is the workflow id the REQUEST carried.
pub fn workflow_not_found_message(workflow_id: &str) -> String {
    format!("Workflow {workflow_id} not found")
}

/// What `GetWorkflowVersion` answers when the workflow version the request names is
/// not one this emulator holds — either the workflow or the version is missing, which
/// are the same answer: the pair was not found.
///
/// The variable parts are the workflow id and the version name the REQUEST carried.
/// `DeleteWorkflowVersion` words the same condition its own way, so each operation
/// carries its own wording rather than sharing one.
pub fn get_workflow_version_not_found_message(workflow_id: &str, version_name: &str) -> String {
    format!("Workflow {workflow_id} with version name {version_name} not found")
}

/// The same condition as [`get_workflow_version_not_found_message`], as
/// `DeleteWorkflowVersion` words it.
pub fn delete_workflow_version_not_found_message(
    workflow_id: &str,
    version_name: &str,
) -> String {
    format!("Workflow {workflow_id} version {version_name} not found")
}

/// What `DeleteWorkflow` answers when the workflow still holds versions.
///
/// Fixed text: the refusal names no resource, because the rule is about the workflow in
/// the request's own path.
pub const WORKFLOW_HAS_VERSIONS_MESSAGE: &str = "Cannot delete the requested workflow, as it \
     has one or more versions. Delete the versions and try again.";

/// The modelled `StorageType` value that allocates a fixed amount of storage.
///
/// Source: `smithy_query(query="shape",shape="StorageType")` -> STATIC | DYNAMIC.
pub const STORAGE_TYPE_STATIC: &str = "STATIC";

/// `StorageType` applied when `StartRun` omits `storageType`.
///
/// From that member's own documentation: "By default, the run uses `STATIC` storage
/// type, which allocates a fixed amount of storage."
pub const DEFAULT_RUN_STORAGE_TYPE: &str = STORAGE_TYPE_STATIC;

/// `WorkflowType` applied when `StartRun` omits `workflowType`.
///
/// From that member's own documentation: "If you are running a `PRIVATE` workflow
/// (default), you do not need to include the workflow type."
pub const DEFAULT_RUN_WORKFLOW_TYPE: &str = WORKFLOW_TYPE_PRIVATE;

/// `RunRetentionMode` applied when `StartRun` omits `retentionMode`.
///
/// From that member's own documentation: "The default value is `RETAIN`."
pub const DEFAULT_RUN_RETENTION_MODE: &str = "RETAIN";

/// How long after the call that starts it a status transition this emulator owes to
/// the clock becomes due.
///
/// This is the emulator's own chosen span, NOT a duration AWS publishes: HealthOmics
/// documents no time for `CREATING` -> `ACTIVE` or for a run's progression, and a real
/// run takes minutes to hours. It is deliberately non-zero so a caller that reads
/// straight after the call still observes the transient status and the state rules
/// that go with it.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// DeleteWorkflow/DeleteRun, so each growth site carries a cap and nothing
// caller-sized reaches the store unchecked. HealthOmics publishes no public quota
// for either resource, so these are this emulator's resource guards, set well above
// any realistic use. Every other caller-sized member is already bounded by the
// model and refused by the centralized walk before a handler runs:
// `parameterTemplate` (1..2000 entries), `definitionUri` (1..256), `outputUri`
// (1..750), `name` (1..128) and so on.
// ---------------------------------------------------------------------------

/// The most workflows this emulator will hold. Emulator resource guard, not an AWS
/// quota. A create past it is refused with the modelled
/// `ServiceQuotaExceededException`.
pub const MAX_WORKFLOWS: usize = 500;

/// The most runs this emulator will hold. Emulator resource guard, not an AWS quota.
pub const MAX_RUNS: usize = 1000;

/// The most versions this emulator will hold for one workflow. Emulator resource
/// guard, not an AWS quota: each version is created by a caller and reclaimed only by
/// an explicit delete, so the per-workflow collection carries a cap too. A create past
/// it is refused with the modelled `ServiceQuotaExceededException`.
pub const MAX_WORKFLOW_VERSIONS: usize = 100;

/// The most sequence stores this emulator will hold. Emulator resource guard, not an
/// AWS quota — `CreateSequenceStore` documents that "You can create multiple sequence
/// stores per region per account" and publishes no ceiling. A create past it is refused
/// with the modelled `ServiceQuotaExceededException`.
pub const MAX_SEQUENCE_STORES: usize = 500;

/// The most run caches this emulator will hold. Emulator resource guard, not an AWS
/// quota — HealthOmics publishes no run cache ceiling. A create past it is refused with
/// the modelled `ServiceQuotaExceededException`.
pub const MAX_RUN_CACHES: usize = 500;

/// The most run groups this emulator will hold. Emulator resource guard, not an AWS
/// quota. A create past it is refused with the modelled
/// `ServiceQuotaExceededException`.
pub const MAX_RUN_GROUPS: usize = 500;

/// The most tags one workflow or run may carry.
///
/// From the `tags` member's own documentation on `CreateWorkflow`: "You can define
/// up to 50 tags for the workflow."
pub const MAX_TAGS: usize = 50;

/// The most idempotency tokens this emulator remembers per operation.
///
/// Emulator resource guard: the map is keyed by caller-supplied `requestId`, so it
/// is capped at one entry per resource either store can hold, and a token whose
/// resource is gone is reclaimed with it.
pub const MAX_IDEMPOTENCY_TOKENS: usize =
    MAX_WORKFLOWS + MAX_RUNS + MAX_SEQUENCE_STORES + MAX_RUN_CACHES + MAX_RUN_GROUPS;

/// One stored private workflow: every member `GetWorkflow` reports.
#[derive(Debug, Clone, Default)]
pub struct Workflow {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    pub uuid: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub engine: Option<String>,
    pub main: Option<String>,
    /// The S3 URI the definition was read from, when the create named one.
    pub definition: Option<String>,
    /// Content digest of the definition the create supplied.
    pub digest: Option<String>,
    pub parameter_template: BTreeMap<String, WorkflowParameter>,
    pub storage_capacity: Option<i32>,
    pub storage_type: Option<String>,
    pub accelerators: Option<String>,
    pub readme_path: Option<String>,
    pub container_registry_map: Option<ContainerRegistryMap>,
    /// The code repository the create sourced the definition from, when it named
    /// one; reported back as `definitionRepositoryDetails`.
    pub definition_repository: Option<DefinitionRepository>,
    /// Always [`WORKFLOW_TYPE_PRIVATE`] for a workflow minted by `CreateWorkflow`.
    pub r#type: String,
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time_ms: i64,
    /// When the transition this record owes the clock becomes due, in epoch millis;
    /// `None` once it has settled.
    pub status_due_at_ms: Option<i64>,
    pub tags: BTreeMap<String, String>,
    /// Every version `CreateWorkflowVersion` minted for this workflow, keyed by the
    /// modelled `WorkflowVersionName` — which that operation documents as unique for
    /// the workflow and unchangeable afterwards, so the name IS the key and the map
    /// holds each version exactly once.
    pub versions: BTreeMap<String, WorkflowVersion>,
}

impl Workflow {
    /// Apply every status change the clock already owed this workflow, or any version
    /// it holds, by `now_ms`.
    ///
    /// Reporting-only: it materialises a change that was due at the same instant
    /// whether or not anybody called, and the status it lands on does not depend on
    /// when the read happened.
    pub fn settle(&mut self, now_ms: i64) {
        settle_creating(
            &mut self.status,
            &mut self.status_due_at_ms,
            now_ms,
            WORKFLOW_STATUS_ACTIVE,
        );
        for version in self.versions.values_mut() {
            version.settle(now_ms);
        }
    }

    /// `parameterTemplate` as the wire shape wants it: omitted when the workflow
    /// carries no parameter at all.
    pub fn parameter_template_output(&self) -> Option<HashMap<String, WorkflowParameter>> {
        parameter_template_output(&self.parameter_template)
    }

    /// `tags` as the wire shape wants it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }

    /// `definitionRepositoryDetails` for the repository the create named.
    pub fn definition_repository_details(&self) -> Option<DefinitionRepositoryDetails> {
        definition_repository_details(self.definition_repository.as_ref())
    }
}

/// One stored workflow version: every member `GetWorkflowVersion` reports.
///
/// A version carries its own configuration rather than the workflow's:
/// `CreateWorkflowVersion` documents that "It doesn't inherit any configuration values
/// from the workflow", so no member here is filled in from the parent record.
#[derive(Debug, Clone, Default)]
pub struct WorkflowVersion {
    /// The modelled `WorkflowVersionName`: this record's key inside its workflow.
    pub version_name: String,
    pub arn: String,
    pub uuid: String,
    pub description: Option<String>,
    pub engine: Option<String>,
    pub main: Option<String>,
    /// The S3 URI the definition was read from, when the create named one.
    pub definition: Option<String>,
    /// Content digest of the definition the create supplied.
    pub digest: Option<String>,
    pub parameter_template: BTreeMap<String, WorkflowParameter>,
    pub storage_capacity: Option<i32>,
    pub storage_type: Option<String>,
    pub accelerators: Option<String>,
    pub readme_path: Option<String>,
    pub container_registry_map: Option<ContainerRegistryMap>,
    pub definition_repository: Option<DefinitionRepository>,
    pub workflow_bucket_owner_id: Option<String>,
    /// The `WorkflowType` of the workflow this version belongs to.
    pub r#type: String,
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time_ms: i64,
    /// When the transition this record owes the clock becomes due, in epoch millis;
    /// `None` once it has settled.
    pub status_due_at_ms: Option<i64>,
    pub tags: BTreeMap<String, String>,
    /// The `@idempotencyToken` of the request that created this version, so a replay
    /// of that token answers with this record instead of minting a second one. The
    /// token's namespace is the workflow's own version collection, which
    /// [`MAX_WORKFLOW_VERSIONS`] already bounds, so no separate token map grows here.
    pub request_id: String,
}

impl WorkflowVersion {
    /// Apply every status change the clock already owed this version by `now_ms`,
    /// under the same rule a workflow settles by.
    pub fn settle(&mut self, now_ms: i64) {
        settle_creating(
            &mut self.status,
            &mut self.status_due_at_ms,
            now_ms,
            WORKFLOW_STATUS_ACTIVE,
        );
    }

    /// `parameterTemplate` as the wire shape wants it: omitted when the version
    /// carries no parameter at all.
    pub fn parameter_template_output(&self) -> Option<HashMap<String, WorkflowParameter>> {
        parameter_template_output(&self.parameter_template)
    }

    /// `tags` as the wire shape wants it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }

    /// `definitionRepositoryDetails` for the repository the create named.
    pub fn definition_repository_details(&self) -> Option<DefinitionRepositoryDetails> {
        definition_repository_details(self.definition_repository.as_ref())
    }
}

/// Apply the `CREATING` -> active move the clock owes a freshly created record: one
/// rule, stated once, so every record that owes one settles identically. `active` is the
/// status that record's own modelled status enum calls active, so a workflow, a workflow
/// version and a sequence store each land on a value its own enum admits.
fn settle_creating(
    status: &mut String,
    status_due_at_ms: &mut Option<i64>,
    now_ms: i64,
    active: &str,
) {
    if status_due_at_ms.is_some_and(|due| due <= now_ms) {
        *status = active.to_string();
        *status_due_at_ms = None;
    }
}

/// One stored sequence store: every member `GetSequenceStore` reports.
#[derive(Debug, Clone, Default)]
pub struct SequenceStore {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The server-side encryption settings the create named, when it named any.
    pub sse_config: Option<SseConfig>,
    pub fallback_location: Option<String>,
    /// The ETag algorithm family in force, which is
    /// [`DEFAULT_E_TAG_ALGORITHM_FAMILY`] when the create named none.
    pub e_tag_algorithm_family: String,
    /// The tag keys that propagate to the objects of this store's read sets, exactly as
    /// the create listed them; `None` when it listed none.
    pub propagated_set_level_tags: Option<Vec<String>>,
    /// `s3AccessConfig$accessLogLocation` the create named, reported back as part of
    /// `s3Access`.
    pub access_log_location: Option<String>,
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time_ms: i64,
    /// When the store was last changed, in epoch millis; `None` until something
    /// changes it, since a create is not an update.
    pub update_time_ms: Option<i64>,
    /// When the transition this record owes the clock becomes due, in epoch millis;
    /// `None` once it has settled.
    pub status_due_at_ms: Option<i64>,
    pub tags: BTreeMap<String, String>,
}

impl SequenceStore {
    /// Apply the `CREATING` -> `ACTIVE` move the clock already owed this store by
    /// `now_ms`, under the same rule every other record settles by.
    pub fn settle(&mut self, now_ms: i64) {
        settle_creating(
            &mut self.status,
            &mut self.status_due_at_ms,
            now_ms,
            SEQUENCE_STORE_STATUS_ACTIVE,
        );
    }

    /// `s3Access` as the wire shapes want it.
    ///
    /// Only the access-log location the create named is reported. `s3Uri` and
    /// `s3AccessPointArn` name an S3 bucket and access point the real service
    /// provisions for the store; this emulator serves no object store and mints no
    /// access point, so there is no location a client could reach and the members are
    /// omitted rather than filled with an address nothing answers on. The structure
    /// itself is omitted when the create named no S3 access configuration.
    pub fn s3_access_output(&self) -> Option<SequenceStoreS3Access> {
        let access_log_location = self.access_log_location.clone()?;
        Some(SequenceStoreS3Access {
            s3_uri: None,
            s3_access_point_arn: None,
            access_log_location: Some(access_log_location),
        })
    }

    /// `tags` as the wire shapes want it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }
}

/// One stored run cache: every member `GetRunCache` reports.
///
/// A run cache is a reference to a caller-owned S3 location, not a resource this
/// emulator provisions, so every member below is either what the create carried or a
/// value derived from it and the id this emulator minted.
#[derive(Debug, Clone, Default)]
pub struct RunCache {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// The S3 location the create named, reported back as `cacheS3Uri`.
    pub cache_s3_uri: String,
    /// The default cache behavior in force, which is [`DEFAULT_CACHE_BEHAVIOR`] when
    /// the create named none.
    pub cache_behavior: String,
    /// The account that owns the cache bucket, which is this emulator's own account
    /// when the create named none.
    pub cache_bucket_owner_id: String,
    pub status: String,
    pub creation_time_ms: i64,
    pub tags: BTreeMap<String, String>,
}

impl RunCache {
    /// `tags` as the wire shapes want it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }
}

/// A parameter template as the wire shapes want it: omitted when it defines nothing.
fn parameter_template_output(
    parameter_template: &BTreeMap<String, WorkflowParameter>,
) -> Option<HashMap<String, WorkflowParameter>> {
    (!parameter_template.is_empty()).then(|| {
        parameter_template
            .iter()
            .map(|(name, parameter)| (name.clone(), parameter.clone()))
            .collect()
    })
}

/// `definitionRepositoryDetails` for the repository a create named.
///
/// `providerType` and `providerEndpoint` are reported by the real service from the
/// CodeConnections connection the ARN names; this emulator resolves no connection, so
/// it reports only what the request itself carried.
fn definition_repository_details(
    repository: Option<&DefinitionRepository>,
) -> Option<DefinitionRepositoryDetails> {
    let repository = repository?;
    Some(DefinitionRepositoryDetails {
        connection_arn: Some(repository.connection_arn.clone()),
        full_repository_id: Some(repository.full_repository_id.clone()),
        source_reference: repository.source_reference.clone(),
        provider_type: None,
        provider_endpoint: None,
    })
}

/// One stored run: every member `GetRun` reports.
#[derive(Debug, Clone, Default)]
pub struct Run {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    pub uuid: String,
    pub workflow_id: String,
    pub workflow_type: String,
    pub workflow_uuid: Option<String>,
    pub workflow_name: Option<String>,
    pub workflow_version_name: Option<String>,
    pub networking_mode: Option<String>,
    pub scratch_storage_mode: Option<String>,
    /// The run this one re-ran, when `StartRun` named one.
    pub source_run_id: Option<String>,
    pub role_arn: String,
    pub name: Option<String>,
    pub run_group_id: Option<String>,
    pub cache_id: Option<String>,
    pub cache_behavior: Option<String>,
    pub priority: Option<i32>,
    pub parameters: Option<::aws_smithy_types::Document>,
    pub engine_settings: Option<::aws_smithy_types::Document>,
    pub storage_capacity: Option<i32>,
    pub storage_type: String,
    pub output_uri: String,
    pub run_output_uri: String,
    pub log_level: Option<String>,
    pub retention_mode: String,
    pub accelerators: Option<String>,
    pub session_policy: Option<String>,
    /// The workflow's definition URI and digest, as of the moment the run started.
    pub definition: Option<String>,
    pub digest: Option<String>,
    pub status: String,
    pub status_message: Option<String>,
    pub failure_reason: Option<String>,
    pub creation_time_ms: i64,
    pub start_time_ms: Option<i64>,
    pub stop_time_ms: Option<i64>,
    /// When the next status in [`RUN_STATUS_SEQUENCE`] becomes due, in epoch millis;
    /// `None` once the run has reached a terminal status.
    pub status_due_at_ms: Option<i64>,
    pub tags: BTreeMap<String, String>,
}

impl Run {
    /// Apply every status change the clock already owed this run by `now_ms`.
    ///
    /// Each step's timestamp is the instant it was DUE, not the instant it was
    /// noticed, so `startTime` and `stopTime` are the same values whenever the run
    /// is next read and a caller that waits longer sees no later clock.
    pub fn settle(&mut self, now_ms: i64) {
        while let Some(due) = self.status_due_at_ms.filter(|due| *due <= now_ms) {
            let next = RUN_STATUS_SEQUENCE
                .iter()
                .position(|status| *status == self.status)
                .and_then(|at| RUN_STATUS_SEQUENCE.get(at + 1));
            let Some(next) = next else {
                self.status_due_at_ms = None;
                return;
            };
            self.status = (*next).to_string();
            if self.status == RUN_STATUS_RUNNING {
                self.start_time_ms = Some(due);
            }
            if is_terminal_run_status(&self.status) {
                self.stop_time_ms = Some(due);
                self.status_due_at_ms = None;
                // Nothing executed, so the terminal status says so rather than
                // claiming an outcome no engine produced.
                self.failure_reason = Some(RUN_NOT_EXECUTED_FAILURE_REASON.to_string());
                self.status_message = Some(RUN_NOT_EXECUTED_STATUS_MESSAGE.to_string());
            } else {
                self.status_due_at_ms = Some(due + TRANSITION_DELAY.as_millis() as i64);
            }
        }
    }

    /// `tags` as the wire shape wants it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }
}

/// One stored run group: every member `GetRunGroup` reports.
///
/// A run group carries no status member in the model — `CreateRunGroup` answers with the
/// group's ARN, id and tags and nothing is provisioned for it — so no record here owes
/// the clock a transition.
#[derive(Debug, Clone, Default)]
pub struct RunGroup {
    /// The decimal numeric id: this record's store key, rendered.
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    /// The caps the group enforces on the runs added to it, exactly as the request set
    /// them; a cap the request omitted is unset, which is the group not capping that
    /// resource at all rather than capping it at zero.
    pub max_cpus: Option<i32>,
    pub max_runs: Option<i32>,
    pub max_duration: Option<i32>,
    pub max_gpus: Option<i32>,
    pub creation_time_ms: i64,
    pub tags: BTreeMap<String, String>,
}

impl RunGroup {
    /// `tags` as the wire shapes want it: omitted when none were ever set.
    pub fn tags_output(&self) -> Option<HashMap<String, String>> {
        tags_output(&self.tags)
    }
}

/// Whether `status` is one a run stays in until something other than the clock moves
/// it, from [`TERMINAL_RUN_STATUSES`].
pub fn is_terminal_run_status(status: &str) -> bool {
    TERMINAL_RUN_STATUSES.contains(&status)
}

/// A tag map as the wire shapes want it: omitted when empty.
fn tags_output(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    (!tags.is_empty()).then(|| {
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
}

/// Which operation an idempotency token was presented to. The token namespace is
/// per-operation, so the same `requestId` on `CreateWorkflow` and on `StartRun` names
/// two different resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdempotencyScope {
    CreateWorkflow,
    StartRun,
    CreateSequenceStore,
    CreateRunCache,
    CreateRunGroup,
}

/// Per-service state for com-amazonaws-omics-omics. A handler reaches it through
/// [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsOmicsOmicsState {
    /// Every private workflow, keyed by its numeric id. The key order is creation
    /// order, which is what makes `ListWorkflows`' paging stable.
    pub workflows: BTreeMap<u64, Workflow>,
    /// Every run, keyed by its numeric id, in creation order for the same reason.
    pub runs: BTreeMap<u64, Run>,
    /// Every sequence store, keyed by its numeric id, in creation order for the same
    /// reason.
    pub sequence_stores: BTreeMap<u64, SequenceStore>,
    /// Every run cache, keyed by its numeric id, in creation order for the same reason.
    pub run_caches: BTreeMap<u64, RunCache>,
    /// Every run group, keyed by its numeric id, in creation order for the same reason.
    pub run_groups: BTreeMap<u64, RunGroup>,
    /// `(operation, requestId)` -> the numeric id of the resource that request
    /// created, so a replayed `@idempotencyToken` answers with the original resource
    /// instead of minting a second one.
    pub idempotency: HashMap<(IdempotencyScope, String), u64>,
    next_workflow_id: u64,
    next_run_id: u64,
    next_sequence_store_id: u64,
    next_run_cache_id: u64,
    next_run_group_id: u64,
}

impl ComAmazonawsOmicsOmicsState {
    /// Apply every status change the clock already owed any stored record by
    /// `now_ms`. Called at the top of each handler so no operation can observe a
    /// status the clock has already moved past.
    pub fn settle(&mut self, now_ms: i64) {
        for workflow in self.workflows.values_mut() {
            workflow.settle(now_ms);
        }
        for run in self.runs.values_mut() {
            run.settle(now_ms);
        }
        for store in self.sequence_stores.values_mut() {
            store.settle(now_ms);
        }
        // A run cache owes the clock nothing: no value of the modelled
        // `RunCacheStatus` enum is a transient "being created" state, so a cache is
        // usable from the moment its create answered and no transition is ever due.
    }

    /// The workflow the modelled `WorkflowId` `id` names, or `None`.
    pub fn workflow(&self, id: &str) -> Option<&Workflow> {
        self.workflows.get(&numeric_id(id)?)
    }

    /// The same record, for an operation that CHANGES the workflow or the versions it
    /// holds. One lookup rule for both, so a create and a read cannot disagree about
    /// which workflow an id names.
    pub fn workflow_mut(&mut self, id: &str) -> Option<&mut Workflow> {
        self.workflows.get_mut(&numeric_id(id)?)
    }

    /// The run the modelled `RunId` `id` names, or `None`.
    pub fn run(&self, id: &str) -> Option<&Run> {
        self.runs.get(&numeric_id(id)?)
    }

    /// The sequence store the modelled `SequenceStoreId` `id` names, or `None`.
    pub fn sequence_store(&self, id: &str) -> Option<&SequenceStore> {
        self.sequence_stores.get(&numeric_id(id)?)
    }

    /// The run cache the modelled `RunCacheId` `id` names, or `None`.
    pub fn run_cache(&self, id: &str) -> Option<&RunCache> {
        self.run_caches.get(&numeric_id(id)?)
    }

    /// The run group the modelled `RunGroupId` `id` names, or `None`.
    pub fn run_group(&self, id: &str) -> Option<&RunGroup> {
        self.run_groups.get(&numeric_id(id)?)
    }

    /// Mint the next workflow id, as the decimal string the model's pattern wants
    /// and the numeric key the store is ordered by.
    pub fn next_workflow_id(&mut self) -> (u64, String) {
        let key = FIRST_RESOURCE_ID + self.next_workflow_id;
        self.next_workflow_id += 1;
        (key, key.to_string())
    }

    /// Mint the next run id, the same way, from a counter offset by
    /// [`RUN_ID_OFFSET`] so no run shares an id with a workflow.
    pub fn next_run_id(&mut self) -> (u64, String) {
        let key = FIRST_RESOURCE_ID + RUN_ID_OFFSET + self.next_run_id;
        self.next_run_id += 1;
        (key, key.to_string())
    }

    /// Mint the next sequence store id, from [`FIRST_SEQUENCE_STORE_ID`] so every one
    /// is the ten-digit decimal the modelled `SequenceStoreId` length admits.
    pub fn next_sequence_store_id(&mut self) -> (u64, String) {
        let key = FIRST_SEQUENCE_STORE_ID + self.next_sequence_store_id;
        self.next_sequence_store_id += 1;
        (key, key.to_string())
    }

    /// Mint the next run cache id, the same way as a run's, from a counter offset by
    /// [`RUN_CACHE_ID_OFFSET`] so no cache shares an id with a workflow, a run or a run
    /// group. The modelled `RunCacheId` is `^[0-9]+$` with length 1..18, which every
    /// value this mints satisfies.
    pub fn next_run_cache_id(&mut self) -> (u64, String) {
        let key = FIRST_RESOURCE_ID + RUN_CACHE_ID_OFFSET + self.next_run_cache_id;
        self.next_run_cache_id += 1;
        (key, key.to_string())
    }

    /// Mint the next run group id, the same way, from a counter offset by
    /// [`RUN_GROUP_ID_OFFSET`] so no run group shares an id with a workflow or a run.
    /// The modelled `RunGroupId` is `^[0-9]+$` with length 1..18, which every value this
    /// mints satisfies.
    pub fn next_run_group_id(&mut self) -> (u64, String) {
        let key = FIRST_RESOURCE_ID + RUN_GROUP_ID_OFFSET + self.next_run_group_id;
        self.next_run_group_id += 1;
        (key, key.to_string())
    }

    /// The resource a replayed `requestId` already created, if this emulator still
    /// holds it. A token whose resource has since been deleted is forgotten, so the
    /// same `requestId` can create a fresh one — the service cannot answer with a
    /// resource that no longer exists.
    pub fn replayed(&mut self, scope: IdempotencyScope, request_id: &str) -> Option<u64> {
        let key = (scope, request_id.to_string());
        let id = *self.idempotency.get(&key)?;
        let still_held = match scope {
            IdempotencyScope::CreateWorkflow => self.workflows.contains_key(&id),
            IdempotencyScope::StartRun => self.runs.contains_key(&id),
            IdempotencyScope::CreateSequenceStore => self.sequence_stores.contains_key(&id),
            IdempotencyScope::CreateRunCache => self.run_caches.contains_key(&id),
            IdempotencyScope::CreateRunGroup => self.run_groups.contains_key(&id),
        };
        if still_held {
            return Some(id);
        }
        self.idempotency.remove(&key);
        None
    }

    /// Remember that `request_id` created the resource `id`.
    ///
    /// The map is capped: once it is full, entries whose resource is gone are dropped
    /// first, and a token is only remembered if that leaves room. Forgetting a token
    /// can only cause a duplicate create on a retry, never a wrong answer.
    pub fn remember(&mut self, scope: IdempotencyScope, request_id: String, id: u64) {
        if self.idempotency.len() >= MAX_IDEMPOTENCY_TOKENS {
            let workflows = &self.workflows;
            let runs = &self.runs;
            let sequence_stores = &self.sequence_stores;
            let run_caches = &self.run_caches;
            let run_groups = &self.run_groups;
            self.idempotency.retain(|key, id| match key.0 {
                IdempotencyScope::CreateWorkflow => workflows.contains_key(id),
                IdempotencyScope::StartRun => runs.contains_key(id),
                IdempotencyScope::CreateSequenceStore => sequence_stores.contains_key(id),
                IdempotencyScope::CreateRunCache => run_caches.contains_key(id),
                IdempotencyScope::CreateRunGroup => run_groups.contains_key(id),
            });
        }
        if self.idempotency.len() < MAX_IDEMPOTENCY_TOKENS {
            self.idempotency.insert((scope, request_id), id);
        }
    }
}

/// The numeric key a modelled `WorkflowId`/`RunId` names.
///
/// Both are `^[0-9]+$` with length 1..18, so every value the centralized walk admits
/// parses; an id outside that (which cannot reach a handler) answers `None` and is
/// then reported as "not found" rather than panicking. The modelled `SequenceStoreId`
/// is the same decimal with length 10..36, so an id wider than `u64` also answers
/// `None`: no store this emulator minted can carry one, so "not found" is the right
/// answer for it too.
pub fn numeric_id(id: &str) -> Option<u64> {
    id.parse::<u64>().ok()
}

/// The largest page either of this crate's listings will ever build, and the page size
/// applied when a request omits `maxResults`.
///
/// It is the maximum of the modelled `maxResults` range (1..100) that `ListRuns`,
/// `ListWorkflows` and `ListRunGroups` share, stated once here. Each handler clamps to
/// it, so the page it allocates is decided by the server, never by the request — the
/// handlers do not rely on the centralized walk having refused a larger value.
pub const MAX_LIST_PAGE_SIZE: usize = 100;

/// The tag a `ListRuns` page token carries, so a token issued by another listing is
/// refused rather than read as a resume point into the run store.
pub const RUN_PAGE_TOKEN_TAG: &str = "run_";

/// The same, for `ListWorkflows`.
pub const WORKFLOW_PAGE_TOKEN_TAG: &str = "wf_";

/// The same, for `ListRunGroups`.
pub const RUN_GROUP_PAGE_TOKEN_TAG: &str = "rg_";

/// The same, for `ListWorkflowVersions`. A version is keyed by its NAME inside its
/// workflow, so that listing's token names a name rather than a numeric id; the tag is
/// still distinct from every other, so no token can resume another listing's walk.
pub const WORKFLOW_VERSION_PAGE_TOKEN_TAG: &str = "wfv_";

/// A page token naming the record the NEXT page of a listing starts at, by that
/// record's own key.
///
/// One codec for every listing in this crate, so each mints and reads tokens the same
/// way. Every list token this service models is opaque, so the key is hex-encoded
/// rather than handed back in the clear — opaque to a caller, not secret: hex is
/// trivially reversible and the token carries nothing beyond a record key. `tag` names
/// the collection the token belongs to, so one listing's token cannot resume another's
/// walk.
///
/// Every modelled token member's pattern is
/// `^[\p{L}||\p{M}||\p{Z}||\p{S}||\p{N}||\p{P}]+$`, and every character this produces is
/// admitted: the hex digits are `\p{N}`/`\p{L}` and a tag's `_` is `\p{Pc}`, a `\p{P}`
/// subcategory. The result is the tag plus two hex characters per byte of the key, which
/// stays inside each member's modelled length: a numeric id is at most the 20 digits of
/// `u64::MAX` (40 characters, inside the 1..128 of `RunListToken` and
/// `WorkflowListToken`) and a `WorkflowVersionName` at most 64 (128 characters, inside
/// the 1..256 of `WorkflowVersionListToken`).
pub fn encode_name_page_token(tag: &str, key: &str) -> String {
    format!("{tag}{}", hex::encode(key))
}

/// The record key a page token tagged `tag` names, or `None` when the token is not one
/// THAT listing issued. The caller renders that as its own 400 refusal.
pub fn decode_name_page_token(tag: &str, token: &str) -> Option<String> {
    let body = token.strip_prefix(tag)?;
    let key = hex::decode(body)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    // A token that names no key at all resumes nothing, so it is not one this service
    // issued either.
    (!key.is_empty()).then_some(key)
}

/// The same token, for a listing whose records are keyed by the numeric id
/// [`numeric_id`] reads.
pub fn encode_page_token(tag: &str, id: u64) -> String {
    encode_name_page_token(tag, &id.to_string())
}

/// The record id a page token tagged `tag` names, or `None` when the token is not one
/// THAT listing issued. The caller renders that as its own 400 refusal.
pub fn decode_page_token(tag: &str, token: &str) -> Option<u64> {
    decode_name_page_token(tag, token)
        .as_deref()
        .and_then(numeric_id)
}

/// The ARN of the workflow `id` in `region`.
pub fn workflow_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{WORKFLOW_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the version `version_name` of the workflow `workflow_id` in `region`.
///
/// The version is named inside its workflow's own resource path, so one ARN names the
/// pair and no version ARN can collide with a workflow ARN. The result is well inside
/// the modelled `WorkflowVersionArn` length (1..150): the account and region are
/// fixed, the workflow id is at most 18 characters and the name at most 64.
pub fn workflow_version_arn(region: &str, workflow_id: &str, version_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{WORKFLOW_ARN_RESOURCE_PREFIX}{workflow_id}{WORKFLOW_VERSION_ARN_INFIX}{version_name}"
        ),
    )
}

/// The ARN of the run `id` in `region`.
pub fn run_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{RUN_ARN_RESOURCE_PREFIX}{id}"))
}

/// The ARN of the run cache `id` in `region`.
///
/// Well inside the modelled `RunCacheArn` length (1..128): the account and region are
/// fixed and the id is at most 18 characters.
pub fn run_cache_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RUN_CACHE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the run group `id` in `region`.
///
/// Well inside the modelled `RunGroupArn` length (1..128): the account and region are
/// fixed and the id is at most 18 characters.
pub fn run_group_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RUN_GROUP_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the sequence store `id` in `region`.
///
/// Well inside the modelled `SequenceStoreArn` length (1..127): the account and region
/// are fixed and the id is at most 36 characters.
pub fn sequence_store_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{SEQUENCE_STORE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// Where this run's outputs land: the caller's `outputUri` with the run's own id
/// appended, so two runs writing to one `outputUri` do not collide.
///
/// Derived from the request and the minted id — nothing here is invented per run.
pub fn run_output_uri(output_uri: &str, id: &str) -> String {
    format!("{}/{id}", output_uri.trim_end_matches('/'))
}

/// Mixed into every [`fresh_uuid`] so two calls in the same nanosecond still differ.
static UUID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A fresh UUID in the form the modelled `WorkflowUuid` pattern describes
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`).
///
/// The 16 bytes come from SHA-256 (the `sha2` crate — no hashing is written here) over a
/// seed no other call can repeat: a monotonically increasing per-process counter, the
/// wall clock and the process id. Bytes 6 and 8 then carry the RFC 4122 version (4) and
/// variant (binary 10) nibbles, so the result is a well-formed v4-shaped UUID rather
/// than 16 arbitrary bytes, and the five groups are `hex`-encoded (the `hex` crate).
///
/// A UUID is used here to NAME a resource, not to authenticate anything, so a value
/// derived this way is the right strength: it is unique and unpredictable enough to
/// identify a workflow or a run, and it is never a credential.
pub fn fresh_uuid() -> String {
    use sha2::Digest;
    let sequence = UUID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut hasher = sha2::Sha256::new();
    hasher.update(sequence.to_be_bytes());
    hasher.update(now_epoch_millis().to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The content digest of a workflow definition, or `None` when this emulator never saw
/// the definition's content.
///
/// SHA-256 over the ZIP archive the create carried, rendered as the 64 lowercase hex
/// characters that exactly fill the modelled `WorkflowDigest` length (1..64) — the same
/// value a client gets by digesting the archive it uploaded.
///
/// A create that named only a `definitionUri` (or a `definitionRepository`) gets NO
/// digest: this emulator serves no S3 object store and no code repository, so it never
/// read the archive and has nothing to digest. Digesting the URI string instead would be
/// indistinguishable on the wire from a content digest, and a client comparing it with
/// its own hash of the archive would see a plausible but wrong mismatch — so the member
/// is omitted rather than filled with something nothing computed from content. A later
/// slice that really reads the object must digest the bytes it read.
pub fn definition_digest(definition_zip: Option<&[u8]>) -> Option<String> {
    use sha2::Digest;
    let zip = definition_zip?;
    Some(hex::encode(sha2::Sha256::digest(zip)))
}

/// Whether `account_id` names the one account this emulator serves
/// (`engine::emulator_account_id()`).
///
/// A `workflowOwnerId` naming any other account asks for a workflow SHARED from that
/// account. Cross-account sharing is its own slice (`CreateShare`/`AcceptShare` are still
/// 501 in this crate), so no such workflow can be resolved and the request is answered
/// "not found" rather than silently served from this account's own store.
pub fn is_emulator_account(account_id: &str) -> bool {
    account_id == ::engine::emulator_account_id()
}

/// The parameter names `parameters` carries that the workflow's `parameterTemplate` does
/// not define, rendered for a refusal message; `None` when there are none.
///
/// From `StartRun`'s own documentation: "The run cannot include any parameters that are
/// not defined in the parameter template." A workflow that declared NO template
/// constrains nothing — HealthOmics parses the template from the definition file in that
/// case, which this slice does not read — so this rule has nothing to say about it. A
/// `parameters` document that is not an object names no parameters at all.
pub fn undefined_parameters(
    parameter_template: &BTreeMap<String, WorkflowParameter>,
    parameters: Option<&::aws_smithy_types::Document>,
) -> Option<String> {
    if parameter_template.is_empty() {
        return None;
    }
    let ::aws_smithy_types::Document::Object(supplied) = parameters? else {
        return None;
    };
    let undefined: Vec<&str> = supplied
        .keys()
        .filter(|name| !parameter_template.contains_key(name.as_str()))
        .map(String::as_str)
        .collect();
    (!undefined.is_empty()).then(|| undefined.join(", "))
}

/// Why the caller-sized collections on a create or start request are unacceptable,
/// or `None` when each is within the store's bounds.
///
/// The model publishes no length for the `tags` map, so the centralized walk cannot
/// refuse an oversized one and this is where the bound lives. Every operation that
/// stores tags applies this one rule, each rendering the refusal as its own modelled
/// `ValidationException`.
pub fn tag_bounds_rejection(tags: Option<&HashMap<String, String>>) -> Option<String> {
    let count = tags.map_or(0, HashMap::len);
    (count > MAX_TAGS).then(|| {
        format!("tags carries {count} entries, more than the {MAX_TAGS} this service accepts")
    })
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN, so
/// anything that is not region-shaped is refused here and the default used instead:
/// a generated ARN can never carry text the modelled `^arn:.+$` would not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, in milliseconds since the Unix epoch — the resolution this service's
/// `date-time` timestamps are stored and compared at.
///
/// A host clock set before 1970 is the only way `duration_since` fails. It is rendered as
/// the NEGATIVE offset rather than collapsed to 0: every reading then stays ordered with
/// every other, so a due instant is still in the future of the record that owes it,
/// instead of every owed transition becoming instantly due.
pub fn now_epoch_millis() -> i64 {
    let clamp = |millis: u128| millis.min(i64::MAX as u128) as i64;
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since_epoch) => clamp(since_epoch.as_millis()),
        Err(before_epoch) => -clamp(before_epoch.duration().as_millis()),
    }
}

/// An epoch-millis instant as the `date-time` timestamp the model binds.
pub fn timestamp(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Emit the structured access record for one call to this service.
///
/// Every operation in this slice calls it exactly once, on the branch it returns from —
/// above all the four that CHANGE the store (`CreateWorkflow`, `StartRun`, `DeleteRun`,
/// `DeleteWorkflow`), whose effect on it is otherwise invisible after the fact. The
/// record carries the operation, the identity the request PRESENTED (its SigV4 access key
/// id, which is an identifier and not a secret), the request target it named, the
/// resource the call resolved when it resolved one, and the outcome; the `tracing`
/// subscriber supplies the timestamp.
///
/// There is no network peer in the record because a Smithy handler is not given one:
/// [`smithy_server_runtime::OperationContext::parts`] carries the request's method, URI,
/// headers and extensions, and nothing in this framework puts a peer address in them. The
/// presented credential is therefore what identifies the caller.
///
/// Deliberately metadata only: no tag, run parameter, workflow definition, output URI or
/// idempotency token is logged, so a caller's own data never reaches the log. Every
/// interpolated value is request-derived, so [`audit_field`] bounds it and replaces the
/// control characters that would otherwise let one request forge a second record.
pub fn audit(
    operation: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    outcome: AuditOutcome,
    resource: Option<&str>,
) {
    let parts = ctx.parts();
    ::tracing::info!(
        target: "emulator::audit",
        service = ctx.identity().signing_name(),
        operation,
        access_key_id = %audit_field(&::engine::extract_akid(&parts.headers)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        resource = %audit_field(resource.unwrap_or(AUDIT_NO_RESOURCE)),
        outcome = outcome.as_str(),
        "omics api call"
    );
}

/// What the `resource` field reads as when the call resolved no resource (a refusal
/// before any lookup, or a listing, which names none).
pub const AUDIT_NO_RESOURCE: &str = "-";

/// Longest request-derived value one audit field carries.
///
/// A SigV4 access key id, a request path and a resource id are all caller-controlled, so
/// an unbounded echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 128;

/// A request-derived value as it may appear in an audit record: control characters (above
/// all CR and LF, which would forge a second record) replaced, and the whole value
/// bounded by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '.' } else { c })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// A private workflow was minted and stored.
    WorkflowCreated,
    /// A stored workflow was removed from the store.
    WorkflowDeleted,
    /// A version was minted and stored against a stored workflow.
    WorkflowVersionCreated,
    /// A stored workflow version was removed from its workflow.
    WorkflowVersionDeleted,
    /// A run was minted and stored against a stored workflow.
    RunStarted,
    /// A stored run was removed from the store.
    RunDeleted,
    /// A sequence store was minted and stored.
    SequenceStoreCreated,
    /// A run cache was minted and stored.
    RunCacheCreated,
    /// A run group was minted and stored.
    RunGroupCreated,
    /// A replayed `@idempotencyToken` changed nothing and answered with the resource the
    /// first call created.
    NoChange,
    /// A reporting operation answered from state and changed nothing beyond the status
    /// transitions the clock already owed.
    Reported,
    /// The request was refused as invalid (400).
    ValidationRefused,
    /// The named resource does not exist, or is not one this emulator holds (404).
    NotFound,
    /// The request cannot be applied to the resource in its current state (409).
    Conflict,
    /// A quota — the service's or this emulator's own ceiling — was met (402).
    QuotaExceeded,
    /// This service's own store was unreachable (500).
    StoreUnavailable,
    /// The request asked for behaviour this emulator does not implement (501).
    NotImplemented,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkflowCreated => "workflow-created",
            Self::WorkflowDeleted => "workflow-deleted",
            Self::WorkflowVersionCreated => "workflow-version-created",
            Self::WorkflowVersionDeleted => "workflow-version-deleted",
            Self::RunStarted => "run-started",
            Self::RunDeleted => "run-deleted",
            Self::SequenceStoreCreated => "sequence-store-created",
            Self::RunCacheCreated => "run-cache-created",
            Self::RunGroupCreated => "run-group-created",
            Self::NoChange => "no-change",
            Self::Reported => "reported",
            Self::ValidationRefused => "validation-refused",
            Self::NotFound => "not-found",
            Self::Conflict => "conflict",
            Self::QuotaExceeded => "quota-exceeded",
            Self::StoreUnavailable => "store-unavailable",
            Self::NotImplemented => "not-implemented",
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOmicsOmicsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOmicsOmicsState>)
}

// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workspacesinstances-eucmifrontendapiservice service.
//!
//! The WorkSpace Instance lifecycle is backed here: every value an operation
//! returns is read out of one of these records, never built from the request.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    BillingConfiguration, ManagedInstanceRequest, ResourceNotFoundException,
    ServiceQuotaExceededException, Tag,
};

/// The `ResourceType` this service reports for a WorkSpace Instance in its
/// resource-scoped errors.
pub const WORKSPACE_INSTANCE_RESOURCE_TYPE: &str = "workspace-instance";

/// The `ServiceCode` this service reports in its quota errors: the service's own
/// code, which is also its sigv4 signing name and ARN namespace.
pub const SERVICE_CODE: &str = "workspaces-instances";

/// The `QuotaCode` for the running-instance quota below. AWS publishes no code for
/// it, so this is the emulator's own stable identifier for the quota it enforces.
pub const WORKSPACE_INSTANCE_QUOTA_CODE: &str = "L-WorkSpaceInstancesPerAccount";

/// The number of WorkSpace Instances this emulator will hold at once.
///
/// A real account has a Service Quotas limit on running WorkSpace Instances, and
/// `CreateWorkspaceInstance` models `ServiceQuotaExceededException` (HTTP 402) for
/// reaching it. AWS publishes no figure, so the number itself is CHOSEN: its purpose
/// is that the store cannot grow without bound, and that it is reached through the
/// modelled error rather than by exhausting memory. Capacity is released by
/// `DeleteWorkspaceInstance`, as on the real service.
pub const MAX_INSTANCES: usize = 500;

/// The number of tags one WorkSpace Instance holds.
///
/// This is MODELLED, not chosen: `ListTagsForResourceResponse$Tags` is a `TagList`
/// with `length 0..50`, so a stored set larger than this could not be reported at
/// all (`smithy_query(query="shape",shape="TagList")`). `TagResource` refuses a
/// request that would leave the resource above it.
pub const MAX_TAGS_PER_INSTANCE: usize = 50;

/// `ProvisionStateEnum` members this slice uses. The full value list is
/// `crate::ops::types::PROVISION_STATE_ENUM_VALUES`, generated from the model
/// (`smithy_query(query="shape",shape="ProvisionStateEnum")`).
pub const PROVISION_STATE_ALLOCATING: &str = "ALLOCATING";
/// See [`PROVISION_STATE_ALLOCATING`].
pub const PROVISION_STATE_ALLOCATED: &str = "ALLOCATED";

/// How long after `CreateWorkspaceInstance` the `ALLOCATING -> ALLOCATED`
/// transition the call starts becomes due.
///
/// AWS documents no duration for it, so this is a CHOSEN span: it is deliberately
/// non-zero, so a caller that reads the instance back immediately still observes
/// the transient `ALLOCATING` state, and short enough that a test can wait it out.
/// The due instant is stored on the record ([`WorkspaceInstanceRecord::allocation_due`])
/// and materialised lazily by [`WorkspaceInstanceRecord::settle`]; nothing sleeps.
pub const ALLOCATION_SPAN: Duration = Duration::from_millis(500);

/// The `BillingMode` an instance created without a `BillingConfiguration` runs on.
///
/// From the model's own doc for `CreateWorkspaceInstance$BillingConfiguration`:
/// "Defaults to hourly billing if not specified." `GetWorkspaceInstance` reports the
/// ACTIVE billing configuration, so the default is materialised at create time rather
/// than left absent.
pub const DEFAULT_BILLING_MODE: &str = "HOURLY";

/// The default page size `ListWorkspaceInstances` applies when the caller sends no
/// `MaxResults`: the modelled maximum for that member (`range 1..25`).
pub const DEFAULT_MAX_RESULTS: usize = 25;

/// One WorkSpace Instance, as created by `CreateWorkspaceInstance`.
#[derive(Debug, Clone)]
pub struct WorkspaceInstanceRecord {
    /// The minted `wsinst-…` id, matching `^wsinst-[0-9a-zA-Z]{8,63}$`.
    pub id: String,
    /// A `ProvisionStateEnum` value — see [`PROVISION_STATE_ALLOCATING`].
    pub provision_state: String,
    /// The `ManagedInstance` configuration exactly as supplied at create time. Held
    /// for the operations that report it (the volume and instance-type slices still
    /// at 501); nothing in this slice returns it.
    pub managed_instance: ManagedInstanceRequest,
    /// The minted `i-…` id of the EC2 instance WorkSpaces manages for this record.
    pub ec2_instance_id: String,
    /// The instance's ACTIVE billing configuration: the one supplied at create time,
    /// or [`DEFAULT_BILLING_MODE`] when the create call supplied none.
    pub billing_configuration: BillingConfiguration,
    /// Tags, keyed by `Tag.Key` so a later `TagResource` with the same key overwrites.
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` the creating call carried, if any.
    pub client_token: Option<String>,
    /// When the record was created. Held for the operations that report or order by
    /// it; nothing in this slice returns it.
    pub created_at: SystemTime,
    /// The instant the `ALLOCATING -> ALLOCATED` transition falls due. Monotonic,
    /// so a wall-clock adjustment cannot make a settled record un-settle.
    pub allocation_due: Instant,
}

impl WorkspaceInstanceRecord {
    /// Materialise the transition the clock has already made due.
    ///
    /// Called from reads as well as writes: the move to `ALLOCATED` was owed at
    /// `allocation_due` whether or not anybody called, so applying it on read
    /// reports the state the service would already be in, and never changes what
    /// a later caller sees beyond that.
    pub fn settle(&mut self, now: Instant) {
        if self.provision_state == PROVISION_STATE_ALLOCATING && now >= self.allocation_due {
            self.provision_state = PROVISION_STATE_ALLOCATED.to_string();
        }
    }
}

/// Per-service state for com-amazonaws-workspacesinstances-eucmifrontendapiservice. A handler reaches it through [`service_state`].
///
/// `instances` is a `BTreeMap` rather than a hash map so iteration order is the
/// id order, which is what makes `ListWorkspaceInstances`' `NextToken` resume at a
/// well-defined place: ids are minted from `id_counter` zero-padded, so id order
/// is creation order.
#[derive(Debug, Default)]
pub struct ComAmazonawsWorkspacesinstancesEucmifrontendapiserviceState {
    /// Every live WorkSpace Instance, keyed by its `wsinst-…` id.
    pub instances: BTreeMap<String, WorkspaceInstanceRecord>,
    /// `ClientToken` -> `WorkspaceInstanceId`, for `@idempotent` create replay.
    pub client_tokens: BTreeMap<String, String>,
    /// Source of the unique `wsinst-…` / `i-…` identifiers.
    pub id_counter: u64,
}

impl ComAmazonawsWorkspacesinstancesEucmifrontendapiserviceState {
    /// Mint the next pair of identifiers: a `wsinst-…` instance id and the `i-…`
    /// id of the EC2 instance behind it.
    ///
    /// The `wsinst-` prefix is 7 characters and the model requires 8..63 further
    /// `[0-9a-zA-Z]`, so a 16-hex-digit counter gives a 23-character id inside the
    /// modelled `len 15..70`. EC2 instance ids are `i-` plus 17 hex digits.
    pub fn mint_ids(&mut self) -> (String, String) {
        self.id_counter += 1;
        let n = self.id_counter;
        (format!("wsinst-{n:016x}"), format!("i-{n:017x}"))
    }

    /// Apply every clock-due transition, then hand back the record for `id`.
    pub fn settled(&mut self, id: &str, now: Instant) -> Option<&WorkspaceInstanceRecord> {
        let record = self.instances.get_mut(id)?;
        record.settle(now);
        Some(record)
    }

    /// Apply every clock-due transition across the whole store.
    pub fn settle_all(&mut self, now: Instant) {
        for record in self.instances.values_mut() {
            record.settle(now);
        }
    }

    /// Whether another instance fits inside [`MAX_INSTANCES`].
    pub fn has_instance_capacity(&self) -> bool {
        self.instances.len() < MAX_INSTANCES
    }
}

/// Whether `candidate` is shaped like a `WorkspaceInstanceId`: `len 15..70` and
/// `^wsinst-[0-9a-zA-Z]{8,63}$` (`smithy_query(query="shape",shape="WorkspaceInstanceId")`).
///
/// The boundary validator already enforces this on every modelled member; this is for
/// a value the service decodes itself, such as the resume point inside a `NextToken`.
pub fn is_workspace_instance_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("wsinst-") else {
        return false;
    };
    (15..=70).contains(&candidate.len())
        && (8..=63).contains(&suffix.len())
        && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The modelled `ServiceQuotaExceededException` (HTTP 402) for a create that would
/// pass [`MAX_INSTANCES`]. Every member the model marks `@required` is filled.
///
/// The quota is account-scoped and no instance was created, so `ResourceId` names the
/// subject the quota applies to — this emulator's account.
pub fn instance_quota_exceeded(held: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "the account already holds {held} WorkSpace Instances, the maximum for this quota"
        ),
        resource_id: ::engine::emulator_account_id().to_string(),
        resource_type: WORKSPACE_INSTANCE_RESOURCE_TYPE.to_string(),
        service_code: SERVICE_CODE.to_string(),
        quota_code: WORKSPACE_INSTANCE_QUOTA_CODE.to_string(),
    }
}

/// The modelled `ResourceNotFoundException` (HTTP 404) for a WorkSpace Instance id
/// that names no stored record. Every member the model marks `@required` is filled.
pub fn instance_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("WorkSpace Instance {id} does not exist"),
        resource_id: id.to_string(),
        resource_type: WORKSPACE_INSTANCE_RESOURCE_TYPE.to_string(),
    }
}

/// The persisted tags of a record, as the modelled `TagList`.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: Some(key.clone()),
            value: Some(value.clone()),
        })
        .collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWorkspacesinstancesEucmifrontendapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWorkspacesinstancesEucmifrontendapiserviceState>)
}

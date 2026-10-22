// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workspacesinstances-eucmifrontendapiservice service.
//!
//! The WorkSpace Instance lifecycle is backed here: every value an operation
//! returns is read out of one of these records, never built from the request.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    BillingConfiguration, InstanceTypeInfo, ManagedInstanceRequest, ResourceNotFoundException,
    ServiceQuotaExceededException, SupportedInstanceConfiguration, Tag, ValidationException,
    ValidationExceptionField, BILLING_MODE_VALUES, INSTANCE_CONFIGURATION_TENANCY_ENUM_VALUES,
    PLATFORM_TYPE_ENUM_VALUES,
};

/// The `ResourceType` this service reports for a WorkSpace Instance in its
/// resource-scoped errors.
///
/// `ResourceNotFoundException$ResourceType` and
/// `ServiceQuotaExceededException$ResourceType` are free `String` members — the
/// model enumerates no token for them (`ResourceTypeEnum` is the EC2-style tag
/// specification enum, nothing to do with these errors), so the token form is
/// RECORDED behaviour: the wire corpus answers `WORKSPACE_INSTANCE` for a missing
/// WorkSpace Instance. A client switches on this field, so the token is the one the
/// service really sends: upper case, underscore separated.
pub const WORKSPACE_INSTANCE_RESOURCE_TYPE: &str = "WORKSPACE_INSTANCE";

/// The HTTP status this service answers `ResourceNotFoundException` with.
///
/// The error SHAPE carries `@httpError 404`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), but the
/// service OVERRIDES it on the WorkSpace Instance paths: the wire recordings for an
/// unknown `wsinst-…` id answer HTTP 400 carrying that same shape, its members and
/// its `__type`. A client branches on the status as well as the code, so the pair it
/// sees has to be the pair the service sends — hence the status is stated here and
/// applied through [`crate::scaffolded::Scaffolded::ModeledWithStatus`], leaving the
/// shape, its members and its wire code the model's.
pub const RESOURCE_NOT_FOUND_STATUS: u16 = 400;

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

/// The default page size `ListInstanceTypes` applies when the caller sends no
/// `MaxResults`: the modelled maximum for THAT member, which is a different shape
/// (`ListInstanceTypesMaxResults`, `range 1..600`) from the one above.
pub const DEFAULT_INSTANCE_TYPES_MAX_RESULTS: usize = 600;

/// The `ValidationExceptionReason` this service reports when one member of a request
/// is wrong. A value of the modelled enum
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`, mirrored by the
/// generated [`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`]).
pub const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

// ---------------------------------------------------------------------------
// The instance-type catalog
//
// Beside the per-account store above, this service publishes one fixed catalog: the
// instance types WorkSpaces Instances supports, each with the configuration
// combinations it supports. That catalog is NOT per-account state — no operation
// creates, changes or deletes an instance type — so it lives here as data rather than
// in the store, and `ListInstanceTypes` projects it. Two emulators built from this
// source answer the same catalog, and reading it changes nothing.
// ---------------------------------------------------------------------------

/// The `BillingMode` values every catalogued instance type supports.
///
/// A WorkSpace Instance's billing mode is chosen per INSTANCE, not per instance type
/// (`CreateWorkspaceInstance$BillingConfiguration`: "Defaults to hourly billing if not
/// specified"), so every row of [`INSTANCE_TYPE_CATALOG`] supports both values. The
/// list is the model's own, reached through the generated constant so it cannot drift
/// from `smithy_query(query="shape",shape="BillingMode")`.
pub const CATALOG_BILLING_MODES: &[&str] = BILLING_MODE_VALUES;

/// Every platform type the model declares
/// (`smithy_query(query="shape",shape="PlatformTypeEnum")`), as the generated constant.
/// The families whose rows name this list run every WorkSpaces platform image.
pub const ALL_PLATFORM_TYPES: &[&str] = PLATFORM_TYPE_ENUM_VALUES;

/// The platform types this catalog records for the GPU families.
///
/// A subset of [`ALL_PLATFORM_TYPES`], in that list's order. WHICH platforms a GPU
/// family carries images for is published by AWS in prose (its "supported instance
/// types" / bundle documentation for the WorkSpaces family — refresh the subset from
/// there), not in the model, so the subset itself is this emulator's catalog data: its
/// purpose is that `InstanceConfigurationFilter` genuinely selects — a platform a
/// family has no image for reports the families that do, rather than every row of the
/// catalog.
pub const GPU_PLATFORM_TYPES: &[&str] = &[
    "Linux/UNIX",
    "Ubuntu Pro Linux",
    "Windows",
    "Windows BYOL",
];

/// The `InstanceConfigurationTenancyEnum` value for a multi-tenant instance
/// (`smithy_query(query="shape",shape="InstanceConfigurationTenancyEnum")`).
pub const TENANCY_SHARED: &str = "SHARED";

/// Both tenancies the model declares, as the generated constant: what a family AWS
/// also offers as a Dedicated Instance supports.
pub const SHARED_AND_DEDICATED_TENANCY: &[&str] = INSTANCE_CONFIGURATION_TENANCY_ENUM_VALUES;

/// Shared tenancy only: the tenancy set this catalog records for the burstable
/// (`t`-prefixed) families, which it does not offer on dedicated hardware. Like
/// [`GPU_PLATFORM_TYPES`] this is catalog data, not a modelled fact.
pub const SHARED_TENANCY_ONLY: &[&str] = &[TENANCY_SHARED];

/// One row of the instance-type catalog: an instance type, and the dimensions along
/// which it supports a configuration.
///
/// The row stores the DIMENSIONS, not the combinations: a
/// `SupportedInstanceConfiguration` is "a single valid configuration combination that
/// an instance type supports, combining tenancy, platform type, and billing mode"
/// (`smithy_query(query="doc",shape="SupportedInstanceConfiguration")`), so the
/// combinations a row supports are the cross product of its platform types, its
/// tenancies and [`CATALOG_BILLING_MODES`] — see
/// [`InstanceTypeSpec::supported_configurations`].
#[derive(Debug, Clone, Copy)]
pub struct InstanceTypeSpec {
    /// The EC2 instance type name, inside the modelled `InstanceType` (`len 1..64`,
    /// `^([a-z0-9-]+)\.([a-z0-9]+)$`).
    pub instance_type: &'static str,
    /// The `PlatformTypeEnum` values this instance type carries images for.
    pub platform_types: &'static [&'static str],
    /// The `InstanceConfigurationTenancyEnum` values this instance type can run under.
    pub tenancies: &'static [&'static str],
}

impl InstanceTypeSpec {
    /// Whether this instance type supports the configuration combination of
    /// `billing_mode`, `platform_type` and `tenancy` — the question
    /// `InstanceConfigurationFilter` asks.
    pub fn supports(&self, billing_mode: &str, platform_type: &str, tenancy: &str) -> bool {
        CATALOG_BILLING_MODES.contains(&billing_mode)
            && self.platform_types.contains(&platform_type)
            && self.tenancies.contains(&tenancy)
    }

    /// Every configuration combination this instance type supports, as the modelled
    /// `SupportedInstanceConfigurations`: the cross product of the row's platform
    /// types, its tenancies and [`CATALOG_BILLING_MODES`], in that nesting order so
    /// the list is stable across calls.
    pub fn supported_configurations(&self) -> Vec<SupportedInstanceConfiguration> {
        let mut configurations = Vec::with_capacity(
            self.platform_types.len() * self.tenancies.len() * CATALOG_BILLING_MODES.len(),
        );
        for platform_type in self.platform_types {
            for tenancy in self.tenancies {
                for billing_mode in CATALOG_BILLING_MODES {
                    configurations.push(SupportedInstanceConfiguration {
                        billing_mode: Some((*billing_mode).to_string()),
                        platform_type: Some((*platform_type).to_string()),
                        tenancy: Some((*tenancy).to_string()),
                    });
                }
            }
        }
        configurations
    }

    /// This row as the modelled `InstanceTypeInfo` `ListInstanceTypes` reports.
    pub fn info(&self) -> InstanceTypeInfo {
        InstanceTypeInfo {
            instance_type: Some(self.instance_type.to_string()),
            supported_instance_configurations: Some(self.supported_configurations()),
        }
    }
}

/// The instance types WorkSpaces Instances supports, ordered by instance type name.
///
/// MEMBERSHIP is this emulator's catalog: AWS publishes the supported families in its
/// documentation rather than in the model, which constrains only the SHAPE of a name
/// (`InstanceType`: `len 1..64`, `^([a-z0-9-]+)\.([a-z0-9]+)$`). The rows cover the
/// general-purpose, burstable, compute-optimised, memory-optimised and GPU families
/// at several sizes each, so the catalog spans every dimension a caller can filter on.
/// The ORDER is the name order, which is what makes `NextToken` resume at a
/// well-defined place, and it is fixed, so a page boundary is reproducible.
pub const INSTANCE_TYPE_CATALOG: &[InstanceTypeSpec] = &[
    InstanceTypeSpec {
        instance_type: "c5.2xlarge",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "c5.large",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "c5.xlarge",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "g4dn.2xlarge",
        platform_types: GPU_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "g4dn.xlarge",
        platform_types: GPU_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "g6.xlarge",
        platform_types: GPU_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "m5.2xlarge",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "m5.large",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "m5.xlarge",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "m6i.large",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "m6i.xlarge",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "r5.2xlarge",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "r5.large",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "r5.xlarge",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceTypeSpec {
        instance_type: "t3.large",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_TENANCY_ONLY,
    },
    InstanceTypeSpec {
        instance_type: "t3.medium",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_TENANCY_ONLY,
    },
    InstanceTypeSpec {
        instance_type: "t3.small",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_TENANCY_ONLY,
    },
    InstanceTypeSpec {
        instance_type: "t3.xlarge",
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_TENANCY_ONLY,
    },
];

/// The position of `instance_type` in [`INSTANCE_TYPE_CATALOG`], if the catalog holds
/// it. This is how a `ListInstanceTypes` `NextToken` resolves to a resume point, so a
/// token naming no catalogued instance type is refused rather than guessed at.
pub fn instance_type_index(instance_type: &str) -> Option<usize> {
    INSTANCE_TYPE_CATALOG
        .iter()
        .position(|spec| spec.instance_type == instance_type)
}

/// The AWS Regions this emulator reports from `ListRegions` as supporting
/// WorkSpaces Instances, in ascending `RegionName` order.
///
/// SOURCE: AWS's published Region availability for the Amazon WorkSpaces family in
/// the `aws` (commercial) partition — the Region table behind "Amazon WorkSpaces
/// Instances endpoints and quotas". The model itself enumerates nothing: `Region`
/// carries a free `RegionName` string (`^[-0-9a-z]{1,31}$`) and the service's
/// endpoint rule set is partition-templated
/// (`smithy_query(query="shape",shape="Region")`), so there is no modelled list to
/// project. Membership is era- and partition-dependent at AWS — a Region that gets
/// the service is added there — so, exactly as the catalog in the Backup slice does,
/// this list is a set this emulator OWNS: it is the Regions this emulator supports,
/// and `ListRegions` reports it rather than inventing one of its own. Regions in the
/// GovCloud and China partitions are deliberately absent: this emulator serves the
/// commercial partition.
///
/// Kept SORTED and free of duplicates: `ListRegions` reports it in this order, whole,
/// so the order a caller observes is this one.
pub const SUPPORTED_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ca-central-1",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "il-central-1",
    "sa-east-1",
    "us-east-1",
    "us-west-2",
];

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

/// The `Message` this service states for a WorkSpace Instance that names no stored
/// record.
///
/// A FIXED sentence, not built from the request: the service states that the
/// resource kind was not found and leaves the identity to the modelled
/// `ResourceId` member, which carries the id a client needs. Echoing the submitted
/// id into the prose as well would make the message vary per request for no
/// information a structured member does not already carry — the same reason this
/// service's validation messages name the member and the constraint without the
/// rejected value.
pub const INSTANCE_NOT_FOUND_MESSAGE: &str = "Workspace instance is not found";

/// The modelled `ResourceNotFoundException` for a WorkSpace Instance id that names
/// no stored record. Every member the model marks `@required` is filled: the
/// request-independent [`INSTANCE_NOT_FOUND_MESSAGE`], the id that was looked up,
/// and this service's [`WORKSPACE_INSTANCE_RESOURCE_TYPE`] token.
pub fn instance_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: INSTANCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: id.to_string(),
        resource_type: WORKSPACE_INSTANCE_RESOURCE_TYPE.to_string(),
    }
}

/// Encode a resume point into the `NextToken` a paginated operation hands back: the
/// exact inverse of [`decode_next_token`], so a token this service mints always
/// decodes on the follow-up request. Mint every token through this.
pub fn encode_next_token(resume_point: &str) -> String {
    hex::encode(resume_point)
}

/// Decode a `NextToken` this service issued back to the resume point it carries.
///
/// This service's one pagination-token encoding: the resume point's own identifier,
/// hex-encoded. Hex carries no secrecy — it is trivially reversible — it is a
/// round-trip and validation device, so a token that does not decode (or decodes to
/// a resume point the caller's list cannot name) is refused with
/// [`invalid_next_token`] rather than guessed at. Both paginated operations that mint
/// a token use this, so the scheme lives in one place.
pub fn decode_next_token(token: &str) -> Option<String> {
    hex::decode(token)
        .ok()
        .and_then(|raw| String::from_utf8(raw).ok())
}

/// The modelled `ValidationException` (HTTP 400) for a `NextToken` this service did
/// not issue: one that does not decode, or that decodes to a resume point this
/// service's pagination cannot name.
///
/// Shared by every paginated operation here, so one bad token is refused the same way
/// whichever list produced it. `Reason` is the modelled enum value for a single bad
/// member, and `FieldList` names that member.
pub fn invalid_next_token() -> ValidationException {
    const WHY: &str = "NextToken is not a token this service issued";
    ValidationException {
        message: WHY.to_string(),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: "NextToken".to_string(),
            reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
            message: WHY.to_string(),
        }]),
    }
}

/// [`instance_not_found`] lifted into an operation's error channel, answered with
/// [`RESOURCE_NOT_FOUND_STATUS`].
///
/// `wrap` is the operation's own `ResourceNotFoundException` variant, so every
/// single-instance operation reports a missing instance through this ONE place and
/// none of them can drift to the shape's status or to another token.
pub fn instance_not_found_error<E>(
    id: &str,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(
        wrap(instance_not_found(id)),
        RESOURCE_NOT_FOUND_STATUS,
    )
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

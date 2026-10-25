// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workspacesinstances-eucmifrontendapiservice service.
//!
//! The WorkSpace Instance lifecycle is backed here: every value an operation
//! returns is read out of one of these records, never built from the request.
//!
//! This service is REGIONAL, so every resource here carries the Region it belongs to
//! (the one its create call was signed for, [`request_region`]) and every lookup takes
//! the Region of the request making it: an account's resources in one Region are not
//! reachable, countable or mutable from another, and the Regions the service serves
//! them in are the ones it reports as supporting it ([`SUPPORTED_REGIONS`]).

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    BillingConfiguration, ConflictException, InstanceTypeInfo, ManagedInstanceRequest,
    ResourceNotFoundException, ServiceQuotaExceededException, SupportedInstanceConfiguration, Tag,
    TagSpecification, ValidationException, ValidationExceptionField, BILLING_MODE_VALUES,
    INSTANCE_CONFIGURATION_TENANCY_ENUM_VALUES, PLATFORM_TYPE_ENUM_VALUES,
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

/// The `ResourceType` this service reports for a volume in its resource-scoped
/// errors.
///
/// Like [`WORKSPACE_INSTANCE_RESOURCE_TYPE`], this member is a free `String` the
/// model enumerates no token for, so the token is RECORDED behaviour: the wire corpus
/// answers `EBS_VOLUME` for a missing volume. The token names the AWS resource KIND
/// that was looked up — the Amazon EBS volume a WorkSpace Instance carries, which is
/// what this service's own not-found wording calls it
/// ([`VOLUME_NOT_FOUND_MESSAGE`]) — not the operation's parameter name, and a client
/// switches on it to tell an instance from its volume. (The `ResourceTypeEnum` in
/// `crate::ops::types` is the EC2-style tag-specification enum — the `volume` value a
/// `TagSpecification` carries — and has nothing to do with these error members.)
pub const VOLUME_RESOURCE_TYPE: &str = "EBS_VOLUME";

/// The `ResourceType` token a `TagSpecification` names for a volume: the
/// `ResourceTypeEnum` member (`crate::ops::types::RESOURCE_TYPE_ENUM_VALUES`), which
/// is what `CreateVolume` matches a tag specification against.
pub const VOLUME_TAG_SPECIFICATION_RESOURCE_TYPE: &str = "volume";

/// The `ResourceType` token a `TagSpecification` names for a WorkSpace Instance: the
/// `ResourceTypeEnum` member (`crate::ops::types::RESOURCE_TYPE_ENUM_VALUES`), which
/// is what `CreateWorkspaceInstance` matches a tag specification against.
pub const INSTANCE_TAG_SPECIFICATION_RESOURCE_TYPE: &str = "instance";

/// The `ServiceCode` this service reports in its quota errors: the service's own
/// code, which is also its sigv4 signing name and ARN namespace.
pub const SERVICE_CODE: &str = "workspaces-instances";

/// The `QuotaCode` for the running-instance quota below. AWS publishes no code for
/// it, so this is the emulator's own stable identifier for the quota it enforces.
pub const WORKSPACE_INSTANCE_QUOTA_CODE: &str = "L-WorkSpaceInstancesPerAccount";

/// The `QuotaCode` for the volume quota below. AWS publishes no code for it, so
/// this is the emulator's own stable identifier for the quota it enforces.
pub const VOLUME_QUOTA_CODE: &str = "L-VolumesPerAccount";

/// The number of RUNNING WorkSpace Instances this emulator will hold at once IN ONE
/// REGION.
///
/// A Service Quotas limit applies to one account in one Region, so this is counted per
/// Region ([`ComAmazonawsWorkspacesinstancesEucmifrontendapiserviceState::running_instance_count`]
/// takes the Region the create was signed for): instances in another Region leave this
/// Region's capacity untouched.
///
/// A real account has a Service Quotas limit on running WorkSpace Instances, and
/// `CreateWorkspaceInstance` models `ServiceQuotaExceededException` (HTTP 402) for
/// reaching it. AWS publishes no figure, so the number itself is CHOSEN: its purpose
/// is that the store cannot grow without bound, and that it is reached through the
/// modelled error rather than by exhausting memory.
///
/// WHICH records this counts is CHOSEN too, and AWS documents neither: it counts the
/// records that are not tearing down
/// ([`ComAmazonawsWorkspacesinstancesEucmifrontendapiserviceState::running_instance_count`]),
/// so `DeleteWorkspaceInstance` releases a slot as soon as it is accepted rather than
/// when the teardown it starts finishes. The reasoning is that the quota is on RUNNING
/// instances and an instance in a deallocation state is not one; the alternative
/// (holding the slot until the record stops being reported) would keep a deleted
/// instance's capacity for the teardown span and its retention.
///
/// Because a tearing-down record still OCCUPIES the store while it is reported, a
/// create/delete burst can leave more records held than this; [`MAX_HELD_INSTANCES`]
/// is what keeps that bounded.
pub const MAX_INSTANCES: usize = 500;

/// The number of WorkSpace Instance RECORDS this emulator will hold at once in one
/// Region, counting the ones that are tearing down and so no longer count against
/// [`MAX_INSTANCES`].
///
/// CHOSEN, like [`MAX_INSTANCES`], and for the same purpose: a caller that creates and
/// deletes in a tight loop leaves behind records that are still reported for
/// [`DEALLOCATION_SPAN`] + [`DEALLOCATED_RETENTION`] each, and without a second bound
/// the store could grow without bound even though no instance is running. A create
/// past this bound is refused through the same modelled
/// `ServiceQuotaExceededException`, never by exhausting memory.
pub const MAX_HELD_INSTANCES: usize = 2 * MAX_INSTANCES;

/// The number of volumes this emulator will hold at once in one Region.
///
/// As with [`MAX_INSTANCES`], a real account has a Service Quotas limit on volumes
/// and `CreateVolume` models `ServiceQuotaExceededException` (HTTP 402) for reaching
/// it; AWS publishes no figure, so the number is CHOSEN. Its purpose is that the
/// store cannot grow without bound, and that the bound is reached through the
/// modelled error. Capacity is released by `DeleteVolume`.
pub const MAX_VOLUMES: usize = 500;

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
/// The transient state an instance is in while `DeleteWorkspaceInstance`'s teardown
/// runs. See [`PROVISION_STATE_ALLOCATING`].
pub const PROVISION_STATE_DEALLOCATING: &str = "DEALLOCATING";
/// The terminal state an instance reaches once its teardown has finished, and the
/// last state it is reported in before it stops being reported at all. See
/// [`PROVISION_STATE_ALLOCATING`].
pub const PROVISION_STATE_DEALLOCATED: &str = "DEALLOCATED";

// The two remaining `ProvisionStateEnum` values, `ERROR_ALLOCATING` and
// `ERROR_DEALLOCATING`, name a FAILED allocation or teardown. Nothing in this
// emulator can fail either one — every create launches and every delete tears down —
// so no record is ever put in them: a failure state for work that succeeded would be
// a fabricated status. They become reachable the day this slice models something that
// can fail (a capacity refusal, a stuck volume detach), and the filter in
// `ListWorkspaceInstances` already matches whatever a record holds.

/// How long after `CreateWorkspaceInstance` the `ALLOCATING -> ALLOCATED`
/// transition the call starts becomes due.
///
/// AWS documents no duration for it, so this is a CHOSEN span: it is deliberately
/// non-zero, so a caller that reads the instance back immediately still observes
/// the transient `ALLOCATING` state, and short enough that a test can wait it out.
/// The due instant is stored on the record ([`WorkspaceInstanceRecord::allocation_due`])
/// and materialised lazily by [`WorkspaceInstanceRecord::settle`]; nothing sleeps.
pub const ALLOCATION_SPAN: Duration = Duration::from_millis(500);

/// How long after `DeleteWorkspaceInstance` the `DEALLOCATING -> DEALLOCATED`
/// transition that call starts becomes due.
///
/// Deleting a WorkSpace Instance is asynchronous teardown: the call is accepted and
/// the instance keeps being reported, in a deallocation state, while the teardown
/// runs. AWS documents no duration for it, so this is a CHOSEN span, non-zero for the
/// same reason as [`ALLOCATION_SPAN`]: a caller that reads the instance back straight
/// after the delete still observes the transient `DEALLOCATING`. The due instant is
/// stored on the record ([`WorkspaceInstanceRecord::deallocation_due`]) and
/// materialised lazily by [`WorkspaceInstanceRecord::settle`]; nothing sleeps.
pub const DEALLOCATION_SPAN: Duration = Duration::from_millis(500);

/// How long a torn-down instance stays reported in the terminal `DEALLOCATED` state
/// before the service stops reporting it at all.
///
/// A deleted resource does eventually stop being reported: past this span a `Get`
/// answers `ResourceNotFoundException` and a `List` no longer carries the instance,
/// whichever `ProvisionStates` filter is asked for. AWS documents no retention for a
/// deleted WorkSpace Instance, so the span is CHOSEN; it is non-zero so the terminal
/// `DEALLOCATED` state is observable, and it is materialised lazily like the two
/// transitions above ([`WorkspaceInstanceRecord::is_forgotten`]).
pub const DEALLOCATED_RETENTION: Duration = Duration::from_millis(500);

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

/// The `ValidationExceptionReason` this service reports when the request names no
/// member this service can fault, because it is the CALL that this service cannot
/// serve — a create addressed to a Region the service is not available in. A value of
/// the same modelled enum as [`VALIDATION_REASON_FIELD_VALIDATION_FAILED`].
pub const VALIDATION_REASON_UNSUPPORTED_OPERATION: &str = "UNSUPPORTED_OPERATION";

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
/// specified"), so every row of [`INSTANCE_TYPE_FAMILIES`] supports both values. The
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

/// The `InstanceConfigurationTenancyEnum` value for an instance on hardware its
/// account does not share, the other value of that same modelled enum.
pub const TENANCY_DEDICATED: &str = "DEDICATED";

/// The `TenancyEnum` value a `Placement` carries for shared hardware, and the one EC2
/// places an instance under when the request names no tenancy
/// (`smithy_query(query="shape",shape="TenancyEnum")`).
pub const PLACEMENT_TENANCY_DEFAULT: &str = "default";

/// The `InstanceConfigurationTenancyEnum` value a `Placement$Tenancy` asks for.
///
/// The two modelled enums name one dimension from two sides: a placement tenancy is
/// EC2's (`default` | `dedicated` | `host`), while a catalogued configuration
/// publishes `SHARED` | `DEDICATED`. Shared hardware is `default`; both `dedicated`
/// (hardware dedicated to the account) and `host` (a dedicated host of the account)
/// are hardware the account does not share, so both ask for the dedicated
/// configuration.
pub fn configuration_tenancy(placement_tenancy: &str) -> &'static str {
    if placement_tenancy == PLACEMENT_TENANCY_DEFAULT {
        TENANCY_SHARED
    } else {
        TENANCY_DEDICATED
    }
}

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
///
/// Rows are not written out one per instance type: a family offers the SAME platform
/// images and tenancies at every size it comes in, so the catalog is declared as
/// [`INSTANCE_TYPE_FAMILIES`] (family × sizes) and expanded into these rows once by
/// [`instance_type_catalog`].
#[derive(Debug, Clone)]
pub struct InstanceTypeSpec {
    /// The EC2 instance type name (`<family>.<size>`), inside the modelled
    /// `InstanceType` (`len 1..64`, `^([a-z0-9-]+)\.([a-z0-9]+)$`).
    pub instance_type: String,
    /// The `PlatformTypeEnum` values this instance type carries images for.
    pub platform_types: &'static [&'static str],
    /// The `InstanceConfigurationTenancyEnum` values this instance type can run under.
    pub tenancies: &'static [&'static str],
}

/// One family of the instance-type catalog: an EC2 instance family, the sizes it is
/// offered in, and the configuration dimensions every one of those sizes supports.
///
/// This is how the catalog is DECLARED. An instance type name is
/// `<family>.<size>` (the modelled `InstanceType` pattern
/// `^([a-z0-9-]+)\.([a-z0-9]+)$` says exactly that), and the platform images and
/// tenancies a family offers do not vary with its size, so one row per family with its
/// size ladder states the whole catalog without hiding a sample behind a hand-picked
/// list of names.
#[derive(Debug, Clone, Copy)]
pub struct InstanceFamilySpec {
    /// The family part of the name: `c5`, `m7i-flex`, `g4dn`.
    pub family: &'static str,
    /// The size part of every name in this family: `large`, `2xlarge`, … — the family's
    /// published EC2 size ladder.
    pub sizes: &'static [&'static str],
    /// The `PlatformTypeEnum` values this family carries images for.
    pub platform_types: &'static [&'static str],
    /// The `InstanceConfigurationTenancyEnum` values this family can run under.
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

    /// Whether this instance type publishes any configuration carrying `tenancy` —
    /// the tenancy half of the question a create asks, which names a tenancy (through
    /// `Placement`) and a billing mode but no platform image.
    pub fn supports_tenancy(&self, tenancy: &str) -> bool {
        self.tenancies.contains(&tenancy)
    }

    /// Whether this instance type publishes a configuration carrying BOTH
    /// `billing_mode` and `tenancy`, on any platform image it carries: the whole
    /// question a create asks of the catalog.
    pub fn supports_billing_mode(&self, billing_mode: &str, tenancy: &str) -> bool {
        self.platform_types
            .iter()
            .any(|platform_type| self.supports(billing_mode, platform_type, tenancy))
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
            instance_type: Some(self.instance_type.clone()),
            supported_instance_configurations: Some(self.supported_configurations()),
        }
    }
}

// ---------------------------------------------------------------------------
// The size ladders
//
// An EC2 instance family is offered in a fixed ladder of sizes, and the ladder — not
// this emulator — decides which `<family>.<size>` names exist: `c5` is offered at
// `9xlarge` and `18xlarge` where `m5` is offered at `8xlarge` and `16xlarge`, a
// `-flex` family stops at `16xlarge`, and a burstable family starts at `nano`. SOURCE:
// the published EC2 instance-type names of each family (the same ladders the EC2 API's
// own `InstanceType` enumeration carries). Families offered at identical sizes share a
// ladder, so a family row below names one of these rather than repeating a list.
// ---------------------------------------------------------------------------

/// The burstable (`t`) ladder: `t2`, `t3`, `t3a`.
pub const SIZES_BURSTABLE: &[&str] = &[
    "nano", "micro", "small", "medium", "large", "xlarge", "2xlarge",
];

/// The fifth-generation Intel/AMD ladder: `m5`, `m5a`, `m5ad`, `m5d`, `m5dn`, `m5n`,
/// `c5a`, `c5ad`, `r5`, `r5a`, `r5ad`, `r5b`, `r5d`, `r5dn`, `r5n`.
pub const SIZES_5TH_GENERATION: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
];

/// The `c5` ladder, which steps through `9xlarge`/`18xlarge` rather than
/// `8xlarge`/`16xlarge`: `c5`, `c5d`.
pub const SIZES_C5: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "9xlarge", "12xlarge", "18xlarge", "24xlarge",
];

/// The `c5n` ladder: the network-optimised compute family, which stops at `18xlarge`.
pub const SIZES_C5N: &[&str] = &["large", "xlarge", "2xlarge", "4xlarge", "9xlarge", "18xlarge"];

/// The high-frequency ladder, which steps through `3xlarge`/`6xlarge`: `m5zn`, `z1d`.
pub const SIZES_HIGH_FREQUENCY: &[&str] =
    &["large", "xlarge", "2xlarge", "3xlarge", "6xlarge", "12xlarge"];

/// The sixth-generation Intel ladder: `c6i`, `c6id`, `c6in`, `m6i`, `m6id`, `m6idn`,
/// `m6in`, `r6i`, `r6id`, `r6idn`, `r6in`.
pub const SIZES_6TH_GENERATION_INTEL: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "32xlarge",
];

/// The sixth-generation AMD ladder, which goes one step further than the Intel one:
/// `c6a`, `m6a`, `r6a`.
pub const SIZES_6TH_GENERATION_AMD: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "32xlarge", "48xlarge",
];

/// The seventh-generation Intel ladder: `c7i`, `m7i`, `r7i`, and the `i7i` storage
/// family offered at the same sizes.
pub const SIZES_7TH_GENERATION_INTEL: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "48xlarge",
];

/// The seventh-generation AMD ladder, which starts at `medium`: `c7a`, `m7a`, `r7a`.
pub const SIZES_7TH_GENERATION_AMD: &[&str] = &[
    "medium", "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge",
    "24xlarge", "32xlarge", "48xlarge",
];

/// The eighth-generation Intel ladder, which reaches `96xlarge`: `c8i`, `m8i`, `r8i`.
pub const SIZES_8TH_GENERATION_INTEL: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "32xlarge", "48xlarge", "96xlarge",
];

/// The eighth-generation AMD ladder: `c8a`, `m8a`, `r8a`.
pub const SIZES_8TH_GENERATION_AMD: &[&str] = &[
    "medium", "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge",
    "24xlarge", "48xlarge",
];

/// The `-flex` ladder, which stops at `16xlarge`: `c7i-flex`, `m7i-flex`, `c8i-flex`,
/// `m8i-flex`, `r8i-flex`.
pub const SIZES_FLEX: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge",
];

/// The `r7iz` ladder: the high-frequency memory-optimised family.
pub const SIZES_R7IZ: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "32xlarge",
];

/// The `x1` ladder: the original high-memory family, offered at two sizes only.
pub const SIZES_X1: &[&str] = &["16xlarge", "32xlarge"];

/// The `x1e` ladder.
pub const SIZES_X1E: &[&str] = &["xlarge", "2xlarge", "4xlarge", "8xlarge", "16xlarge", "32xlarge"];

/// The `x2idn` ladder: offered at the three largest sizes only.
pub const SIZES_X2IDN: &[&str] = &["16xlarge", "24xlarge", "32xlarge"];

/// The `x2iedn` ladder.
pub const SIZES_X2IEDN: &[&str] = &[
    "xlarge", "2xlarge", "4xlarge", "8xlarge", "16xlarge", "24xlarge", "32xlarge",
];

/// The `x2iezn` ladder: the high-frequency high-memory family.
pub const SIZES_X2IEZN: &[&str] = &["2xlarge", "4xlarge", "6xlarge", "8xlarge", "12xlarge"];

/// The dense-storage ladder: `d2`, `d3`.
pub const SIZES_DENSE_STORAGE: &[&str] = &["xlarge", "2xlarge", "4xlarge", "8xlarge"];

/// The `d3en` ladder, which steps through `6xlarge`.
pub const SIZES_D3EN: &[&str] = &["xlarge", "2xlarge", "4xlarge", "6xlarge", "8xlarge", "12xlarge"];

/// The `h1` ladder: the HDD-backed storage family, which starts at `2xlarge`.
pub const SIZES_H1: &[&str] = &["2xlarge", "4xlarge", "8xlarge", "16xlarge"];

/// The `i3` ladder.
pub const SIZES_I3: &[&str] = &["large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "16xlarge"];

/// The `i3en` ladder, which steps through `3xlarge`/`6xlarge`.
pub const SIZES_I3EN: &[&str] = &[
    "large", "xlarge", "2xlarge", "3xlarge", "6xlarge", "12xlarge", "24xlarge",
];

/// The `i4i` ladder.
pub const SIZES_I4I: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "32xlarge",
];

/// The `i7ie` ladder, which steps through `3xlarge`/`6xlarge`/`18xlarge`.
pub const SIZES_I7IE: &[&str] = &[
    "large", "xlarge", "2xlarge", "3xlarge", "6xlarge", "12xlarge", "18xlarge", "24xlarge",
    "48xlarge",
];

/// The ladder the current NVIDIA GPU families are offered at: `g5`, `g6`, `g6e`.
pub const SIZES_GPU: &[&str] = &[
    "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge", "48xlarge",
];

/// The `g4dn` ladder.
pub const SIZES_G4DN: &[&str] = &["xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge"];

/// The `g4ad` ladder: the AMD GPU family.
pub const SIZES_G4AD: &[&str] = &["xlarge", "2xlarge", "4xlarge", "8xlarge", "16xlarge"];

/// The `gr6` ladder: the memory-heavy variant of `g6`, offered at two sizes.
pub const SIZES_GR6: &[&str] = &["4xlarge", "8xlarge"];

/// The instance families WorkSpaces Instances supports, each with the sizes it is
/// offered in and the configuration dimensions those sizes support.
///
/// MEMBERSHIP is service data, not a modelled list: the model constrains only the
/// SHAPE of a name (`InstanceType`: `len 1..64`, `^([a-z0-9-]+)\.([a-z0-9]+)$`) and
/// enumerates no instance type, so what belongs here is what AWS publishes as
/// supported for this service. SOURCE: the instance families AWS documents for
/// WorkSpaces Instances — the x86-64 (Intel and AMD) general-purpose, burstable,
/// compute-, memory- and storage-optimised families from the fifth generation on,
/// plus the graphics (`g`) families — crossed with each family's published EC2 size
/// ladder above. What is deliberately OUTSIDE it: previous-generation families
/// (`c1`, `c3`, `c4`, `m1`…`m4`, `r3`, `r4`, `t1`, `i2`, `d1`), AArch64/Graviton
/// families (no WorkSpaces x86 platform image runs on them), bare-metal sizes (a
/// WorkSpace Instance is a managed EC2 instance, not a dedicated host), Mac, HPC and
/// ML-accelerator families (`hpc*`, `p*`, `trn*`, `inf*`, `f*`, `vt*`, `dl*`), and
/// every non-commercial partition's exclusives.
///
/// The expansion is reported whole on one page by default: the catalog is smaller than
/// the modelled maximum page of `ListInstanceTypesMaxResults` (`range 1..600`), which
/// is the ceiling the service's own catalog fits in.
pub const INSTANCE_TYPE_FAMILIES: &[InstanceFamilySpec] = &[
    // Burstable general purpose. EC2 offers the `t` families in shared tenancy only,
    // so they are the catalog's shared-tenancy rows.
    InstanceFamilySpec {
        family: "t2",
        sizes: SIZES_BURSTABLE,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_TENANCY_ONLY,
    },
    InstanceFamilySpec {
        family: "t3",
        sizes: SIZES_BURSTABLE,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_TENANCY_ONLY,
    },
    InstanceFamilySpec {
        family: "t3a",
        sizes: SIZES_BURSTABLE,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_TENANCY_ONLY,
    },
    // General purpose.
    InstanceFamilySpec {
        family: "m5",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m5a",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m5ad",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m5d",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m5dn",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m5n",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m5zn",
        sizes: SIZES_HIGH_FREQUENCY,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m6a",
        sizes: SIZES_6TH_GENERATION_AMD,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m6i",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m6id",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m6idn",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m6in",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m7a",
        sizes: SIZES_7TH_GENERATION_AMD,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m7i",
        sizes: SIZES_7TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m7i-flex",
        sizes: SIZES_FLEX,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m8a",
        sizes: SIZES_8TH_GENERATION_AMD,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m8i",
        sizes: SIZES_8TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "m8i-flex",
        sizes: SIZES_FLEX,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    // Compute optimised.
    InstanceFamilySpec {
        family: "c5",
        sizes: SIZES_C5,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c5a",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c5ad",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c5d",
        sizes: SIZES_C5,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c5n",
        sizes: SIZES_C5N,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c6a",
        sizes: SIZES_6TH_GENERATION_AMD,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c6i",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c6id",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c6in",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c7a",
        sizes: SIZES_7TH_GENERATION_AMD,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c7i",
        sizes: SIZES_7TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c7i-flex",
        sizes: SIZES_FLEX,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c8a",
        sizes: SIZES_8TH_GENERATION_AMD,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c8i",
        sizes: SIZES_8TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "c8i-flex",
        sizes: SIZES_FLEX,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    // Memory optimised.
    InstanceFamilySpec {
        family: "r5",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r5a",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r5ad",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r5b",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r5d",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r5dn",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r5n",
        sizes: SIZES_5TH_GENERATION,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r6a",
        sizes: SIZES_6TH_GENERATION_AMD,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r6i",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r6id",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r6idn",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r6in",
        sizes: SIZES_6TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r7a",
        sizes: SIZES_7TH_GENERATION_AMD,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r7i",
        sizes: SIZES_7TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r7iz",
        sizes: SIZES_R7IZ,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r8a",
        sizes: SIZES_8TH_GENERATION_AMD,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r8i",
        sizes: SIZES_8TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "r8i-flex",
        sizes: SIZES_FLEX,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "z1d",
        sizes: SIZES_HIGH_FREQUENCY,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    // High memory.
    InstanceFamilySpec {
        family: "x1",
        sizes: SIZES_X1,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "x1e",
        sizes: SIZES_X1E,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "x2idn",
        sizes: SIZES_X2IDN,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "x2iedn",
        sizes: SIZES_X2IEDN,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "x2iezn",
        sizes: SIZES_X2IEZN,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    // Storage optimised.
    InstanceFamilySpec {
        family: "d2",
        sizes: SIZES_DENSE_STORAGE,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "d3",
        sizes: SIZES_DENSE_STORAGE,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "d3en",
        sizes: SIZES_D3EN,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "h1",
        sizes: SIZES_H1,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "i3",
        sizes: SIZES_I3,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "i3en",
        sizes: SIZES_I3EN,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "i4i",
        sizes: SIZES_I4I,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "i7i",
        sizes: SIZES_7TH_GENERATION_INTEL,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "i7ie",
        sizes: SIZES_I7IE,
        platform_types: ALL_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    // Graphics. These carry the GPU platform images only — see
    // [`GPU_PLATFORM_TYPES`].
    InstanceFamilySpec {
        family: "g4ad",
        sizes: SIZES_G4AD,
        platform_types: GPU_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "g4dn",
        sizes: SIZES_G4DN,
        platform_types: GPU_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "g5",
        sizes: SIZES_GPU,
        platform_types: GPU_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "g6",
        sizes: SIZES_GPU,
        platform_types: GPU_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "g6e",
        sizes: SIZES_GPU,
        platform_types: GPU_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
    InstanceFamilySpec {
        family: "gr6",
        sizes: SIZES_GR6,
        platform_types: GPU_PLATFORM_TYPES,
        tenancies: SHARED_AND_DEDICATED_TENANCY,
    },
];

/// The instance types WorkSpaces Instances supports, ordered by instance type name.
///
/// [`INSTANCE_TYPE_FAMILIES`] expanded: one row per `<family>.<size>`, sorted by name
/// and holding each name once. The ORDER is the name order, which is what makes
/// `NextToken` resume at a well-defined place, and it is fixed, so a page boundary is
/// reproducible. Expanded ONCE on first use and shared from then on: the catalog is
/// fixed service data, so two calls — and two emulators built from this source — report
/// the same rows in the same order, and reading it changes nothing.
pub fn instance_type_catalog() -> &'static [InstanceTypeSpec] {
    static CATALOG: OnceLock<Vec<InstanceTypeSpec>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut rows: Vec<InstanceTypeSpec> = INSTANCE_TYPE_FAMILIES
            .iter()
            .flat_map(|family| {
                family.sizes.iter().map(move |size| InstanceTypeSpec {
                    instance_type: format!("{}.{}", family.family, size),
                    platform_types: family.platform_types,
                    tenancies: family.tenancies,
                })
            })
            .collect();
        rows.sort_by(|left, right| left.instance_type.cmp(&right.instance_type));
        rows.dedup_by(|left, right| left.instance_type == right.instance_type);
        rows
    })
}

/// The position of `instance_type` in [`instance_type_catalog`], if the catalog holds
/// it. This is how a `ListInstanceTypes` `NextToken` resolves to a resume point, so a
/// token naming no catalogued instance type is refused rather than guessed at. The
/// catalog is sorted by name, so the lookup is a binary search over it.
pub fn instance_type_index(instance_type: &str) -> Option<usize> {
    instance_type_catalog()
        .binary_search_by(|spec| spec.instance_type.as_str().cmp(instance_type))
        .ok()
}

/// The catalog row for `instance_type`, if [`instance_type_catalog`] holds it.
///
/// The ONE place a request's instance type is resolved against the catalog
/// `ListInstanceTypes` publishes, so an operation that has to decide whether the
/// service can launch a type reads the same rows that call reports.
pub fn instance_type_spec(instance_type: &str) -> Option<&'static InstanceTypeSpec> {
    instance_type_index(instance_type).map(|at| &instance_type_catalog()[at])
}

/// The AWS Regions `ListRegions` reports as supporting WorkSpaces Instances, in
/// ascending `RegionName` order.
///
/// MEMBERSHIP is service data, not a modelled list: `Region` carries a free
/// `RegionName` string (`^[-0-9a-z]{1,31}$`) and the service's endpoint rule set is
/// partition-templated (`smithy_query(query="shape",shape="Region")`), so there is no
/// modelled list to project and what belongs here is the service's own availability
/// set. SOURCE: AWS's published Region availability for WorkSpaces Instances in the
/// `aws` (commercial) partition — the Region table behind "Amazon WorkSpaces Instances
/// endpoints and quotas", confirmed against the wire recordings of this operation,
/// which report every commercial Region below — among them the US and European ones
/// (`us-east-2`, `eu-west-3`, `eu-south-2`) and the Asia-Pacific and Middle East ones
/// (`ap-east-1`, `ap-southeast-5`, `me-central-1`) a narrower list would hide. A
/// caller uses this set to decide where it may deploy, so a Region the service
/// supports being absent tells it the service is unavailable there.
///
/// Regions in the GovCloud and China partitions are deliberately absent: this
/// emulator serves the commercial partition, which is the partition its endpoints and
/// its signing scope name.
///
/// Kept SORTED and free of duplicates: `ListRegions` reports it in this order, whole,
/// so the order a caller observes is this one.
pub const SUPPORTED_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-east-1",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-5",
    "ca-central-1",
    "eu-central-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
    "me-central-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-west-2",
];

/// The Region a request belongs to when it carries no SigV4 credential scope to read
/// one from: the Region every emulator defaults to (`::engine::emulator_arn`, whose
/// doc gives `us-east-1` as that default).
pub const DEFAULT_REGION: &str = "us-east-1";

/// Whether `candidate` is shaped like an AWS Region name: `Region$RegionName`'s
/// `^[-0-9a-z]{1,31}$` (`smithy_query(query="shape",shape="Region")`).
///
/// The model constrains the Region names this service REPORTS; this is the same test
/// applied to a Region name the service reads out of a request's credential scope,
/// which no boundary validator sees.
pub fn is_modelled_region_name(candidate: &str) -> bool {
    (1..=31).contains(&candidate.len())
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The Region this request was signed for, from its SigV4 credential scope;
/// [`DEFAULT_REGION`] when it carries none.
///
/// A regional resource belongs to the Region its create call was signed for, so this
/// is where the Region comes from — never from a request member. The scope is
/// caller-controlled text that is rendered into Availability Zone names, so a value
/// that is not shaped like a Region name at all ([`is_modelled_region_name`]) names no
/// Region and the default is used instead.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_modelled_region_name(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether this service is available in Region `region`: whether the Region is one of
/// the [`SUPPORTED_REGIONS`] `ListRegions` reports.
///
/// A caller reads that set to decide where it may deploy, so the Regions the service
/// serves resources in are exactly the Regions it reports as supporting it — a Region
/// outside the set (another partition, or a commercial Region the service has not
/// launched in) has no endpoint of this service and so holds none of its resources.
pub fn is_supported_region(region: &str) -> bool {
    SUPPORTED_REGIONS.contains(&region)
}

/// The modelled `ValidationException` (HTTP 400) for a create addressed to a Region
/// this service is not available in ([`is_supported_region`]).
///
/// `Reason` is [`VALIDATION_REASON_UNSUPPORTED_OPERATION`] and `FieldList` is absent:
/// no member of the request is at fault — the Region travels in the credential scope —
/// so there is no member to name, and the wording states the rule without echoing what
/// the caller sent.
pub fn region_not_supported() -> ValidationException {
    ValidationException {
        message: "this service is not available in the Region the request was signed for"
            .to_string(),
        reason: VALIDATION_REASON_UNSUPPORTED_OPERATION.to_string(),
        field_list: None,
    }
}

/// The zone letters each Region this service serves publishes Availability Zones for,
/// in ascending order, one row per Region of [`SUPPORTED_REGIONS`].
///
/// A zone's name is its Region's name followed by one of that Region's letters
/// (`us-east-1a`). WHICH letters a Region publishes is PER REGION data: the count
/// differs from Region to Region and the letters are not a run starting at `a` — the
/// oldest US Region publishes six zones, two of the four-zone Regions publish `a`
/// through `d`, Tokyo publishes `a`, `c` and `d` with no `b`, and Canada Central
/// publishes `a`, `b` and `d`. A caller names the zone its subnet sits in, so a zone
/// a Region really has must be accepted and a letter that Region does not publish must
/// not be, whichever side of the third letter it falls on.
///
/// SOURCE: AWS's published Availability Zones per Region ("Regions and Zones" in the
/// AWS documentation, the same table the Lightsail service's own Region catalog in
/// this emulator records its `availability_zone_letters` from). Every Region whose
/// published zones are the three letters `a`, `b`, `c` carries that string here rather
/// than defaulting to it, so the catalog states what each Region has instead of
/// assuming a shape.
///
/// Kept SORTED by Region name and free of duplicates, like [`SUPPORTED_REGIONS`], and
/// covering exactly that set: a Region this service does not serve has no endpoint
/// here and so no zones here either.
pub const REGION_AVAILABILITY_ZONE_LETTERS: &[(&str, &str)] = &[
    ("af-south-1", "abc"),
    ("ap-east-1", "abc"),
    ("ap-northeast-1", "acd"),
    ("ap-northeast-2", "abcd"),
    ("ap-south-1", "abc"),
    ("ap-southeast-1", "abc"),
    ("ap-southeast-2", "abc"),
    ("ap-southeast-5", "abc"),
    ("ca-central-1", "abd"),
    ("eu-central-1", "abc"),
    ("eu-south-2", "abc"),
    ("eu-west-1", "abc"),
    ("eu-west-2", "abc"),
    ("eu-west-3", "abc"),
    ("il-central-1", "abc"),
    ("me-central-1", "abc"),
    ("sa-east-1", "abc"),
    ("us-east-1", "abcdef"),
    ("us-east-2", "abc"),
    ("us-west-2", "abcd"),
];

/// The zone letters Region `region` publishes, from
/// [`REGION_AVAILABILITY_ZONE_LETTERS`]; the empty string for a Region this service
/// does not serve, which publishes no zones here at all.
pub fn availability_zone_letters(region: &str) -> &'static str {
    REGION_AVAILABILITY_ZONE_LETTERS
        .iter()
        .find(|(name, _)| *name == region)
        .map_or("", |(_, letters)| *letters)
}

/// The Availability Zone names Region `region` has: `<region><letter>`, one per letter
/// [`availability_zone_letters`] gives for that Region.
///
/// Derived from the catalog rather than stored: no operation of this service creates,
/// changes or deletes an Availability Zone, so two emulators built from this source
/// place a volume in the same zones and reading the set changes nothing.
pub fn availability_zones_in_region(region: &str) -> Vec<String> {
    availability_zone_letters(region)
        .chars()
        .map(|letter| format!("{region}{letter}"))
        .collect()
}

/// The volume type a `CreateVolume` that names none creates.
///
/// "Default: gp2" (Amazon EBS `CreateVolume$VolumeType`). This service's own model
/// documents no default for its `CreateVolumeRequest$VolumeType`, and what the call
/// creates IS an Amazon EBS volume, so the backing service's default is the one that
/// applies — and the type the per-type rules below are then read for.
pub const DEFAULT_VOLUME_TYPE: &str = "gp2";

/// The `SizeInGB` range this service's model admits at all: `VolumeSizeInGB`'s
/// `range 1..16384` (`smithy_query(query="shape",shape="VolumeSizeInGB")`).
///
/// The boundary validator refuses anything outside it before a handler runs, so a
/// per-type range is only ever applied INSIDE this one — see
/// [`VolumeTypeSpec::size_range`].
pub const MODELLED_VOLUME_SIZE_RANGE: (i32, i32) = (1, 16_384);

/// What ONE Amazon EBS volume type accepts: the size range AWS publishes for it, the
/// provisioned-IOPS range (and whether IOPS are required), and the throughput range.
///
/// `None` means the type takes that member at all: a volume type that provisions no
/// IOPS is one `Iops` must be omitted for, not one with a wide range.
#[derive(Debug, Clone, Copy)]
pub struct VolumeTypeSpec {
    /// A `VolumeTypeEnum` value (`crate::ops::types::VOLUME_TYPE_ENUM_VALUES`).
    pub volume_type: &'static str,
    /// The size range AWS publishes for the type, in GiB, as `(min, max)`.
    pub published_size_gib: (i32, i32),
    /// The provisioned-IOPS range the type accepts, or `None` for a type that
    /// provisions none.
    pub iops: Option<(i32, i32)>,
    /// Whether a create of this type must name `Iops`.
    pub iops_required: bool,
    /// The throughput range the type accepts, in MiB/s, or `None` for a type whose
    /// throughput is not configurable.
    pub throughput_mib: Option<(i32, i32)>,
}

impl VolumeTypeSpec {
    /// The size range a request may name for this type HERE: the published range
    /// narrowed to what the model can carry ([`MODELLED_VOLUME_SIZE_RANGE`]).
    ///
    /// `gp3` and `io2` are published up to 65,536 GiB, which `SizeInGB` (`range
    /// 1..16384`) cannot express, so the model's ceiling is the effective one for
    /// them; the per-type MINIMUM is above the model's floor for `io1`, `io2`, `st1`
    /// and `sc1`, so it is the effective one there.
    pub fn size_range(&self) -> (i32, i32) {
        let (model_min, model_max) = MODELLED_VOLUME_SIZE_RANGE;
        (
            self.published_size_gib.0.max(model_min),
            self.published_size_gib.1.min(model_max),
        )
    }
}

/// Every volume type this service provisions, and what each one accepts.
///
/// These are the configuration rules of the BACKING Amazon EBS volume service, which
/// this service's model cannot express (`Iops` and `Throughput` are plain
/// `NonNegativeInteger`s and `SizeInGB` one range for every type), so they are the
/// handler's to apply. SOURCE: the Amazon EBS `CreateVolume` reference —
///
/// * `Size`: "Valid sizes: gp2: 1 - 16,384 GiB; gp3: 1 - 65,536 GiB; io1: 4 - 16,384
///   GiB; io2: 4 - 65,536 GiB; st1 and sc1: 125 - 16,384 GiB; standard: 1 - 1024 GiB";
/// * `Iops`: "Required for io1 and io2 volumes. Optional for gp3 volumes. Omit for all
///   other volume types. Valid ranges: gp3: 3,000 - 80,000 IOPS; io1: 100 - 64,000
///   IOPS; io2: 100 - 256,000 IOPS";
/// * `Throughput`: "Supported for gp3 volumes only. Omit for all other volume types.
///   Valid Range: 125 - 2000 MiB/s".
///
/// The published table is the same one the EC2 service crate models for its own
/// `CreateVolume` (`VOLUME_SIZE_RANGES` / `VOLUME_IOPS_RANGES` there): it is a fact of
/// Amazon EBS, so the two must stay the values above rather than diverge.
///
/// Every value of the modelled `VolumeTypeEnum`
/// (`crate::ops::types::VOLUME_TYPE_ENUM_VALUES`) appears exactly once, so no admitted
/// type is left without rules.
pub const VOLUME_TYPE_SPECS: [VolumeTypeSpec; 7] = [
    VolumeTypeSpec {
        volume_type: "gp2",
        published_size_gib: (1, 16_384),
        iops: None,
        iops_required: false,
        throughput_mib: None,
    },
    VolumeTypeSpec {
        volume_type: "gp3",
        published_size_gib: (1, 65_536),
        iops: Some((3_000, 80_000)),
        iops_required: false,
        throughput_mib: Some((125, 2_000)),
    },
    VolumeTypeSpec {
        volume_type: "io1",
        published_size_gib: (4, 16_384),
        iops: Some((100, 64_000)),
        iops_required: true,
        throughput_mib: None,
    },
    VolumeTypeSpec {
        volume_type: "io2",
        published_size_gib: (4, 65_536),
        iops: Some((100, 256_000)),
        iops_required: true,
        throughput_mib: None,
    },
    VolumeTypeSpec {
        volume_type: "sc1",
        published_size_gib: (125, 16_384),
        iops: None,
        iops_required: false,
        throughput_mib: None,
    },
    VolumeTypeSpec {
        volume_type: "st1",
        published_size_gib: (125, 16_384),
        iops: None,
        iops_required: false,
        throughput_mib: None,
    },
    VolumeTypeSpec {
        volume_type: "standard",
        published_size_gib: (1, 1_024),
        iops: None,
        iops_required: false,
        throughput_mib: None,
    },
];

/// The rules [`VOLUME_TYPE_SPECS`] carries for `volume_type`, or `None` for a type
/// this service has no rules for — which the modelled enum cannot admit, so a `None`
/// here means the model and the table have drifted.
pub fn volume_type_spec(volume_type: &str) -> Option<&'static VolumeTypeSpec> {
    VOLUME_TYPE_SPECS
        .iter()
        .find(|spec| spec.volume_type == volume_type)
}

/// One WorkSpace Instance, as created by `CreateWorkspaceInstance`.
#[derive(Debug, Clone)]
pub struct WorkspaceInstanceRecord {
    /// The minted `wsinst-…` id, matching `^wsinst-[0-9a-zA-Z]{8,63}$`.
    pub id: String,
    /// The Region this instance lives in: the one the create call that made it was
    /// signed for ([`request_region`]).
    ///
    /// This service is REGIONAL, so the record belongs to that Region alone and is
    /// listed, read, tagged and deleted only through it; a call signed for any other
    /// Region sees an account that does not hold this instance.
    pub region: String,
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
    /// The instant the `DEALLOCATING -> DEALLOCATED` transition falls due, set by
    /// the `DeleteWorkspaceInstance` that started the teardown and `None` on a record
    /// no delete has been accepted for. Monotonic, like [`Self::allocation_due`], and
    /// also the point the retention of [`DEALLOCATED_RETENTION`] is measured from.
    pub deallocation_due: Option<Instant>,
}

impl WorkspaceInstanceRecord {
    /// Materialise the transition the clock has already made due.
    ///
    /// Called from reads as well as writes: the move to `ALLOCATED`, and the move from
    /// `DEALLOCATING` to the terminal `DEALLOCATED`, were each owed at their stored
    /// due instant whether or not anybody called, so applying them on read reports
    /// the state the service would already be in, and never changes what a later
    /// caller sees beyond that.
    pub fn settle(&mut self, now: Instant) {
        if self.provision_state == PROVISION_STATE_ALLOCATING && now >= self.allocation_due {
            self.provision_state = PROVISION_STATE_ALLOCATED.to_string();
        }
        if self.provision_state == PROVISION_STATE_DEALLOCATING
            && self.deallocation_due.is_some_and(|due| now >= due)
        {
            self.provision_state = PROVISION_STATE_DEALLOCATED.to_string();
        }
    }

    /// Start the asynchronous teardown `DeleteWorkspaceInstance` performs: the
    /// instance leaves whatever state it was in for the transient `DEALLOCATING`, and
    /// the move on to the terminal `DEALLOCATED` falls due one [`DEALLOCATION_SPAN`]
    /// from `now`.
    ///
    /// A record already tearing down is left exactly as it is — same state, same
    /// deadline: a second delete asks for an end state the instance is already heading
    /// to, so it must neither push the teardown further out nor move a record that has
    /// reached `DEALLOCATED` back to `DEALLOCATING`.
    pub fn begin_deallocation(&mut self, now: Instant) {
        if self.is_tearing_down() {
            return;
        }
        self.deallocation_due = Some(now + DEALLOCATION_SPAN);
        self.provision_state = PROVISION_STATE_DEALLOCATING.to_string();
    }

    /// The Availability Zone this instance was placed in, when its create call named
    /// one — `ManagedInstance.Placement.AvailabilityZone`.
    ///
    /// `None` where the create call named no zone (no `Placement` at all, the member
    /// left out, or an empty string, which names no zone): the placement was left to
    /// the service, and this emulator reports no zone for such an instance, so nothing
    /// may be decided from a zone it does not know.
    pub fn placement_availability_zone(&self) -> Option<&str> {
        self.managed_instance
            .placement
            .as_ref()
            .and_then(|placement| placement.availability_zone.as_deref())
            .filter(|zone| !zone.is_empty())
    }

    /// Whether a delete has been accepted for this instance, so it is tearing down or
    /// torn down and no longer one of the account's running instances.
    pub fn is_tearing_down(&self) -> bool {
        self.deallocation_due.is_some()
    }

    /// Whether the clock has already taken this torn-down record past
    /// [`DEALLOCATED_RETENTION`], so the service no longer reports it at all.
    ///
    /// Measured from the teardown deadline, so the record is reported in
    /// `DEALLOCATING` for one [`DEALLOCATION_SPAN`] and then in `DEALLOCATED` for one
    /// [`DEALLOCATED_RETENTION`] before it goes.
    pub fn is_forgotten(&self, now: Instant) -> bool {
        self.deallocation_due
            .is_some_and(|due| now >= due + DEALLOCATED_RETENTION)
    }
}

/// Where ONE volume is attached: the WorkSpace Instance it is attached to, and the
/// device path it is attached at.
///
/// A volume holds at most one of these ([`VolumeRecord::attachment`]), which is what
/// makes an attachment the relation `AssociateVolume` creates and `DisassociateVolume`
/// removes: the backing Amazon EBS volume service attaches a volume to one instance at
/// a time (its `AttachVolume` answers `VolumeInUse` for a volume that already carries
/// an attachment), and one device path on an instance carries one volume.
///
/// The attachment is held on the VOLUME rather than in a map of its own so the two can
/// never disagree: deleting the volume record takes its attachment with it, and no
/// second copy of the relation exists for an operation to read instead.
#[derive(Debug, Clone)]
pub struct VolumeAttachment {
    /// The `wsinst-…` id of the WorkSpace Instance the volume is attached to.
    pub workspace_instance_id: String,
    /// The device path the volume is attached at, as the attaching call named it,
    /// inside the modelled `DeviceName` (`len 0..32`).
    pub device: String,
}

/// One volume, as created by `CreateVolume`.
///
/// The configuration is held exactly as the creating call supplied it — nothing is
/// defaulted, because this service models no operation that reports a volume's
/// configuration back, and `ModifyVolume` reports the TARGET of a modification. A
/// member the caller left out stays `None` so a later reader can tell "unset" from
/// "set to the service default".
#[derive(Debug, Clone)]
pub struct VolumeRecord {
    /// The minted `vol-…` id, matching `^vol-[0-9a-zA-Z]{1,63}$`.
    pub id: String,
    /// The Region this volume lives in: the one the create call that made it was
    /// signed for ([`request_region`]), which is also the Region
    /// [`Self::availability_zone`] names a zone of. Like a WorkSpace Instance, the
    /// volume is reachable only through that Region.
    pub region: String,
    /// The `AvailabilityZone` the volume was created in (a required member).
    pub availability_zone: String,
    /// `SizeInGB`, inside the modelled `range 1..16384` when present.
    pub size_in_gb: Option<i32>,
    /// A `VolumeTypeEnum` value (`crate::ops::types::VOLUME_TYPE_ENUM_VALUES`).
    pub volume_type: Option<String>,
    /// `Iops`, as supplied.
    pub iops: Option<i32>,
    /// `Throughput`, as supplied.
    pub throughput: Option<i32>,
    /// `Encrypted`, as supplied.
    pub encrypted: Option<bool>,
    /// `KmsKeyId`, as supplied. Sensitive: never logged.
    pub kms_key_id: Option<String>,
    /// The `snap-…` id the volume was restored from, if any.
    pub snapshot_id: Option<String>,
    /// Tags from the creating call's `TagSpecifications`, keyed by `Tag.Key`.
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` the creating call carried, if any.
    pub client_token: Option<String>,
    /// When the record was created.
    pub created_at: SystemTime,
    /// Where this volume is attached, or `None` for a volume no `AssociateVolume` has
    /// attached (or one a `DisassociateVolume` has detached again).
    pub attachment: Option<VolumeAttachment>,
}

/// What one spent `@idempotencyToken` created, and WHICH request spent it.
///
/// A create's `ClientToken` identifies one request, not just one caller: a later
/// call carrying the same token is a retry of THAT request, so the claim keeps a
/// [`request_fingerprint`] of the parameters the first call carried beside the id it
/// minted. A retry whose parameters fingerprint alike replays `resource_id`; one
/// whose parameters differ is a different request under a spent token, which the
/// create operations refuse with [`idempotent_parameter_mismatch`].
#[derive(Debug, Clone)]
pub struct IdempotencyClaim {
    /// The id the first call minted: the only id this token can ever answer with.
    pub resource_id: String,
    /// [`request_fingerprint`] of the parameters that first call carried.
    pub fingerprint: String,
}

/// What a create call must do about the `ClientToken` it carries — the answer
/// [`resolve_client_token`] gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenOutcome {
    /// No claim holds this token (or the call carries none): create the resource.
    Unclaimed,
    /// A retry of the same request: replay this id and create nothing.
    Replay(String),
    /// The token is already spent on DIFFERENT parameters: refuse the call, naming
    /// the resource the token did create.
    Mismatch(String),
}

/// Resolve a create's `ClientToken` against the claims this service already holds for
/// the Region the create was signed for.
///
/// The ONE place the idempotency decision is made, so every `@idempotent` create of
/// this service tells a retry from a reuse the same way: a token is matched on the
/// Region, the token AND the fingerprint of the request's parameters, never on the
/// token alone. The Region is part of the key because what the token created is
/// regional — the same token at another Region's endpoint asks that Region for a
/// resource it does not hold, so it creates one there rather than replaying this one.
pub fn resolve_client_token(
    claims: &BTreeMap<(String, String), IdempotencyClaim>,
    region: &str,
    token: Option<&str>,
    fingerprint: &str,
) -> TokenOutcome {
    let Some(claim) = token.and_then(|token| claims.get(&(region.to_string(), token.to_string())))
    else {
        return TokenOutcome::Unclaimed;
    };
    if claim.fingerprint == fingerprint {
        TokenOutcome::Replay(claim.resource_id.clone())
    } else {
        TokenOutcome::Mismatch(claim.resource_id.clone())
    }
}

/// Reduce a create request's parameters to ONE comparable value, so a reused
/// `ClientToken` can be checked against the request it was first spent on.
///
/// `parameters` is the request shape itself with its `ClientToken` cleared — the
/// token is the KEY a claim is stored under, not one of the parameters being
/// compared. The rendering is the shape's DERIVED `Debug`, which prints every member
/// the model declares, in declaration order, with each value in Rust literal form
/// (strings quoted and escaped, so no member value can imitate the start of the next
/// member): two renderings are equal exactly when every member is equal, and a member
/// a later model revision adds joins the comparison without this code changing. The
/// shapes this is called with must therefore keep their DERIVED `Debug`: a
/// hand-written one that hid or folded a member would silently weaken the comparison.
///
/// The rendering is DIGESTED rather than stored: a claim then costs the same however
/// large the request was, and the sensitive members a request can carry
/// (`ClientToken`, `KmsKeyId`) are not held a second time in the clear. The digest is
/// only ever compared with another digest this same code produced, and is never
/// returned to a caller or logged.
pub fn request_fingerprint(parameters: &impl Debug) -> String {
    let mut digest = Sha256::new();
    digest.update(format!("{parameters:?}").as_bytes());
    hex::encode(digest.finalize())
}

/// The `Message` this service states for a `ClientToken` spent on one request and
/// then reused for a different one.
///
/// A FIXED sentence, like [`INSTANCE_NOT_FOUND_MESSAGE`] and for the same reason: it
/// states what the conflict IS and leaves the identity of the conflicting resource to
/// the modelled `ResourceId` member, so the prose does not vary per request and echoes
/// nothing the caller sent (the token is `@sensitive`).
pub const IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str =
    "ClientToken was already used to create a resource with different parameters";

/// The modelled `ConflictException` (HTTP 409) for a create whose `ClientToken` is
/// already spent on different parameters.
///
/// The ONE place this shape is built, so every `@idempotent` create of this service
/// refuses a reused token identically: `ResourceId` names the resource the token DID
/// create — the only resource that token can ever answer with — and `ResourceType`
/// that resource's kind ([`WORKSPACE_INSTANCE_RESOURCE_TYPE`] /
/// [`VOLUME_RESOURCE_TYPE`]). Every member the model marks `@required` is filled.
pub fn idempotent_parameter_mismatch(resource_id: &str, resource_type: &str) -> ConflictException {
    ConflictException {
        message: IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE.to_string(),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// Per-service state for com-amazonaws-workspacesinstances-eucmifrontendapiservice. A handler reaches it through [`service_state`].
///
/// `instances` is a `BTreeMap` rather than a hash map so iteration order is the
/// id order, which is what makes `ListWorkspaceInstances`' `NextToken` resume at a
/// well-defined place: ids are minted from `id_counter` zero-padded, so id order
/// is creation order.
///
/// Every resource here carries the Region it belongs to, and every lookup below takes
/// the Region of the request making it: this service is regional, so one store holds
/// each Region's resources side by side without any of them being reachable from
/// another Region.
#[derive(Debug, Default)]
pub struct ComAmazonawsWorkspacesinstancesEucmifrontendapiserviceState {
    /// Every live WorkSpace Instance, keyed by its `wsinst-…` id. The id is minted
    /// from the one account-wide [`Self::id_counter`], so it is unique across Regions
    /// and the Region a record belongs to is the one on the record itself.
    pub instances: BTreeMap<String, WorkspaceInstanceRecord>,
    /// (Region, `ClientToken`) -> the [`IdempotencyClaim`] `CreateWorkspaceInstance`
    /// spent that token on in that Region: the instance id to replay, and the
    /// fingerprint of the request that minted it, so a token reused for different
    /// parameters is refused rather than replayed.
    ///
    /// Keyed by Region as well as token because the resource a token creates is
    /// regional: the same token presented to another Region's endpoint is a create
    /// for a Region that holds nothing yet, not a retry of this one.
    pub client_tokens: BTreeMap<(String, String), IdempotencyClaim>,
    /// Every live volume, keyed by its `vol-…` id, each carrying its own Region.
    pub volumes: BTreeMap<String, VolumeRecord>,
    /// (Region, `ClientToken`) -> the [`IdempotencyClaim`] `CreateVolume` spent it on.
    /// A separate namespace from [`Self::client_tokens`]: a token is scoped to the
    /// create call that carried it, so a volume token can never replay an instance
    /// id (the two ids are not even the same shape).
    pub volume_client_tokens: BTreeMap<(String, String), IdempotencyClaim>,
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

    /// Apply every clock-due transition, then hand back Region `region`'s record for
    /// `id`.
    ///
    /// `None` for an id this service never minted, for one whose teardown has taken it
    /// past [`DEALLOCATED_RETENTION`] (a torn-down record the clock has already made
    /// due for removal is removed here, so a reporting operation answers not-found for
    /// it rather than reporting a resource the service would have forgotten by now),
    /// and for one whose record belongs to ANOTHER Region: that instance is not one
    /// this Region's endpoint holds, so every operation looking it up here answers
    /// exactly as it does for an id that was never minted.
    pub fn settled(
        &mut self,
        region: &str,
        id: &str,
        now: Instant,
    ) -> Option<&WorkspaceInstanceRecord> {
        self.settled_mut(region, id, now).map(|record| &*record)
    }

    /// [`Self::settled`] for an operation that goes on to CHANGE the record.
    pub fn settled_mut(
        &mut self,
        region: &str,
        id: &str,
        now: Instant,
    ) -> Option<&mut WorkspaceInstanceRecord> {
        // The clock-due work is done under its own borrow, so the forgotten record can
        // be dropped (which needs the store again) before the record is handed back.
        // The Region is checked FIRST: a call signed for another Region must not even
        // settle this record, since to that caller the instance does not exist.
        let forgotten = {
            let record = self.instances.get_mut(id)?;
            if record.region != region {
                return None;
            }
            record.settle(now);
            record.is_forgotten(now)
        };
        if forgotten {
            self.forget(id);
            return None;
        }
        self.instances.get_mut(id)
    }

    /// Apply every clock-due transition across the whole store, and drop every record
    /// whose teardown retention has elapsed.
    ///
    /// The volume attachments are settled with the instances: tearing a WorkSpace
    /// Instance down detaches the volumes attached to it, and that was owed at the
    /// instant the delete was accepted whether or not anybody called afterwards — so a
    /// volume whose instance is tearing down (or is no longer reported at all) holds no
    /// attachment, and is free to be attached elsewhere or deleted.
    pub fn settle_all(&mut self, now: Instant) {
        let forgotten: Vec<String> = self
            .instances
            .values_mut()
            .filter_map(|record| {
                record.settle(now);
                record.is_forgotten(now).then(|| record.id.clone())
            })
            .collect();
        for id in forgotten {
            self.forget(&id);
        }
        // Disjoint field borrows: the instances are read while the volumes are written.
        let instances = &self.instances;
        for volume in self.volumes.values_mut() {
            let released = volume.attachment.as_ref().is_some_and(|attachment| {
                instances
                    .get(&attachment.workspace_instance_id)
                    .is_none_or(WorkspaceInstanceRecord::is_tearing_down)
            });
            if released {
                volume.attachment = None;
            }
        }
    }

    /// Stop reporting a torn-down record: the record goes, and so does the
    /// `ClientToken` of the create that made it, since replaying that token must not
    /// hand back an id the service no longer knows. The claim is retired under the
    /// Region the record belonged to, which is the Region that create was signed for.
    fn forget(&mut self, id: &str) {
        if let Some(removed) = self.instances.remove(id) {
            if let Some(token) = removed.client_token {
                self.client_tokens.remove(&(removed.region, token));
            }
        }
    }

    /// The instances of Region `region` that count against the running-instance quota:
    /// every record of that Region no delete has been accepted for. See
    /// [`MAX_INSTANCES`].
    ///
    /// Per Region as well as per account: a Service Quotas limit applies to one
    /// account in one Region, so instances in another Region leave this one's capacity
    /// untouched.
    pub fn running_instance_count(&self, region: &str) -> usize {
        self.instances
            .values()
            .filter(|record| record.region == region && !record.is_tearing_down())
            .count()
    }

    /// The records Region `region` holds in total, counting the ones that are tearing
    /// down and so no longer count against [`MAX_INSTANCES`]. See
    /// [`MAX_HELD_INSTANCES`].
    pub fn held_instance_count(&self, region: &str) -> usize {
        self.instances
            .values()
            .filter(|record| record.region == region)
            .count()
    }

    /// Whether another instance fits inside BOTH bounds this service keeps for Region
    /// `region`: the running-instance quota [`MAX_INSTANCES`], and
    /// [`MAX_HELD_INSTANCES`] on the records held in total, so a create/delete burst
    /// cannot grow the store without bound through the records teardown leaves
    /// reported.
    pub fn has_instance_capacity(&self, region: &str) -> bool {
        self.running_instance_count(region) < MAX_INSTANCES
            && self.held_instance_count(region) < MAX_HELD_INSTANCES
    }

    /// Mint the next `vol-…` volume id.
    ///
    /// The `vol-` prefix is 4 characters and the model requires 1..63 further
    /// `[0-9a-zA-Z]`, so a 17-hex-digit counter is well inside the pattern. The
    /// counter is the one [`Self::mint_ids`] draws from, so no two resources of this
    /// service ever share a number.
    pub fn mint_volume_id(&mut self) -> String {
        self.id_counter += 1;
        format!("vol-{:017x}", self.id_counter)
    }

    /// Whether another volume fits inside [`MAX_VOLUMES`] for Region `region`.
    pub fn has_volume_capacity(&self, region: &str) -> bool {
        self.volume_count(region) < MAX_VOLUMES
    }

    /// The volumes Region `region` holds: what that Region's volume quota is counted
    /// against, per account AND per Region like the instance quota above.
    pub fn volume_count(&self, region: &str) -> usize {
        self.volumes
            .values()
            .filter(|record| record.region == region)
            .count()
    }

    /// Take Region `region`'s volume `id` out of the store, as `DeleteVolume` does.
    ///
    /// `None` both for an id this service never minted and for one whose volume
    /// belongs to another Region: that volume is not one this Region's endpoint holds,
    /// so the delete answers not-found and the volume itself is left untouched.
    ///
    /// The `ClientToken` of the create that made the volume is retired WITH the record,
    /// exactly as [`Self::forget`] retires an instance's: replaying that token must not
    /// hand back an id the service no longer knows, and a claim that outlived its
    /// resource would let a create/delete cycle grow the claim map without bound even
    /// though [`MAX_VOLUMES`] holds.
    pub fn remove_volume(&mut self, region: &str, id: &str) -> Option<VolumeRecord> {
        if self.volumes.get(id)?.region != region {
            return None;
        }
        let removed = self.volumes.remove(id)?;
        if let Some(token) = removed.client_token.clone() {
            self.volume_client_tokens
                .remove(&(removed.region.clone(), token));
        }
        Some(removed)
    }

    /// Region `region`'s volume `id`.
    ///
    /// `None` both for an id this service never minted and for one whose volume belongs
    /// to ANOTHER Region, exactly as [`Self::remove_volume`] and [`Self::settled`]
    /// answer: that volume is not one this Region's endpoint holds, so every operation
    /// looking it up here answers as it does for an id that was never minted.
    pub fn volume(&self, region: &str, id: &str) -> Option<&VolumeRecord> {
        self.volumes.get(id).filter(|record| record.region == region)
    }

    /// [`Self::volume`] for an operation that goes on to CHANGE the record.
    pub fn volume_mut(&mut self, region: &str, id: &str) -> Option<&mut VolumeRecord> {
        self.volumes
            .get_mut(id)
            .filter(|record| record.region == region)
    }

    /// The volume of Region `region` already attached to `workspace_instance_id` at
    /// `device`, or `None` when that device path carries no volume.
    ///
    /// One device path on an instance carries one volume, so this is the question an
    /// attach has to ask before it writes: the attachments are held per volume, so the
    /// occupant of a device is found by looking across them.
    pub fn volume_attached_at(
        &self,
        region: &str,
        workspace_instance_id: &str,
        device: &str,
    ) -> Option<&VolumeRecord> {
        self.volumes.values().find(|volume| {
            volume.region == region
                && volume.attachment.as_ref().is_some_and(|attachment| {
                    attachment.workspace_instance_id == workspace_instance_id
                        && attachment.device == device
                })
        })
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
/// The quota is scoped to one account in one Region and no instance was created, so
/// `ResourceId` names the subject the quota applies to — this emulator's account —
/// and `held` is what that account holds in the Region the create was signed for.
pub fn instance_quota_exceeded(held: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "the account already holds {held} WorkSpace Instances in this Region, the maximum \
             for this quota"
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

/// The modelled `ResourceNotFoundException` for `id`, reporting `resource_type` as
/// the kind that was not found and the request-independent `message`.
///
/// The ONE place this shape is built, so no resource kind can drift on which
/// members it fills: every member the model marks `@required` is set here.
fn resource_not_found(id: &str, resource_type: &str, message: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: message.to_string(),
        resource_id: id.to_string(),
        resource_type: resource_type.to_string(),
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
    field_validation_failed("NextToken", "NextToken is not a token this service issued")
}

/// The modelled `ValidationException` (HTTP 400) for ONE member of a request this
/// service cannot act on.
///
/// The ONE place this shape is built for a bad member, so every refusal of this
/// service carries the same structure: `Reason` is the modelled single-member reason
/// [`VALIDATION_REASON_FIELD_VALIDATION_FAILED`], `FieldList` names the member, and
/// the field's `Message` is the overall `Message` — one wording, in both places a
/// client reads.
///
/// `why` names the member and the constraint it violated and NONE of what the caller
/// sent: this service does not reflect request values back into an error body (see
/// [`INSTANCE_NOT_FOUND_MESSAGE`]), which is why the refusal renderer in
/// `crate::service` states a boundary violation the same way.
pub fn field_validation_failed(name: &str, why: impl Into<String>) -> ValidationException {
    let why = why.into();
    ValidationException {
        message: why.clone(),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: name.to_string(),
            reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
            message: why,
        }]),
    }
}

/// The tag-key prefix AWS keeps for the tags it generates itself.
///
/// AWS's tagging guidance reserves this prefix — in any mix of upper and lower case —
/// for AWS's own use: a caller may neither create nor edit a tag key that begins with
/// it. The modelled `TagKey` pattern (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`,
/// `smithy_query(query="shape",shape="TagKey")`) admits `:`, so no boundary validator
/// can refuse such a key and the operations that store tags enforce it themselves
/// through [`reject_reserved_tag_keys`].
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is a tag key in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// Case-insensitive, because the prefix is reserved whatever case it is written in.
/// Only the PREFIX is reserved: a key that merely contains it later on, or that stops
/// short of the colon, is an ordinary caller key.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Refuse a request carrying a caller-supplied tag key in the reserved namespace.
///
/// `member` is the input member the keys arrived in, named in the refusal exactly as
/// [`field_validation_failed`] names any other bad member. Shared by every operation
/// that stores caller tags, so one reserved key is refused the same way whichever
/// member carried it, and nothing is written for a request that is going to be
/// refused. The wording states the rule and none of what the caller sent.
pub fn reject_reserved_tag_keys<'a>(
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), ValidationException> {
    if keys.into_iter().any(is_reserved_tag_key) {
        return Err(field_validation_failed(
            member,
            format!(
                "a tag key must not begin with the reserved {RESERVED_TAG_KEY_PREFIX} prefix, \
                 which carries the tags AWS generates"
            ),
        ));
    }
    Ok(())
}

/// Every tag key the `TagSpecification` list of a create request carries, whichever
/// resource type each specification names.
///
/// The reserved-namespace rule applies to a key the caller supplied anywhere in the
/// request, so the keys of EVERY specification are checked through
/// [`reject_reserved_tag_keys`] — not only the ones that go on to tag the resource
/// being created.
pub fn specification_tag_keys<'a>(
    specifications: impl IntoIterator<Item = &'a TagSpecification>,
) -> impl Iterator<Item = &'a str> {
    specifications
        .into_iter()
        .flat_map(|specification| specification.tags.iter().flatten())
        .filter_map(|tag| tag.key.as_deref())
}

/// The tags a create request's `TagSpecification` list applies to the ONE resource
/// the call creates, whose `ResourceTypeEnum` token is `resource_type`.
///
/// Each `TagSpecification` carries a `ResourceTypeEnum` and a `TagList`, so a
/// specification that names ANOTHER resource type tags nothing on this call; one that
/// names none, or names this call's own resource type, tags the resource it creates.
/// `Tag.Key` and `Tag.Value` are both optional in the model: an entry with no key
/// names no tag, and an absent value is the empty value its `len 0..256` allows. A
/// repeated key keeps the last value, which is what merging onto a map gives.
///
/// Shared by every create that models a `TagSpecifications` member, so the member is
/// read the same way whichever resource the call creates.
pub fn tags_in_specifications<'a>(
    resource_type: &str,
    specifications: impl IntoIterator<Item = &'a TagSpecification>,
) -> BTreeMap<String, String> {
    let mut tags = BTreeMap::new();
    for specification in specifications {
        let targets_resource = specification
            .resource_type
            .as_deref()
            .is_none_or(|kind| kind == resource_type);
        if !targets_resource {
            continue;
        }
        for tag in specification.tags.iter().flatten() {
            if let Some(key) = tag.key.as_deref() {
                tags.insert(
                    key.to_string(),
                    tag.value.clone().unwrap_or_default(),
                );
            }
        }
    }
    tags
}

/// [`resource_not_found`] lifted into an operation's error channel, answered with
/// [`RESOURCE_NOT_FOUND_STATUS`].
///
/// `wrap` is the operation's own `ResourceNotFoundException` variant, so every
/// operation of this service reports a missing resource through this ONE place and
/// none of them can drift to the shape's status.
fn not_found_error<E>(
    error: ResourceNotFoundException,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(wrap(error), RESOURCE_NOT_FOUND_STATUS)
}

/// The modelled `ResourceNotFoundException` for a WorkSpace Instance id that names
/// no stored record: the request-independent [`INSTANCE_NOT_FOUND_MESSAGE`], the id
/// that was looked up, and this service's [`WORKSPACE_INSTANCE_RESOURCE_TYPE`] token.
pub fn instance_not_found(id: &str) -> ResourceNotFoundException {
    resource_not_found(
        id,
        WORKSPACE_INSTANCE_RESOURCE_TYPE,
        INSTANCE_NOT_FOUND_MESSAGE,
    )
}

/// [`instance_not_found`] lifted into an operation's error channel through
/// [`not_found_error`], so every single-instance operation answers the same
/// (status, code) pair.
pub fn instance_not_found_error<E>(
    id: &str,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    not_found_error(instance_not_found(id), wrap)
}

/// The message this service reports for a volume id that names no volume.
///
/// A FIXED sentence, like [`INSTANCE_NOT_FOUND_MESSAGE`] and for the same reason:
/// the service states that the resource kind was not found and leaves the identity
/// to the modelled `ResourceId` member, so the prose does not vary per request.
pub const VOLUME_NOT_FOUND_MESSAGE: &str = "Workspace managed EBS volume is not found";

/// The modelled `ResourceNotFoundException` for a volume id that names no stored
/// record: the request-independent [`VOLUME_NOT_FOUND_MESSAGE`], the id that was
/// looked up, and this service's [`VOLUME_RESOURCE_TYPE`] token.
pub fn volume_not_found(id: &str) -> ResourceNotFoundException {
    resource_not_found(id, VOLUME_RESOURCE_TYPE, VOLUME_NOT_FOUND_MESSAGE)
}

/// [`volume_not_found`] lifted into an operation's error channel, answered with
/// [`RESOURCE_NOT_FOUND_STATUS`] — the status this service answers the shape with,
/// whichever resource was looked up, so the volume paths cannot drift from the
/// instance paths on the (status, code) pair a client branches on.
pub fn volume_not_found_error<E>(
    id: &str,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    not_found_error(volume_not_found(id), wrap)
}

/// The modelled `ConflictException` (HTTP 409) for a request this service cannot act
/// on because of the CURRENT state of one resource.
///
/// The ONE place the state-conflict shape is built, beside
/// [`idempotent_parameter_mismatch`] for the token case, so every such refusal fills
/// the same members: `ResourceId` names the resource whose state conflicts and
/// `ResourceType` its kind ([`WORKSPACE_INSTANCE_RESOURCE_TYPE`] /
/// [`VOLUME_RESOURCE_TYPE`]), and `Message` states the conflict in a FIXED sentence —
/// the identity travels in `ResourceId`, as it does for
/// [`INSTANCE_NOT_FOUND_MESSAGE`], so the prose does not vary per request and echoes
/// nothing the caller sent.
pub fn resource_conflict(
    resource_id: &str,
    resource_type: &str,
    message: &str,
) -> ConflictException {
    ConflictException {
        message: message.to_string(),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The `Message` for an attach of a volume that already carries an attachment.
///
/// A volume is attached to one WorkSpace Instance at a time
/// ([`VolumeAttachment`]), so a second attach conflicts with the attachment the
/// volume already holds — including one to the same instance, which the backing
/// Amazon EBS `AttachVolume` refuses as `VolumeInUse` too.
pub const VOLUME_ALREADY_ATTACHED_MESSAGE: &str =
    "Workspace managed EBS volume is already attached to a workspace instance";

/// The `Message` for an attach at a device path that another volume already occupies
/// on that WorkSpace Instance. `ResourceId` names that other volume.
pub const DEVICE_ALREADY_IN_USE_MESSAGE: &str =
    "the device is already in use by another volume on the workspace instance";

/// The `Message` for a detach of a volume that is not attached to the WorkSpace
/// Instance the request names — one that carries no attachment at all, or one
/// attached to a different instance.
pub const VOLUME_NOT_ATTACHED_MESSAGE: &str =
    "Workspace managed EBS volume is not attached to the workspace instance";

/// The `Message` for a detach whose `Device` names a device path the volume is not
/// attached at.
pub const VOLUME_DEVICE_MISMATCH_MESSAGE: &str =
    "Workspace managed EBS volume is not attached at the device the request names";

/// The `Message` for an attach to a WorkSpace Instance a delete has already been
/// accepted for, which is tearing down and taking its attachments with it.
pub const INSTANCE_TEARING_DOWN_MESSAGE: &str =
    "workspace instance is being deallocated, so no volume can be attached to it";

/// The `Message` for an attach whose volume sits in a different Availability Zone
/// from the WorkSpace Instance it would be attached to.
///
/// From the backing Amazon EBS `AttachVolume` reference: "The volume and the instance
/// must be in the same Availability Zone" — an attach of a volume elsewhere is refused
/// there as `InvalidVolume.ZoneMismatch`. The wording names the member and the
/// constraint, never what the caller sent, like every other refusal this service
/// builds through [`field_validation_failed`].
pub const VOLUME_ZONE_MISMATCH_MESSAGE: &str =
    "VolumeId must name a volume in the same Availability Zone as the workspace instance";

/// The input member a zone mismatch is reported against: the volume is what the call
/// attaches, and its zone is the one that has to match the instance's placement.
pub const VOLUME_ID_MEMBER: &str = "VolumeId";

/// Whether a volume in `volume_zone` may be attached to a WorkSpace Instance placed in
/// `instance_zone`.
///
/// A block volume and the instance it is attached to live in ONE Availability Zone, so
/// the zones have to be the same one. `instance_zone` is `None` for an instance whose
/// create call named no zone ([`WorkspaceInstanceRecord::placement_availability_zone`]):
/// the service placed it, this emulator reports no zone for it, and a placement nobody
/// can observe decides nothing — such an instance takes a volume from any zone of its
/// Region.
pub fn zones_allow_attachment(instance_zone: Option<&str>, volume_zone: &str) -> bool {
    instance_zone.is_none_or(|zone| zone == volume_zone)
}

/// The modelled `ValidationException` (HTTP 400) for an attach across Availability
/// Zones — the one place that refusal is built, so every attach path refuses it alike.
///
/// A 400 rather than the modelled `ConflictException`: the zone of a volume and the
/// placement of an instance are both fixed when the resource is created and no later
/// call can change either, so this attach is not a state another call clears but a
/// pair of ids that can never be attached to one another — which is how the backing
/// Amazon EBS volume service answers it too (a client `InvalidVolume.*` error, not the
/// `VolumeInUse` it answers for a state).
pub fn volume_zone_mismatch() -> ValidationException {
    field_validation_failed(VOLUME_ID_MEMBER, VOLUME_ZONE_MISMATCH_MESSAGE)
}

/// The `Message` for a delete of a volume that is still attached.
///
/// From the backing Amazon EBS `DeleteVolume` reference: "The volume must be in the
/// `available` state (not attached to an instance)."
pub const VOLUME_IN_USE_MESSAGE: &str =
    "Workspace managed EBS volume is attached to a workspace instance and must be \
     detached before it is deleted";

/// The modelled `ServiceQuotaExceededException` (HTTP 402) for a create that would
/// pass [`MAX_VOLUMES`]. The quota is scoped to one account in one Region and no
/// volume was created, so `ResourceId` names the subject the quota applies to — this
/// emulator's account — and `held` is what it holds in the Region the create was
/// signed for.
pub fn volume_quota_exceeded(held: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "the account already holds {held} volumes in this Region, the maximum for this quota"
        ),
        resource_id: ::engine::emulator_account_id().to_string(),
        resource_type: VOLUME_RESOURCE_TYPE.to_string(),
        service_code: SERVICE_CODE.to_string(),
        quota_code: VOLUME_QUOTA_CODE.to_string(),
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

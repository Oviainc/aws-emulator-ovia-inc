// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workspacesinstances-eucmifrontendapiservice service.
//!
//! The WorkSpace Instance lifecycle is backed here: every value an operation
//! returns is read out of one of these records, never built from the request.

use std::collections::BTreeMap;
use std::sync::OnceLock;
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

/// The `ServiceCode` this service reports in its quota errors: the service's own
/// code, which is also its sigv4 signing name and ARN namespace.
pub const SERVICE_CODE: &str = "workspaces-instances";

/// The `QuotaCode` for the running-instance quota below. AWS publishes no code for
/// it, so this is the emulator's own stable identifier for the quota it enforces.
pub const WORKSPACE_INSTANCE_QUOTA_CODE: &str = "L-WorkSpaceInstancesPerAccount";

/// The `QuotaCode` for the volume quota below. AWS publishes no code for it, so
/// this is the emulator's own stable identifier for the quota it enforces.
pub const VOLUME_QUOTA_CODE: &str = "L-VolumesPerAccount";

/// The number of WorkSpace Instances this emulator will hold at once.
///
/// A real account has a Service Quotas limit on running WorkSpace Instances, and
/// `CreateWorkspaceInstance` models `ServiceQuotaExceededException` (HTTP 402) for
/// reaching it. AWS publishes no figure, so the number itself is CHOSEN: its purpose
/// is that the store cannot grow without bound, and that it is reached through the
/// modelled error rather than by exhausting memory. Capacity is released by
/// `DeleteWorkspaceInstance`, as on the real service.
pub const MAX_INSTANCES: usize = 500;

/// The number of volumes this emulator will hold at once.
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

/// The letters an AWS Region's Availability Zones are named with.
///
/// A zone's name is its Region's name followed by a letter, counting from `a`
/// (`us-east-1a`). WHICH zones the account has is a fact of the account's Region
/// rather than of this service, so the set is the one this emulator models
/// everywhere: `services/common/src/network.rs` states the account-network contract
/// as "a Region's default subnets sit one per Availability Zone, in the zones named
/// `<region>a`, `<region>b`, `<region>c`", and the EC2 service derives exactly those
/// three zones for a Region. A client that asks EC2 which zones a Region has and then
/// creates a volume here must see one set of zones, not two.
pub const AVAILABILITY_ZONE_LETTERS: [char; 3] = ['a', 'b', 'c'];

/// The Availability Zone names Region `region` has: `<region><letter>`, one per
/// [`AVAILABILITY_ZONE_LETTERS`].
///
/// Derived rather than stored: no operation of this service creates, changes or
/// deletes an Availability Zone, so two emulators built from this source place a
/// volume in the same zones and reading the set changes nothing.
pub fn availability_zones_in_region(region: &str) -> Vec<String> {
    AVAILABILITY_ZONE_LETTERS
        .iter()
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
    /// Every live volume, keyed by its `vol-…` id.
    pub volumes: BTreeMap<String, VolumeRecord>,
    /// `ClientToken` -> `VolumeId`, for `CreateVolume`'s `@idempotent` replay. A
    /// separate namespace from [`Self::client_tokens`]: a token is scoped to the
    /// create call that carried it, so a volume token can never replay an instance
    /// id (the two ids are not even the same shape).
    pub volume_client_tokens: BTreeMap<String, String>,
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

    /// Whether another volume fits inside [`MAX_VOLUMES`].
    pub fn has_volume_capacity(&self) -> bool {
        self.volumes.len() < MAX_VOLUMES
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

/// [`resource_not_found`] lifted into an operation's error channel, answered with
/// [`RESOURCE_NOT_FOUND_STATUS`].
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

/// The modelled `ServiceQuotaExceededException` (HTTP 402) for a create that would
/// pass [`MAX_VOLUMES`]. The quota is account-scoped and no volume was created, so
/// `ResourceId` names the subject the quota applies to — this emulator's account.
pub fn volume_quota_exceeded(held: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!("the account already holds {held} volumes, the maximum for this quota"),
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
